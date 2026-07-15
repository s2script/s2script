//! Engine-generic SQLite subsystem. Holds NO V8 handles and knows nothing about any game.
//! Connections live in a thread-local registry keyed by opaque integer handles (never a raw
//! pointer crosses to JS) — but the `rusqlite::Connection` itself lives on a dedicated
//! per-connection ACTOR THREAD, never on the main game thread. `submit_query`/`submit_execute`
//! hand a `Command` to the actor over an mpsc channel and return immediately (the game thread
//! never blocks); the actor runs each statement in FIFO submission order and sends a
//! `DbCompletion` back over the shared channel (`completion_tx`/`try_recv_completed`), which the
//! frame drain polls and resolves via `resolve_db` — mirroring the remote sqlx driver in
//! `sqldb.rs`.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use rusqlite::Connection;

/// A SQLite value in either direction (params in, results out). Booleans collapse to `Int(0|1)`
/// (SQLite has no boolean type) — the documented "bool reads back as a number" quirk.
pub enum DbValue { Null, Int(i64), Real(f64), Text(String) }

pub struct QueryResult { pub columns: Vec<String>, pub rows: Vec<Vec<DbValue>> }
pub struct ExecResult { pub changes: i64, pub last_insert_id: i64 }

/// A completed off-thread DB job (SQLite actor OR the remote sqlx tasks in sqldb.rs). Both backends
/// send here; `v8host::frame_async_drain` polls `try_recv_completed()` and resolves via `resolve_db`.
pub(crate) enum DbOutcome { Query(QueryResult), Exec(ExecResult) }
pub(crate) struct DbCompletion { pub id: u64, pub result: Result<DbOutcome, String> }

// Process-global completion channel (mirrors http.rs::ENGINE). Actor threads / tokio tasks send;
// the frame drain (main thread) polls. Shared by BOTH db.rs (SQLite) and sqldb.rs (MySQL/Postgres).
struct Chan { tx: Sender<DbCompletion>, rx: Mutex<Receiver<DbCompletion>> }
static CHAN: OnceLock<Chan> = OnceLock::new();
fn chan() -> &'static Chan { CHAN.get_or_init(|| { let (tx, rx) = channel(); Chan { tx, rx: Mutex::new(rx) } }) }
/// A cloned sender for producers (the SQLite actor loop in this module; sqldb.rs's tokio tasks).
pub(crate) fn completion_tx() -> Sender<DbCompletion> { chan().tx.clone() }
/// Pop one completion, or None. Called on the main thread by the frame drain.
pub(crate) fn try_recv_completed() -> Option<DbCompletion> { chan().rx.lock().ok()?.try_recv().ok() }

/// A command sent to a connection's actor thread. Owned values only (Send) so nothing borrows across
/// the thread boundary.
enum Command {
    Query { id: u64, sql: String, params: Vec<DbValue> },
    Execute { id: u64, sql: String, params: Vec<DbValue> },
    Shutdown,
}

/// The main-thread handle to a connection's actor. The rusqlite `Connection` lives ON the actor
/// thread — never on the main thread, never crossing to JS.
struct ConnHandle { cmd_tx: Sender<Command> }

thread_local! {
    // handle -> (actor handle, owner plugin id). Owner scopes access (charter: no raw cross-plugin
    // reference); query/execute/close verify the CALLER owns the handle.
    static CONNS: RefCell<HashMap<u64, (ConnHandle, String)>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = Cell::new(1);
}

/// A connection name becomes a filename, so it must be a safe single path component.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl rusqlite::ToSql for DbValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value};
        Ok(match self {
            DbValue::Null => ToSqlOutput::Owned(Value::Null),
            DbValue::Int(i) => ToSqlOutput::Owned(Value::Integer(*i)),
            DbValue::Real(f) => ToSqlOutput::Owned(Value::Real(*f)),
            DbValue::Text(s) => ToSqlOutput::Owned(Value::Text(s.clone())),
        })
    }
}

/// The actor loop: owns the Connection, runs each statement in submission (FIFO) order off the game
/// thread, sends a completion back over the shared channel. `catch_unwind` per statement so a panic
/// in rusqlite becomes an Err completion, never a dead actor. Exits on Shutdown (or when all senders
/// drop), dropping the Connection (closing the SQLite handle).
fn actor_loop(conn: Connection, rx: Receiver<Command>) {
    let tx = completion_tx();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Query { id, sql, params } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_query(&conn, &sql, &params)))
                    .unwrap_or_else(|_| Err("db query panicked".to_string()))
                    .map(DbOutcome::Query);
                let _ = tx.send(DbCompletion { id, result });
            }
            Command::Execute { id, sql, params } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_execute(&conn, &sql, &params)))
                    .unwrap_or_else(|_| Err("db execute panicked".to_string()))
                    .map(DbOutcome::Exec);
                let _ = tx.send(DbCompletion { id, result });
            }
            Command::Shutdown => break,
        }
    }
    // conn drops here -> SQLite connection closed.
}

/// Run a parameterized SELECT on the actor's owned connection (the former `query` body, minus the
/// registry lookup — ownership is checked on the main thread before the command is sent).
fn run_query(conn: &Connection, sql: &str, params: &[DbValue]) -> Result<QueryResult, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let ncol = columns.len();
    let mut out_rows = Vec::new();
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut vals = Vec::with_capacity(ncol);
        for i in 0..ncol {
            use rusqlite::types::ValueRef;
            let v = match row.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => DbValue::Null,
                ValueRef::Integer(n) => DbValue::Int(n),
                ValueRef::Real(f) => DbValue::Real(f),
                ValueRef::Text(t) => DbValue::Text(String::from_utf8_lossy(t).into_owned()),
                ValueRef::Blob(_) => DbValue::Null, // blobs out of scope
            };
            vals.push(v);
        }
        out_rows.push(vals);
    }
    Ok(QueryResult { columns, rows: out_rows })
}

/// Run an INSERT/UPDATE/DELETE/DDL on the actor's owned connection (the former `execute` body).
fn run_execute(conn: &Connection, sql: &str, params: &[DbValue]) -> Result<ExecResult, String> {
    let changes = conn
        .execute(sql, rusqlite::params_from_iter(params.iter()))
        .map_err(|e| e.to_string())? as i64;
    Ok(ExecResult { changes, last_insert_id: conn.last_insert_rowid() })
}

pub fn open(data_dir: &Path, name: &str, owner: &str) -> Result<u64, String> {
    if !valid_name(name) {
        return Err(format!("invalid database name: {name:?}"));
    }
    std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create data dir: {e}"))?;
    let mut path: PathBuf = data_dir.to_path_buf();
    path.push(format!("{name}.sqlite"));
    // Eager open on the main thread (runs at load/map-start, not a hot frame): a bad path rejects
    // synchronously. Then the Connection MOVES to the actor thread and never returns to the main thread.
    let conn = Connection::open(&path).map_err(|e| format!("open failed: {e}"))?;
    // Off-thread writes can now contend when two connections share a file (e.g. nominations +
    // rockthevote both open "mapvote"); a locked write should wait, not error.
    let _ = conn.busy_timeout(Duration::from_millis(5000));
    let (cmd_tx, rx) = channel::<Command>();
    thread::Builder::new()
        .name(format!("s2db-{name}"))
        .spawn(move || actor_loop(conn, rx))
        .map_err(|e| format!("cannot spawn db thread: {e}"))?;
    let handle = NEXT.with(|n| { let h = n.get(); n.set(h + 1); h });
    CONNS.with(|c| c.borrow_mut().insert(handle, (ConnHandle { cmd_tx }, owner.to_string())));
    Ok(handle)
}

/// Clone the command sender for a handle the caller owns (a wrong/absent owner is "invalid db handle",
/// not probeable). Mirrors sqldb::get_pool. Private — callers use `submit_query`/`submit_execute`.
fn get_sender(handle: u64, owner: &str) -> Result<Sender<Command>, String> {
    CONNS.with(|c| match c.borrow().get(&handle) {
        Some((h, o)) if o == owner => Ok(h.cmd_tx.clone()),
        _ => Err("invalid db handle".to_string()),
    })
}

/// Owner-check + queue a SELECT on the connection's actor thread. Returns immediately (the game thread
/// does NOT block); the result arrives later as a `DbCompletion` (resolved by the frame drain). Err on
/// a wrong/absent handle ("invalid db handle") or a dead actor ("db connection closed").
pub(crate) fn submit_query(id: u64, handle: u64, sql: String, params: Vec<DbValue>, owner: &str) -> Result<(), String> {
    let sender = get_sender(handle, owner)?;
    sender.send(Command::Query { id, sql, params }).map_err(|_| "db connection closed".to_string())
}

/// Owner-check + queue an INSERT/UPDATE/DELETE/DDL on the connection's actor thread. Same contract as
/// `submit_query`.
pub(crate) fn submit_execute(id: u64, handle: u64, sql: String, params: Vec<DbValue>, owner: &str) -> Result<(), String> {
    let sender = get_sender(handle, owner)?;
    sender.send(Command::Execute { id, sql, params }).map_err(|_| "db connection closed".to_string())
}

/// Close a connection the caller owns: remove-if-owned (one borrow), then signal the actor to drain +
/// exit (detached — never joined, so close never blocks). Idempotent — a wrong owner / already-closed
/// handle is a harmless `false`. Teardown passes the unloading plugin's own id.
pub fn close(handle: u64, owner: &str) -> bool {
    let removed = CONNS.with(|c| {
        let mut map = c.borrow_mut();
        match map.get(&handle) {
            Some((_, o)) if o == owner => map.remove(&handle),
            _ => None,
        }
    });
    match removed {
        Some((h, _)) => { let _ = h.cmd_tx.send(Command::Shutdown); true }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp() -> std::path::PathBuf {
        thread_local! { static N: std::cell::Cell<u64> = std::cell::Cell::new(0); }
        let n = N.with(|c| { let v = c.get(); c.set(v + 1); v });
        let mut p = std::env::temp_dir();
        p.push(format!("s2db_test_{}_{}", std::process::id(), n));
        p
    }
    const O: &str = "pluginA";

    // Block until the completion for `id` arrives (tests are serial; each op is awaited before the
    // next, so the next completion is the one we want). Mirrors http.rs::drain_blocking.
    fn wait_for(id: u64) -> Result<DbOutcome, String> {
        for _ in 0..500 {
            if let Some(c) = try_recv_completed() { if c.id == id { return c.result; } }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no completion for id {id}");
    }
    // Convenience: submit + wait, returning the QueryResult (panics if it was an Exec/Err).
    fn q(id: u64, h: u64, sql: &str, params: Vec<DbValue>) -> QueryResult {
        submit_query(id, h, sql.to_string(), params, O).unwrap();
        match wait_for(id) { Ok(DbOutcome::Query(r)) => r, other => panic!("expected query rows, got {:?}", other.is_ok()) }
    }
    fn e(id: u64, h: u64, sql: &str, params: Vec<DbValue>) -> ExecResult {
        submit_execute(id, h, sql.to_string(), params, O).unwrap();
        match wait_for(id) { Ok(DbOutcome::Exec(r)) => r, _ => panic!("expected exec result") }
    }

    #[test]
    fn open_create_insert_select_roundtrip() {
        let h = open(&tmp(), "t1", O).unwrap();
        e(1, h, "CREATE TABLE kv (k TEXT, v TEXT)", vec![]);
        let r = e(2, h, "INSERT INTO kv (k, v) VALUES (?, ?)",
            vec![DbValue::Text("color".into()), DbValue::Text("red".into())]);
        assert_eq!(r.changes, 1);
        let sel = q(3, h, "SELECT k, v FROM kv WHERE k = ?", vec![DbValue::Text("color".into())]);
        assert_eq!(sel.columns, vec!["k".to_string(), "v".to_string()]);
        assert_eq!(sel.rows.len(), 1);
        match &sel.rows[0][1] { DbValue::Text(s) => assert_eq!(s, "red"), _ => panic!("wrong type") }
        close(h, O);
    }

    #[test]
    fn fifo_ordering_execute_then_query_sees_insert() {
        let h = open(&tmp(), "fifo", O).unwrap();
        e(1, h, "CREATE TABLE n (x INTEGER)", vec![]);
        // Submit the INSERT and the SELECT back-to-back WITHOUT awaiting between them.
        submit_execute(2, h, "INSERT INTO n VALUES (42)".into(), vec![], O).unwrap();
        submit_query(3, h, "SELECT x FROM n".into(), vec![], O).unwrap();
        let _ = wait_for(2);
        let sel = match wait_for(3) { Ok(DbOutcome::Query(r)) => r, _ => panic!() };
        assert_eq!(sel.rows.len(), 1); // the actor ran the INSERT before the SELECT (FIFO)
        match sel.rows[0][0] { DbValue::Int(i) => assert_eq!(i, 42), _ => panic!() }
        close(h, O);
    }

    #[test]
    fn bad_statement_errors_but_actor_survives() {
        let h = open(&tmp(), "survive", O).unwrap();
        submit_query(1, h, "SELECT * FROM nope".into(), vec![], O).unwrap();
        assert!(wait_for(1).is_err()); // bad SQL → Err completion
        // The actor is still alive: a good statement afterward still works.
        e(2, h, "CREATE TABLE ok (a INTEGER)", vec![]);
        let sel = q(3, h, "SELECT COUNT(*) AS c FROM ok", vec![]);
        assert_eq!(sel.rows.len(), 1);
        close(h, O);
    }

    #[test]
    fn invalid_name_rejected() {
        assert!(open(&tmp(), "../evil", O).is_err());
        assert!(open(&tmp(), "a/b", O).is_err());
        assert!(open(&tmp(), "", O).is_err());
    }

    #[test]
    fn submit_to_wrong_owner_is_invalid_handle() {
        let h = open(&tmp(), "owned", "pluginA").unwrap();
        // A different plugin cannot submit against pluginA's handle (synchronous Err, no command sent).
        assert_eq!(submit_query(1, h, "SELECT 1".into(), vec![], "pluginB").unwrap_err(), "invalid db handle");
        assert_eq!(submit_execute(2, h, "SELECT 1".into(), vec![], "pluginB").unwrap_err(), "invalid db handle");
        assert!(!close(h, "pluginB")); // wrong owner → no close
        assert!(close(h, "pluginA"));  // owner closes it
    }

    #[test]
    fn closed_handle_is_invalid() {
        let h = open(&tmp(), "closed", O).unwrap();
        assert!(close(h, O));
        assert_eq!(submit_query(1, h, "SELECT 1".into(), vec![], O).unwrap_err(), "invalid db handle");
        assert!(!close(h, O)); // already gone
    }

    #[test]
    fn concurrent_different_handles() {
        let h1 = open(&tmp(), "c1", O).unwrap();
        let h2 = open(&tmp(), "c2", O).unwrap();
        e(1, h1, "CREATE TABLE a (x INTEGER)", vec![]);
        e(2, h2, "CREATE TABLE b (y INTEGER)", vec![]);
        e(3, h1, "INSERT INTO a VALUES (1)", vec![]);
        e(4, h2, "INSERT INTO b VALUES (2)", vec![]);
        assert_eq!(q(5, h1, "SELECT x FROM a", vec![]).rows.len(), 1);
        assert_eq!(q(6, h2, "SELECT y FROM b", vec![]).rows.len(), 1);
        close(h1, O); close(h2, O);
    }
}
