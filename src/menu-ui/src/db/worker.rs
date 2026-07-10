//! SQLite worker thread.
//!
//! Owns every `rusqlite::Connection` and executes all statements in
//! strict FIFO request order, off the Boa/UI thread — an SD-card
//! query costing tens of milliseconds must never stall a reconcile.
//! (The donor code in the old frontend returned promises but ran
//! rusqlite inline inside `JsPromise::new`; this makes the async
//! real.)
//!
//! Everything crossing the channel is plain owned data ([`SqlValue`],
//! Strings, ids). JS conversion happens on the UI side in
//! `bridge.rs`.
//!
//! Transactions: SQLite transactions are connection-level, and the
//! worker executes one request at a time, so `begin`/`commit`/
//! `rollback` are ordinary FIFO ops (`BEGIN`/`COMMIT`/`ROLLBACK`).
//! At most one transaction is active per connection; the returned
//! token is validated for balance. Ops issued between `begin` and
//! `commit` run inside the transaction by construction (FIFO) — the
//! same effective semantics the old single-threaded code had, since
//! it also multiplexed one physical connection.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use rusqlite::Connection;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, Value, ValueRef};
use tracing::{debug, trace};

/// A SQL value in transport form — no JS types, `Send` by
/// construction.
#[derive(Clone, Debug)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Number(f64),
    Text(String),
    Binary(Vec<u8>),
    Json(serde_json::Value),
}

impl FromSql for SqlValue {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Null => Ok(SqlValue::Null),
            ValueRef::Integer(i) => Ok(SqlValue::Integer(i)),
            ValueRef::Real(f) => Ok(SqlValue::Number(f)),
            ValueRef::Text(s) => Ok(SqlValue::Text(
                std::str::from_utf8(s)
                    .map_err(|_| FromSqlError::InvalidType)?
                    .to_string(),
            )),
            ValueRef::Blob(b) => Ok(SqlValue::Binary(b.to_vec())),
        }
    }
}

impl From<SqlValue> for Value {
    fn from(value: SqlValue) -> Self {
        match value {
            SqlValue::Null => Value::Null,
            SqlValue::Integer(i) => Value::Integer(i),
            SqlValue::Number(f) => Value::Real(f),
            SqlValue::Text(s) => Value::Text(s),
            SqlValue::Binary(b) => Value::Blob(b),
            SqlValue::Json(json) => Value::Text(json.to_string()),
        }
    }
}

/// Result rows in transport form.
#[derive(Clone, Debug, Default)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
}

pub type DbId = u32;
pub type TxId = u32;
pub type ReqId = u64;

#[derive(Debug)]
pub enum Request {
    /// Open (or create) `<root>/<name>.sqlite`.
    Open { name: String },
    /// Open an explicit path (downloaded catalog DBs).
    OpenPath { path: String },
    /// Delete `<root>/<name>.sqlite`. Fails if that DB is open.
    Reset { name: String },
    Query {
        db: DbId,
        sql: String,
        bindings: Vec<SqlValue>,
    },
    QueryOne {
        db: DbId,
        sql: String,
        bindings: Vec<SqlValue>,
    },
    Execute {
        db: DbId,
        sql: String,
        bindings: Vec<SqlValue>,
    },
    ExecuteRaw { db: DbId, sql: String },
    ExecuteMany {
        db: DbId,
        sql: String,
        bindings: Vec<Vec<SqlValue>>,
    },
    Begin { db: DbId },
    Commit { db: DbId, tx: TxId },
    Rollback { db: DbId, tx: TxId },
}

#[derive(Debug)]
pub enum ReplyData {
    Opened(DbId),
    Rows(Rows),
    Row(Option<Rows>), // one-row variant reuses Rows with 0..1 entries
    Count(usize),
    Tx(TxId),
    Done,
}

#[derive(Debug)]
pub struct Reply {
    pub req: ReqId,
    pub result: Result<ReplyData, String>,
}

/// UI-side handle: enqueue requests, tagged with fresh request ids.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<(ReqId, Request)>,
}

impl WorkerHandle {
    pub fn send(&self, req_id: ReqId, req: Request) {
        // A closed channel means the worker died; the bridge will
        // never see a reply and the promise stays pending — the
        // worker logs its panic/exit, so don't double-report here.
        let _ = self.tx.send((req_id, req));
    }
}

/// Root directory for named databases. `/media/fat/1fpga` on the
/// device; a local config dir when developing off-target.
fn db_root() -> PathBuf {
    let fat = PathBuf::from("/media/fat");
    if fat.exists() {
        fat.join("1fpga")
    } else {
        let d = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config/1fpga");
        d
    }
}

struct DbState {
    connection: Connection,
    /// The single active transaction's token, if any.
    active_tx: Option<TxId>,
    /// Named-open bookkeeping so `Reset` can refuse deleting an open DB.
    name: Option<String>,
}

/// Spawn the worker. Replies flow to `reply_tx`; the runtime drains
/// them on the UI thread each tick (see `bridge.rs`).
pub fn spawn(reply_tx: Sender<Reply>) -> WorkerHandle {
    let (tx, rx): (Sender<(ReqId, Request)>, Receiver<(ReqId, Request)>) = channel();
    std::thread::Builder::new()
        .name("db-worker".into())
        .spawn(move || worker_main(rx, reply_tx))
        .expect("spawn db worker");
    WorkerHandle { tx }
}

fn worker_main(rx: Receiver<(ReqId, Request)>, reply_tx: Sender<Reply>) {
    let mut dbs: HashMap<DbId, DbState> = HashMap::new();
    let mut next_db: DbId = 1;
    let mut next_tx: TxId = 1;

    while let Ok((req_id, req)) = rx.recv() {
        trace!("db-worker: {:?}", req);
        let result = handle(&mut dbs, &mut next_db, &mut next_tx, req);
        if reply_tx.send(Reply { req: req_id, result }).is_err() {
            break; // UI gone; shut down.
        }
    }
    debug!("db-worker: exiting");
}

fn handle(
    dbs: &mut HashMap<DbId, DbState>,
    next_db: &mut DbId,
    next_tx: &mut TxId,
    req: Request,
) -> Result<ReplyData, String> {
    match req {
        Request::Open { name } => {
            let root = db_root();
            std::fs::create_dir_all(&root)
                .map_err(|e| format!("Could not create db root: {e}"))?;
            let path = root.join(format!("{name}.sqlite"));
            open(dbs, next_db, path, Some(name))
        }
        Request::OpenPath { path } => open(dbs, next_db, PathBuf::from(path), None),
        Request::Reset { name } => {
            if dbs.values().any(|d| d.name.as_deref() == Some(name.as_str())) {
                return Err(format!("Database '{name}' is open; close it first"));
            }
            let path = db_root().join(format!("{name}.sqlite"));
            std::fs::remove_file(&path)
                .map_err(|e| format!("Could not delete database: {e}"))?;
            Ok(ReplyData::Done)
        }
        Request::Query { db, sql, bindings } => {
            let rows = run_query(conn(dbs, db)?, &sql, &bindings, None)?;
            Ok(ReplyData::Rows(rows))
        }
        Request::QueryOne { db, sql, bindings } => {
            let rows = run_query(conn(dbs, db)?, &sql, &bindings, Some(1))?;
            Ok(ReplyData::Row(if rows.rows.is_empty() {
                None
            } else {
                Some(rows)
            }))
        }
        Request::Execute { db, sql, bindings } => {
            let connection = conn(dbs, db)?;
            let mut stmt = connection.prepare(&sql).map_err(sql_err)?;
            bind(&mut stmt, bindings)?;
            let count = stmt.raw_execute().map_err(sql_err)?;
            Ok(ReplyData::Count(count))
        }
        Request::ExecuteRaw { db, sql } => {
            conn(dbs, db)?.execute_batch(&sql).map_err(sql_err)?;
            Ok(ReplyData::Done)
        }
        Request::ExecuteMany { db, sql, bindings } => {
            let connection = conn(dbs, db)?;
            let mut stmt = connection.prepare(&sql).map_err(sql_err)?;
            let mut total = 0;
            for b in bindings {
                bind(&mut stmt, b)?;
                total += stmt.raw_execute().map_err(sql_err)?;
            }
            Ok(ReplyData::Count(total))
        }
        Request::Begin { db } => {
            let state = state_mut(dbs, db)?;
            if state.active_tx.is_some() {
                return Err("A transaction is already active on this database".into());
            }
            state.connection.execute_batch("BEGIN").map_err(sql_err)?;
            *next_tx += 1;
            state.active_tx = Some(*next_tx);
            Ok(ReplyData::Tx(*next_tx))
        }
        Request::Commit { db, tx } => {
            end_tx(dbs, db, tx, "COMMIT")?;
            Ok(ReplyData::Done)
        }
        Request::Rollback { db, tx } => {
            end_tx(dbs, db, tx, "ROLLBACK")?;
            Ok(ReplyData::Done)
        }
    }
}

fn open(
    dbs: &mut HashMap<DbId, DbState>,
    next_db: &mut DbId,
    path: PathBuf,
    name: Option<String>,
) -> Result<ReplyData, String> {
    debug!("db-worker: opening {:?}", path);
    let connection = Connection::open(&path).map_err(sql_err)?;
    *next_db += 1;
    dbs.insert(
        *next_db,
        DbState { connection, active_tx: None, name },
    );
    Ok(ReplyData::Opened(*next_db))
}

fn conn<'a>(dbs: &'a HashMap<DbId, DbState>, db: DbId) -> Result<&'a Connection, String> {
    dbs.get(&db)
        .map(|s| &s.connection)
        .ok_or_else(|| format!("Unknown database handle {db}"))
}

fn state_mut<'a>(
    dbs: &'a mut HashMap<DbId, DbState>,
    db: DbId,
) -> Result<&'a mut DbState, String> {
    dbs.get_mut(&db)
        .ok_or_else(|| format!("Unknown database handle {db}"))
}

fn end_tx(
    dbs: &mut HashMap<DbId, DbState>,
    db: DbId,
    tx: TxId,
    stmt: &str,
) -> Result<(), String> {
    let state = state_mut(dbs, db)?;
    match state.active_tx {
        Some(active) if active == tx => {
            state.connection.execute_batch(stmt).map_err(sql_err)?;
            state.active_tx = None;
            Ok(())
        }
        Some(_) => Err(format!("Transaction {tx} is not the active transaction")),
        None => Err(format!("Transaction {tx} not found")),
    }
}

fn bind(stmt: &mut rusqlite::Statement<'_>, bindings: Vec<SqlValue>) -> Result<(), String> {
    for (i, b) in bindings.into_iter().map(Value::from).enumerate() {
        stmt.raw_bind_parameter(i + 1, b).map_err(sql_err)?;
    }
    Ok(())
}

fn run_query(
    connection: &Connection,
    sql: &str,
    bindings: &[SqlValue],
    limit: Option<usize>,
) -> Result<Rows, String> {
    let mut stmt = connection.prepare(sql).map_err(sql_err)?;
    let columns: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    bind(&mut stmt, bindings.to_vec())?;
    let mut raw = stmt.raw_query();
    let mut rows = Vec::new();
    while let Some(row) = raw.next().map_err(sql_err)? {
        let mut out = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            let v: SqlValue = row.get(i).map_err(sql_err)?;
            out.push(v);
        }
        rows.push(out);
        if let Some(l) = limit
            && rows.len() >= l
        {
            break;
        }
    }
    Ok(Rows { columns, rows })
}

fn sql_err(e: impl std::fmt::Display) -> String {
    format!("SQL Error: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn roundtrip(handle: &WorkerHandle, rx: &Receiver<Reply>, id: ReqId, req: Request) -> ReplyData {
        handle.send(id, req);
        let reply = rx.recv_timeout(Duration::from_secs(5)).expect("reply");
        assert_eq!(reply.req, id);
        reply.result.expect("ok")
    }

    /// Full request/reply round trip through the real worker thread:
    /// open → DDL → insert (many) → tx begin/insert/commit → query
    /// with bindings of every SqlValue kind.
    #[test]
    fn worker_round_trip() {
        let dir = std::env::temp_dir().join(format!("menu-ui-dbtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sqlite");
        let _ = std::fs::remove_file(&path);

        let (reply_tx, rx) = channel();
        let handle = spawn(reply_tx);

        let ReplyData::Opened(db) = roundtrip(
            &handle, &rx, 1,
            Request::OpenPath { path: path.to_string_lossy().into_owned() },
        ) else { panic!("expected Opened") };

        roundtrip(&handle, &rx, 2, Request::ExecuteRaw {
            db,
            sql: "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, blob BLOB, score REAL)".into(),
        });

        let ReplyData::Count(n) = roundtrip(&handle, &rx, 3, Request::ExecuteMany {
            db,
            sql: "INSERT INTO t (name, blob, score) VALUES (?, ?, ?)".into(),
            bindings: vec![
                vec![SqlValue::Text("a".into()), SqlValue::Binary(vec![1, 2]), SqlValue::Number(0.5)],
                vec![SqlValue::Text("b".into()), SqlValue::Null, SqlValue::Integer(2)],
            ],
        }) else { panic!("expected Count") };
        assert_eq!(n, 2);

        // Transaction: rows written inside are visible after commit.
        let ReplyData::Tx(tx) = roundtrip(&handle, &rx, 4, Request::Begin { db })
        else { panic!("expected Tx") };
        roundtrip(&handle, &rx, 5, Request::Execute {
            db,
            sql: "INSERT INTO t (name) VALUES (?)".into(),
            bindings: vec![SqlValue::Text("c".into())],
        });
        roundtrip(&handle, &rx, 6, Request::Commit { db, tx });

        let ReplyData::Rows(rows) = roundtrip(&handle, &rx, 7, Request::Query {
            db,
            sql: "SELECT name, blob, score FROM t ORDER BY id".into(),
            bindings: vec![],
        }) else { panic!("expected Rows") };
        assert_eq!(rows.columns, vec!["name", "blob", "score"]);
        assert_eq!(rows.rows.len(), 3);
        assert!(matches!(&rows.rows[0][1], SqlValue::Binary(b) if b == &vec![1u8, 2]));
        assert!(matches!(&rows.rows[1][1], SqlValue::Null));

        let ReplyData::Row(Some(one)) = roundtrip(&handle, &rx, 8, Request::QueryOne {
            db,
            sql: "SELECT name FROM t WHERE name = ?".into(),
            bindings: vec![SqlValue::Text("c".into())],
        }) else { panic!("expected Row(Some)") };
        assert!(matches!(&one.rows[0][0], SqlValue::Text(s) if s == "c"));

        // Double-begin is rejected; unknown handles error cleanly.
        let ReplyData::Tx(tx2) = roundtrip(&handle, &rx, 9, Request::Begin { db })
        else { panic!() };
        handle.send(10, Request::Begin { db });
        let reply = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(reply.result.is_err(), "second begin must fail");
        roundtrip(&handle, &rx, 11, Request::Rollback { db, tx: tx2 });

        let _ = std::fs::remove_file(&path);
    }
}
