//! `1fpga:db` — SQLite access for the frontend.
//!
//! Ported from the old `one_fpga` frontend (its schemas and query
//! code carry over unchanged), with one structural upgrade: every
//! call is now REALLY asynchronous. Statements execute on a dedicated
//! worker thread ([`worker`]) and results come back as pending
//! promises settled by the runtime's per-tick drain ([`bridge`]) —
//! the old code returned promises but ran rusqlite inline on the JS
//! thread, which would stall reconciles for the duration of an
//! SD-card query.
//!
//! JS surface (all promise-returning):
//!   import * as db from '1fpga:db';
//!   const d  = await db.load('1fpga');          // <root>/1fpga.sqlite
//!   const c  = await db.loadPath('/media/fat/catalog/games.sqlite');
//!   const rs = await d.query('SELECT ... WHERE x = ?', [x]); // {rows}
//!   const r  = await d.queryOne('...');          // object | null
//!   const n  = await d.execute('...', [..]);     // affected count
//!   await d.executeRaw('...multi-statement...');
//!   const n2 = await d.executeMany('INSERT ...', [[..], [..]]);
//!   const tx = await d.beginTransaction();
//!   ... tx.query/execute/... ; await tx.commit() / tx.rollback();
//!   await db.reset('name');

pub mod bridge;
pub mod worker;

use boa_engine::builtins::typed_array::TypedArray;
use boa_engine::class::Class;
use boa_engine::module::MapModuleLoader;
use boa_engine::object::builtins::{JsPromise, JsUint8Array};
use boa_engine::{
    Context, JsObject, JsResult, JsValue, JsVariant, js_error,
};
use boa_macros::{Finalize, JsData, Trace, boa_class, boa_module};

use bridge::{DbBridge, Shape};
use worker::{DbId, Request, SqlValue, TxId};

/// JS → transport conversion (mirrors the old frontend's mapping:
/// booleans become integers, typed arrays become blobs, other objects
/// serialize as JSON text).
fn sql_value_from_js(value: &JsValue, context: &mut Context) -> JsResult<SqlValue> {
    Ok(match value.variant() {
        JsVariant::Null | JsVariant::Undefined => SqlValue::Null,
        JsVariant::Boolean(b) => SqlValue::Integer(b as i64),
        JsVariant::String(s) => SqlValue::Text(s.to_std_string_escaped()),
        JsVariant::Float64(r) => SqlValue::Number(r),
        JsVariant::Integer32(i) => SqlValue::Integer(i as i64),
        JsVariant::Object(o) if o.is::<TypedArray>() => {
            let array = JsUint8Array::from_object(o.clone())?;
            SqlValue::Binary(array.iter(context).collect())
        }
        _ => SqlValue::Json(
            value
                .to_json(context)?
                .ok_or_else(|| js_error!("Failed to convert binding value."))?,
        ),
    })
}

fn bindings_from_js(
    bindings: &Option<Vec<JsValue>>,
    context: &mut Context,
) -> JsResult<Vec<SqlValue>> {
    match bindings {
        None => Ok(Vec::new()),
        Some(vs) => vs.iter().map(|v| sql_value_from_js(v, context)).collect(),
    }
}

fn the_bridge(context: &mut Context) -> JsResult<DbBridge> {
    context
        .get_data::<DbBridge>()
        .cloned()
        .ok_or_else(|| js_error!("db bridge not initialised"))
}

/// An open database. Thin JS wrapper over a worker-side handle.
#[derive(Clone, Trace, Finalize, JsData)]
pub struct JsDb {
    db: DbId,
}

impl JsDb {
    pub(crate) fn from_opened(db: DbId, context: &mut Context) -> JsResult<JsObject> {
        JsDb::from_data(JsDb { db }, context)
    }
}

#[boa_class(rename = "Db")]
#[boa(rename_all = "camelCase")]
impl JsDb {
    #[boa(constructor)]
    fn constructor() -> JsResult<Self> {
        Err(js_error!("Use db.load()/db.loadPath() instead of new Db()"))
    }

    pub fn query(
        &self,
        query: String,
        bindings: Option<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings_from_js(&bindings, context)?;
        the_bridge(context)?.submit(
            Request::Query { db: self.db, sql: query, bindings },
            Shape::RowsObject,
            context,
        )
    }

    pub fn query_one(
        &self,
        query: String,
        bindings: Option<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings_from_js(&bindings, context)?;
        the_bridge(context)?.submit(
            Request::QueryOne { db: self.db, sql: query, bindings },
            Shape::RowOrNull,
            context,
        )
    }

    pub fn execute(
        &self,
        query: String,
        bindings: Option<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings_from_js(&bindings, context)?;
        the_bridge(context)?.submit(
            Request::Execute { db: self.db, sql: query, bindings },
            Shape::Count,
            context,
        )
    }

    pub fn execute_raw(&self, query: String, context: &mut Context) -> JsResult<JsPromise> {
        the_bridge(context)?.submit(
            Request::ExecuteRaw { db: self.db, sql: query },
            Shape::Unit,
            context,
        )
    }

    pub fn execute_many(
        &self,
        query: String,
        bindings: Vec<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings
            .iter()
            .map(|row| {
                row.iter()
                    .map(|v| sql_value_from_js(v, context))
                    .collect::<JsResult<Vec<_>>>()
            })
            .collect::<JsResult<Vec<_>>>()?;
        the_bridge(context)?.submit(
            Request::ExecuteMany { db: self.db, sql: query, bindings },
            Shape::Count,
            context,
        )
    }

    pub fn begin_transaction(&self, context: &mut Context) -> JsResult<JsPromise> {
        the_bridge(context)?.submit(
            Request::Begin { db: self.db },
            Shape::Tx { db: self.db },
            context,
        )
    }
}

/// An active transaction. Same op surface as [`JsDb`]; the worker
/// validates token balance. All ops between `beginTransaction()` and
/// `commit()`/`rollback()` run inside the transaction (the worker is
/// strictly FIFO on one connection).
#[derive(Clone, Trace, Finalize, JsData)]
pub struct JsDbTransaction {
    db: DbId,
    tx: TxId,
}

impl JsDbTransaction {
    pub(crate) fn from_parts(db: DbId, tx: TxId, context: &mut Context) -> JsResult<JsObject> {
        JsDbTransaction::from_data(JsDbTransaction { db, tx }, context)
    }
}

#[boa_class(rename = "DbTransaction")]
#[boa(rename_all = "camelCase")]
impl JsDbTransaction {
    #[boa(constructor)]
    fn constructor() -> JsResult<Self> {
        Err(js_error!("Cannot construct DbTransaction directly"))
    }

    pub fn query(
        &self,
        query: String,
        bindings: Option<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings_from_js(&bindings, context)?;
        the_bridge(context)?.submit(
            Request::Query { db: self.db, sql: query, bindings },
            Shape::RowsObject,
            context,
        )
    }

    pub fn query_one(
        &self,
        query: String,
        bindings: Option<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings_from_js(&bindings, context)?;
        the_bridge(context)?.submit(
            Request::QueryOne { db: self.db, sql: query, bindings },
            Shape::RowOrNull,
            context,
        )
    }

    pub fn execute(
        &self,
        query: String,
        bindings: Option<Vec<JsValue>>,
        context: &mut Context,
    ) -> JsResult<JsPromise> {
        let bindings = bindings_from_js(&bindings, context)?;
        the_bridge(context)?.submit(
            Request::Execute { db: self.db, sql: query, bindings },
            Shape::Count,
            context,
        )
    }

    pub fn execute_raw(&self, query: String, context: &mut Context) -> JsResult<JsPromise> {
        the_bridge(context)?.submit(
            Request::ExecuteRaw { db: self.db, sql: query },
            Shape::Unit,
            context,
        )
    }

    pub fn commit(&self, context: &mut Context) -> JsResult<JsPromise> {
        the_bridge(context)?.submit(
            Request::Commit { db: self.db, tx: self.tx },
            Shape::Unit,
            context,
        )
    }

    pub fn rollback(&self, context: &mut Context) -> JsResult<JsPromise> {
        the_bridge(context)?.submit(
            Request::Rollback { db: self.db, tx: self.tx },
            Shape::Unit,
            context,
        )
    }
}

#[boa_module]
#[boa(rename_all = "camelCase")]
mod js_module {
    use super::*;

    fn load(name: String, context: &mut Context) -> JsResult<JsPromise> {
        super::the_bridge(context)?.submit(Request::Open { name }, Shape::Db, context)
    }

    fn load_path(path: String, context: &mut Context) -> JsResult<JsPromise> {
        super::the_bridge(context)?.submit(Request::OpenPath { path }, Shape::Db, context)
    }

    fn reset(name: String, context: &mut Context) -> JsResult<JsPromise> {
        super::the_bridge(context)?.submit(Request::Reset { name }, Shape::Unit, context)
    }
}

/// Register the `1fpga:db` module, its classes, and start the worker.
/// Returns the bridge so the runtime can drain replies per tick.
pub fn register(loader: &MapModuleLoader, context: &mut Context) -> JsResult<DbBridge> {
    let bridge = DbBridge::start();
    context.insert_data(bridge.clone());
    context.register_global_class::<JsDb>()?;
    context.register_global_class::<JsDbTransaction>()?;
    loader.insert("1fpga:db", js_module::boa_module(None, context));
    Ok(bridge)
}

