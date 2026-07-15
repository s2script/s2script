//! Engine-generic remote SQL (MySQL + Postgres) over the shared tokio runtime from http.rs. Holds
//! NO V8 handles. Pools live in a thread-local owner-scoped registry keyed by opaque integer handles
//! (a wrong owner reads "invalid db handle"); the main thread clones a pool + hands off the query to
//! the runtime, which sends a completion the frame drain resolves — the isolate thread never blocks.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;
use sqlx::{Column, Row, TypeInfo, ValueRef};
use sqlx::mysql::{MySqlArguments, MySqlPool, MySqlPoolOptions};
use sqlx::postgres::{PgArguments, PgPool, PgPoolOptions};
use sqlx::query::Query;
use crate::db::{DbValue, QueryResult, ExecResult, DbOutcome, DbCompletion};

#[derive(Clone)]
pub enum PoolKind { MySql(MySqlPool), Postgres(PgPool) }

thread_local! {
    // handle -> (pool, owner plugin id). Main-thread registry (the native clones the pool before
    // spawning); mirrors db.rs::CONNS ownership.
    static POOLS: RefCell<HashMap<u64, (PoolKind, String)>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = Cell::new(1);
}

/// Rewrite `?` placeholders to Postgres `$1..$n`, skipping any `?` inside a single-quoted literal.
pub fn pg_translate_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n = 0u32;
    let mut in_str = false;
    for c in sql.chars() {
        match c {
            '\'' => { in_str = !in_str; out.push(c); }
            '?' if !in_str => { n += 1; out.push('$'); out.push_str(&n.to_string()); }
            _ => out.push(c),
        }
    }
    out
}

/// A connection config parsed from databases.json (one entry). password is never logged.
struct RemoteConfig { driver: String, host: String, port: u16, user: String, password: String, database: String }
fn parse_config(json: &str) -> Result<RemoteConfig, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad db config: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let driver = s("driver");
    let default_port = if driver == "postgres" { 5432 } else { 3306 };
    let port = v.get("port").and_then(|x| x.as_u64()).unwrap_or(default_port) as u16;
    Ok(RemoteConfig { driver, host: s("host"), port, user: s("user"), password: s("password"), database: s("database") })
}

/// Build a lazy pool for `config_json` and register it under a fresh owner-scoped handle. Lazy: no I/O
/// here (a dead DB surfaces at query time); constructed inside the runtime context via http::enter().
pub fn connect(config_json: &str, owner: &str) -> Result<u64, String> {
    let cfg = parse_config(config_json)?;
    let _guard = crate::http::enter(); // ambient runtime for connect_lazy_with (may be None in a unit test)
    let pool = match cfg.driver.as_str() {
        "mysql" => {
            let opts = sqlx::mysql::MySqlConnectOptions::new()
                .host(&cfg.host).port(cfg.port).username(&cfg.user).password(&cfg.password).database(&cfg.database);
            PoolKind::MySql(MySqlPoolOptions::new().max_connections(4).acquire_timeout(Duration::from_secs(10)).connect_lazy_with(opts))
        }
        "postgres" => {
            let opts = sqlx::postgres::PgConnectOptions::new()
                .host(&cfg.host).port(cfg.port).username(&cfg.user).password(&cfg.password).database(&cfg.database);
            PoolKind::Postgres(PgPoolOptions::new().max_connections(4).acquire_timeout(Duration::from_secs(10)).connect_lazy_with(opts))
        }
        other => return Err(format!("unknown remote db driver: {other}")),
    };
    let handle = NEXT.with(|n| { let h = n.get(); n.set(h + 1); h });
    POOLS.with(|p| p.borrow_mut().insert(handle, (pool, owner.to_string())));
    Ok(handle)
}

/// Clone the pool for a handle the caller owns (a wrong owner is "invalid db handle", not probeable).
pub fn get_pool(handle: u64, owner: &str) -> Result<PoolKind, String> {
    POOLS.with(|p| match p.borrow().get(&handle) {
        Some((pool, o)) if o == owner => Ok(pool.clone()),
        _ => Err("invalid db handle".to_string()),
    })
}

pub fn close(handle: u64, owner: &str) -> bool {
    // Remove-if-owned in ONE borrow (returns the owned pool), THEN spawn the async close outside the
    // borrow — never a nested borrow_mut (that would panic "already borrowed").
    let pool = POOLS.with(|p| {
        let mut map = p.borrow_mut();
        match map.get(&handle) {
            Some((_, o)) if o == owner => map.remove(&handle).map(|(pool, _)| pool),
            _ => None,
        }
    });
    match pool {
        Some(pool) => { crate::http::spawn(async move { match pool { PoolKind::MySql(p) => p.close().await, PoolKind::Postgres(p) => p.close().await } }); true }
        None => false,
    }
}

/// Spawn a SELECT on the shared runtime; send a DbCompletion the frame drain resolves.
pub fn spawn_query(id: u64, pool: PoolKind, sql: String, params: Vec<DbValue>) {
    let tx = crate::db::completion_tx();
    crate::http::spawn(async move {
        let result = run_query(pool, sql, params).await.map(DbOutcome::Query);
        let _ = tx.send(DbCompletion { id, result });
    });
}
pub fn spawn_execute(id: u64, pool: PoolKind, sql: String, params: Vec<DbValue>) {
    let tx = crate::db::completion_tx();
    crate::http::spawn(async move {
        let result = run_execute(pool, sql, params).await.map(DbOutcome::Exec);
        let _ = tx.send(DbCompletion { id, result });
    });
}

// --- the async query/execute + decode/bind (per-backend; shares the DbValue mapping) ---
async fn run_query(pool: PoolKind, sql: String, params: Vec<DbValue>) -> Result<QueryResult, String> {
    match pool {
        PoolKind::MySql(p) => {
            let mut q = sqlx::query::<sqlx::MySql>(&sql);
            for v in &params { q = bind_mysql(q, v); }
            let rows = q.fetch_all(&p).await.map_err(|e| e.to_string())?;
            Ok(rows_to_result_mysql(rows))
        }
        PoolKind::Postgres(p) => {
            let sql = pg_translate_placeholders(&sql);
            let mut q = sqlx::query::<sqlx::Postgres>(&sql);
            for v in &params { q = bind_pg(q, v); }
            let rows = q.fetch_all(&p).await.map_err(|e| e.to_string())?;
            Ok(rows_to_result_pg(rows))
        }
    }
}
async fn run_execute(pool: PoolKind, sql: String, params: Vec<DbValue>) -> Result<ExecResult, String> {
    match pool {
        PoolKind::MySql(p) => {
            let mut q = sqlx::query::<sqlx::MySql>(&sql);
            for v in &params { q = bind_mysql(q, v); }
            let r = q.execute(&p).await.map_err(|e| e.to_string())?;
            Ok(ExecResult { changes: r.rows_affected() as i64, last_insert_id: r.last_insert_id() as i64 })
        }
        PoolKind::Postgres(p) => {
            let sql = pg_translate_placeholders(&sql);
            let mut q = sqlx::query::<sqlx::Postgres>(&sql);
            for v in &params { q = bind_pg(q, v); }
            let r = q.execute(&p).await.map_err(|e| e.to_string())?;
            Ok(ExecResult { changes: r.rows_affected() as i64, last_insert_id: 0 }) // PG has none; use RETURNING
        }
    }
}

// --- bind helpers (thread the rebound query; sqlx's Query has a backend-specific Arguments type) ---
// Owned values only (Int/Real are Copy; Text clones; Null binds a typed None) so nothing borrows the
// short-lived params vec across the await.
fn bind_mysql<'q>(q: Query<'q, sqlx::MySql, MySqlArguments>, v: &DbValue)
    -> Query<'q, sqlx::MySql, MySqlArguments>
{
    match v {
        DbValue::Int(i) => q.bind(*i),
        DbValue::Real(f) => q.bind(*f),
        DbValue::Text(s) => q.bind(s.clone()),
        DbValue::Null => q.bind(Option::<String>::None),
    }
}
fn bind_pg<'q>(q: Query<'q, sqlx::Postgres, PgArguments>, v: &DbValue)
    -> Query<'q, sqlx::Postgres, PgArguments>
{
    match v {
        DbValue::Int(i) => q.bind(*i),
        DbValue::Real(f) => q.bind(*f),
        DbValue::Text(s) => q.bind(s.clone()),
        DbValue::Null => q.bind(Option::<String>::None),
    }
}

// Decode one row of a backend row (`$row: &<DB>Row`) into a Vec<DbValue>. Dispatches on the column's
// SQL type name (uppercased) per the type→DbValue mapping; EVERY `try_get` is fallible → a decode
// miss (or an unexpected type) degrades to DbValue::Null, never a panic. Width fallbacks (i64→i32→…,
// f64→f32) make the common path succeed regardless of sqlx's strict per-width Type compatibility.
macro_rules! decode_row {
    ($row:expr) => {{
        let row = $row;
        let cols = row.columns();
        let mut vals: Vec<DbValue> = Vec::with_capacity(cols.len());
        for (i, col) in cols.iter().enumerate() {
            // NULL first (via the raw ValueRef) — a NULL cell is Null regardless of its declared type.
            let is_null = row.try_get_raw(i).map(|r| r.is_null()).unwrap_or(true);
            let v = if is_null {
                DbValue::Null
            } else {
                match col.type_info().name().to_ascii_uppercase().as_str() {
                    // small/medium integers → Int (widen from whatever fits)
                    "TINYINT" | "SMALLINT" | "INT" | "INT2" | "INT4" | "MEDIUMINT"
                    | "SERIAL" | "SMALLSERIAL" =>
                        row.try_get::<i64, _>(i)
                            .or_else(|_| row.try_get::<i32, _>(i).map(|n| n as i64))
                            .or_else(|_| row.try_get::<i16, _>(i).map(|n| n as i64))
                            .or_else(|_| row.try_get::<i8, _>(i).map(|n| n as i64))
                            .map(DbValue::Int)
                            .unwrap_or(DbValue::Null),
                    // 64-bit + exact-precision types → a DECIMAL STRING (no 2^53 loss; matches the
                    // framework's "64-bit as decimal string" convention). String first (works where
                    // the backend allows it), else i64→to_string.
                    "BIGINT" | "INT8" | "BIGSERIAL" | "DECIMAL" | "NUMERIC" | "BIT" =>
                        row.try_get::<String, _>(i)
                            .or_else(|_| row.try_get::<i64, _>(i).map(|n| n.to_string()))
                            .map(DbValue::Text)
                            .unwrap_or(DbValue::Null),
                    // floating point → Real
                    "FLOAT" | "DOUBLE" | "FLOAT4" | "FLOAT8" | "REAL" =>
                        row.try_get::<f64, _>(i)
                            .or_else(|_| row.try_get::<f32, _>(i).map(|f| f as f64))
                            .map(DbValue::Real)
                            .unwrap_or(DbValue::Null),
                    // booleans → Int(0|1) (DbValue has no Bool; the JS side reads it as a number)
                    "BOOL" | "BOOLEAN" =>
                        row.try_get::<bool, _>(i)
                            .map(|b| DbValue::Int(if b { 1 } else { 0 }))
                            .or_else(|_| row.try_get::<i64, _>(i).map(DbValue::Int))
                            .unwrap_or(DbValue::Null),
                    // strings
                    "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT"
                    | "BPCHAR" | "NAME" | "CITEXT" | "JSON" | "JSONB" | "UUID" | "ENUM" =>
                        row.try_get::<String, _>(i).map(DbValue::Text).unwrap_or(DbValue::Null),
                    // temporal → text (chrono/time features not enabled; try the string protocol,
                    // else Null — a documented per-decode degrade this slice)
                    "DATE" | "TIME" | "DATETIME" | "TIMESTAMP" | "TIMESTAMPTZ" | "TIMETZ" | "YEAR" =>
                        row.try_get::<String, _>(i).map(DbValue::Text).unwrap_or(DbValue::Null),
                    // anything else: best-effort text → int → real, else Null.
                    _ =>
                        row.try_get::<String, _>(i).map(DbValue::Text)
                            .or_else(|_| row.try_get::<i64, _>(i).map(DbValue::Int))
                            .or_else(|_| row.try_get::<f64, _>(i).map(DbValue::Real))
                            .unwrap_or(DbValue::Null),
                }
            };
            vals.push(v);
        }
        vals
    }};
}

fn rows_to_result_mysql(rows: Vec<sqlx::mysql::MySqlRow>) -> QueryResult {
    // fetch_all yields no metadata for a zero-row result, so column names come from the first row.
    let columns: Vec<String> = rows.first()
        .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
        .unwrap_or_default();
    let out: Vec<Vec<DbValue>> = rows.iter().map(|r| decode_row!(r)).collect();
    QueryResult { columns, rows: out }
}
fn rows_to_result_pg(rows: Vec<sqlx::postgres::PgRow>) -> QueryResult {
    let columns: Vec<String> = rows.first()
        .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
        .unwrap_or_default();
    let out: Vec<Vec<DbValue>> = rows.iter().map(|r| decode_row!(r)).collect();
    QueryResult { columns, rows: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pg_placeholder_translation() {
        assert_eq!(pg_translate_placeholders("SELECT * FROM t WHERE a=? AND b=?"),
                   "SELECT * FROM t WHERE a=$1 AND b=$2");
        assert_eq!(pg_translate_placeholders("SELECT 1"), "SELECT 1");           // none
        assert_eq!(pg_translate_placeholders("INSERT INTO t VALUES (?,?,?)"),
                   "INSERT INTO t VALUES ($1,$2,$3)");
        // a '?' inside a single-quoted string literal must NOT be renumbered
        assert_eq!(pg_translate_placeholders("SELECT '?' , a=?"), "SELECT '?' , a=$1");
    }
}
