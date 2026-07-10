//! UI-thread side of the DB worker: request ids, pending-promise
//! registry, and the per-tick reply drain.
//!
//! Every `1fpga:db` call that touches SQLite returns a REAL pending
//! promise: the request is queued to the worker thread and the
//! `(resolve, reject)` pair is parked here. The runtime loop calls
//! [`DbBridge::drain`] once per tick (before `tick_jobs`, so promise
//! continuations run in the same tick their reply arrived in).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};

use boa_engine::object::builtins::{JsArray, JsPromise, JsUint8Array};
use boa_engine::{Context, JsError, JsObject, JsResult, JsString, JsValue, js_error};
use boa_macros::{Finalize, JsData, Trace};

use super::worker::{self, Reply, ReplyData, ReqId, Request, Rows, SqlValue, WorkerHandle};

/// What to construct from a successful [`ReplyData`] when settling
/// the promise.
pub enum Shape {
    /// `{ rows: [...] }` — matches the old frontend's `query()` result.
    RowsObject,
    /// A single row object or `null`.
    RowOrNull,
    /// A plain number (execute counts).
    Count,
    /// `undefined` (executeRaw / commit / rollback / reset).
    Unit,
    /// A `Db` class instance wrapping the opened handle.
    Db,
    /// A `DbTransaction` class instance for the given db handle.
    Tx { db: worker::DbId },
}

struct Pending {
    resolve: boa_engine::object::builtins::JsFunction,
    reject: boa_engine::object::builtins::JsFunction,
    shape: Shape,
}

#[derive(Clone, Trace, Finalize, JsData)]
pub struct DbBridge {
    #[unsafe_ignore_trace]
    inner: Rc<RefCell<Inner>>,
}

struct Inner {
    handle: WorkerHandle,
    replies: Receiver<Reply>,
    pending: HashMap<ReqId, Pending>,
    next_req: ReqId,
}

impl DbBridge {
    /// Spawn the worker and build the bridge. Called once at context
    /// setup.
    pub fn start() -> Self {
        let (reply_tx, replies) = channel();
        let handle = worker::spawn(reply_tx);
        Self {
            inner: Rc::new(RefCell::new(Inner {
                handle,
                replies,
                pending: HashMap::new(),
                next_req: 0,
            })),
        }
    }

    /// Queue `req`, park a fresh promise for it, return the promise.
    pub fn submit(
        &self,
        req: Request,
        shape: Shape,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let (promise, resolvers) = JsPromise::new_pending(context);
        let mut inner = self.inner.borrow_mut();
        inner.next_req += 1;
        let id = inner.next_req;
        inner.pending.insert(
            id,
            Pending {
                resolve: resolvers.resolve,
                reject: resolvers.reject,
                shape,
            },
        );
        inner.handle.send(id, req);
        Ok(promise)
    }

    /// Settle every promise whose reply has arrived. Cheap when idle
    /// (one `try_recv` miss).
    pub fn drain(&self, context: &mut Context) {
        loop {
            // Take one reply + its pending entry with the borrow held,
            // then RELEASE it before touching the context: settling a
            // promise can run arbitrary JS which may re-enter the
            // bridge (e.g. a .then() that issues another query).
            let (pending, result) = {
                let mut inner = self.inner.borrow_mut();
                let Ok(reply) = inner.replies.try_recv() else {
                    return;
                };
                let Some(pending) = inner.pending.remove(&reply.req) else {
                    tracing::warn!("db reply for unknown request {}", reply.req);
                    continue;
                };
                (pending, reply.result)
            };
            let settled = match result {
                Ok(data) => match build_value(&pending.shape, data, context) {
                    Ok(v) => pending.resolve.call(&JsValue::undefined(), &[v], context),
                    Err(e) => reject(&pending, e, context),
                },
                Err(msg) => reject(&pending, js_error!("{}", msg), context),
            };
            if let Err(e) = settled {
                tracing::warn!("db promise settlement threw: {e}");
            }
        }
    }
}

fn reject(pending: &Pending, err: JsError, context: &mut Context) -> JsResult<JsValue> {
    // into_opaque can itself fail on exotic error payloads; fall back
    // to a plain string value so the promise still rejects.
    let v = err
        .into_opaque(context)
        .unwrap_or_else(|_| JsString::from("db error").into());
    pending.reject.call(&JsValue::undefined(), &[v], context)
}

fn build_value(shape: &Shape, data: ReplyData, context: &mut Context) -> JsResult<JsValue> {
    match (shape, data) {
        (Shape::RowsObject, ReplyData::Rows(rows)) => {
            let arr = rows_to_array(&rows, context)?;
            let obj = JsObject::with_null_proto();
            obj.set(JsString::from("rows"), arr, true, context)?;
            Ok(obj.into())
        }
        (Shape::RowOrNull, ReplyData::Row(None)) => Ok(JsValue::null()),
        (Shape::RowOrNull, ReplyData::Row(Some(rows))) => {
            Ok(row_to_object(&rows.columns, &rows.rows[0], context)?.into())
        }
        (Shape::Count, ReplyData::Count(n)) => Ok(JsValue::from(n as u32)),
        (Shape::Unit, ReplyData::Done) => Ok(JsValue::undefined()),
        (Shape::Db, ReplyData::Opened(id)) => {
            super::JsDb::from_opened(id, context).map(JsValue::from)
        }
        (Shape::Tx { db }, ReplyData::Tx(tx)) => {
            super::JsDbTransaction::from_parts(*db, tx, context).map(JsValue::from)
        }
        (_, other) => Err(js_error!("db worker returned mismatched reply: {:?}", other)),
    }
}

fn rows_to_array(rows: &Rows, context: &mut Context) -> JsResult<JsValue> {
    let mut out = Vec::with_capacity(rows.rows.len());
    for row in &rows.rows {
        out.push(row_to_object(&rows.columns, row, context)?.into());
    }
    Ok(JsArray::from_iter(out, context).into())
}

fn row_to_object(
    columns: &[String],
    row: &[SqlValue],
    context: &mut Context,
) -> JsResult<JsObject> {
    let obj = JsObject::with_null_proto();
    for (name, value) in columns.iter().zip(row.iter()) {
        obj.set(
            JsString::from(name.as_str()),
            sql_value_to_js(value, context)?,
            true,
            context,
        )?;
    }
    Ok(obj)
}

pub fn sql_value_to_js(value: &SqlValue, context: &mut Context) -> JsResult<JsValue> {
    Ok(match value {
        SqlValue::Null => JsValue::null(),
        SqlValue::Integer(i) => (*i).into(),
        SqlValue::Number(f) => (*f).into(),
        SqlValue::Text(s) => JsString::from(s.as_str()).into(),
        SqlValue::Binary(b) => JsUint8Array::from_iter(b.iter().cloned(), context)?.into(),
        SqlValue::Json(j) => JsValue::from_json(j, context)?,
    })
}
