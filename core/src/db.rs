//! Engine-generic SQLite subsystem. Holds NO V8 handles and knows nothing about any game.
//! Connections live in a thread-local registry keyed by opaque integer handles (never a raw
//! pointer crosses to JS). Synchronous: callers run on the main isolate thread (see the plan's
//! simplification note); moving to a threadpool later does not change this module's signatures.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use rusqlite::Connection;

/// A SQLite value in either direction (params in, results out). Booleans collapse to `Int(0|1)`
/// (SQLite has no boolean type) — the documented "bool reads back as a number" quirk.
pub enum DbValue { Null, Int(i64), Real(f64), Text(String) }

pub struct QueryResult { pub columns: Vec<String>, pub rows: Vec<Vec<DbValue>> }
pub struct ExecResult { pub changes: i64, pub last_insert_id: i64 }

thread_local! {
    // handle -> (connection, owner plugin id). The owner scopes access: a handle is a small
    // guessable integer, so query/execute/close verify the CALLER owns it (the charter forbids a
    // raw cross-plugin reference — one plugin must not touch another's connection by handle).
    static CONNS: RefCell<HashMap<u64, (Connection, String)>> = RefCell::new(HashMap::new());
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

pub fn open(data_dir: &Path, name: &str, owner: &str) -> Result<u64, String> {
    if !valid_name(name) {
        return Err(format!("invalid database name: {name:?}"));
    }
    std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create data dir: {e}"))?;
    let mut path: PathBuf = data_dir.to_path_buf();
    path.push(format!("{name}.sqlite"));
    let conn = Connection::open(&path).map_err(|e| format!("open failed: {e}"))?;
    let handle = NEXT.with(|n| { let h = n.get(); n.set(h + 1); h });
    CONNS.with(|c| c.borrow_mut().insert(handle, (conn, owner.to_string())));
    Ok(handle)
}

/// Look up a handle, verifying `owner` matches — a wrong owner reads as "invalid db handle" (no
/// distinction between "not yours" and "does not exist", so a handle is not probeable).
fn get_owned<'m>(map: &'m HashMap<u64, (Connection, String)>, handle: u64, owner: &str)
    -> Result<&'m Connection, String>
{
    match map.get(&handle) {
        Some((conn, o)) if o == owner => Ok(conn),
        _ => Err("invalid db handle".to_string()),
    }
}

pub fn query(handle: u64, sql: &str, params: &[DbValue], owner: &str) -> Result<QueryResult, String> {
    CONNS.with(|c| {
        let map = c.borrow();
        let conn = get_owned(&map, handle, owner)?;
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
                    ValueRef::Blob(_) => DbValue::Null, // blobs out of scope this slice
                };
                vals.push(v);
            }
            out_rows.push(vals);
        }
        Ok(QueryResult { columns, rows: out_rows })
    })
}

pub fn execute(handle: u64, sql: &str, params: &[DbValue], owner: &str) -> Result<ExecResult, String> {
    CONNS.with(|c| {
        let map = c.borrow();
        let conn = get_owned(&map, handle, owner)?;
        let changes = conn
            .execute(sql, rusqlite::params_from_iter(params.iter()))
            .map_err(|e| e.to_string())? as i64;
        Ok(ExecResult { changes, last_insert_id: conn.last_insert_rowid() })
    })
}

/// Close a connection the caller owns. Returns true if a connection was removed; a wrong owner or an
/// already-closed handle is a harmless `false` (teardown passes the unloading plugin's own id).
pub fn close(handle: u64, owner: &str) -> bool {
    CONNS.with(|c| {
        let mut map = c.borrow_mut();
        match map.get(&handle) {
            Some((_, o)) if o == owner => { map.remove(&handle); true }
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp() -> std::path::PathBuf {
        // unique per test via a counter (Date/rand are unavailable in this crate's tests)
        thread_local! { static N: std::cell::Cell<u64> = std::cell::Cell::new(0); }
        let n = N.with(|c| { let v = c.get(); c.set(v + 1); v });
        let mut p = std::env::temp_dir();
        p.push(format!("s2db_test_{}_{}", std::process::id(), n));
        p
    }

    const O: &str = "pluginA"; // a single owner for the same-context tests

    #[test]
    fn open_create_insert_select_roundtrip() {
        let dir = tmp();
        let h = open(&dir, "t1", O).unwrap();
        execute(h, "CREATE TABLE kv (k TEXT, v TEXT)", &[], O).unwrap();
        let r = execute(h, "INSERT INTO kv (k, v) VALUES (?, ?)",
            &[DbValue::Text("color".into()), DbValue::Text("red".into())], O).unwrap();
        assert_eq!(r.changes, 1);
        let q = query(h, "SELECT k, v FROM kv WHERE k = ?", &[DbValue::Text("color".into())], O).unwrap();
        assert_eq!(q.columns, vec!["k".to_string(), "v".to_string()]);
        assert_eq!(q.rows.len(), 1);
        match &q.rows[0][1] { DbValue::Text(s) => assert_eq!(s, "red"), _ => panic!("wrong type") }
        close(h, O);
    }

    #[test]
    fn bad_sql_errors_not_panics() {
        let h = open(&tmp(), "t2", O).unwrap();
        assert!(query(h, "SELECT * FROM nope", &[], O).is_err());
    }

    #[test]
    fn invalid_name_rejected() {
        assert!(open(&tmp(), "../evil", O).is_err());
        assert!(open(&tmp(), "a/b", O).is_err());
        assert!(open(&tmp(), "", O).is_err());
    }

    #[test]
    fn closed_handle_is_stale() {
        let h = open(&tmp(), "t3", O).unwrap();
        assert!(close(h, O));
        assert!(query(h, "SELECT 1", &[], O).is_err());
        assert!(!close(h, O)); // already gone
    }

    #[test]
    fn int_and_null_params_roundtrip() {
        let h = open(&tmp(), "t4", O).unwrap();
        execute(h, "CREATE TABLE n (a INTEGER, b REAL, c TEXT)", &[], O).unwrap();
        execute(h, "INSERT INTO n VALUES (?, ?, ?)",
            &[DbValue::Int(7), DbValue::Real(1.5), DbValue::Null], O).unwrap();
        let q = query(h, "SELECT a, b, c FROM n", &[], O).unwrap();
        match q.rows[0][0] { DbValue::Int(i) => assert_eq!(i, 7), _ => panic!() }
        match q.rows[0][1] { DbValue::Real(f) => assert_eq!(f, 1.5), _ => panic!() }
        match q.rows[0][2] { DbValue::Null => {}, _ => panic!() }
    }

    #[test]
    fn handle_scoped_to_owner() {
        let h = open(&tmp(), "owned", "pluginA").unwrap();
        execute(h, "CREATE TABLE t (x INTEGER)", &[], "pluginA").unwrap();
        // A different plugin cannot read, mutate, or close pluginA's handle.
        assert!(query(h, "SELECT * FROM t", &[], "pluginB").is_err());
        assert!(execute(h, "INSERT INTO t VALUES (1)", &[], "pluginB").is_err());
        assert!(!close(h, "pluginB"));
        // The owner still works, then closes it.
        assert!(query(h, "SELECT * FROM t", &[], "pluginA").is_ok());
        assert!(close(h, "pluginA"));
    }
}
