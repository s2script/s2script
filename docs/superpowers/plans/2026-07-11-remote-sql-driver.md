# Remote SQL driver (MySQL + Postgres via sqlx) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Database.open("stats")` resolves — by an operator-owned `databases.json` — to a MySQL or Postgres connection, with the query running async off the game thread. Zero plugin-code change.

**Architecture:** A new core engine module `core/src/sqldb.rs` (mirrors `http.rs`/`ws.rs`: reuses the shared tokio runtime, owns a completion channel) holds sqlx pools behind opaque owner-scoped handles. New V8 natives (`__s2_db_remote_connect/query/execute/close`) mirror `s2_fetch`/`s2_sqlite_open`; remote query/execute resolve on a new `frame_async_drain` loop via `resolve_db`. The `@s2script/db` prelude gains a `databases.json` resolver + auto-registered `mysql`/`postgres` built-in drivers behind the existing `Driver` seam.

**Tech Stack:** Rust (`sqlx` on the existing `tokio` runtime), JavaScript (the `@s2script/db` prelude in `core/src/v8host.rs`), Docker (mysql/postgres live-gate sidecars).

**Design:** `docs/superpowers/specs/2026-07-11-remote-sql-driver-design.md`

## Global Constraints

- **Boundary:** `sqldb.rs` + the natives are engine-generic (connection config + SQL strings; no CS2/game symbol). `@s2script/db` is an engine-generic prelude module. **No shim change, no new `S2EngineOps` op** (natives `set_native`'d). One sniper rebuild (core `.so`; adds sqlx). Both boundary gates stay green (`scripts/check-core-boundary.sh`).
- **No `packages/*` change** — this slots behind the existing `Database`/`Driver` API + `SqlValue` union. Local-merge cadence (not a PR/changeset).
- **Async off-thread:** remote query/execute NEVER block the isolate thread — they hand off to the tokio runtime and resolve on a later `frame_async_drain` (the `s2_fetch` pattern). Local SQLite stays sync-behind-Promise (unchanged).
- **Degrade-never-crash:** every native `catch_unwind`s; a connect/query/SQL error REJECTS the Promise; malformed `databases.json` → WARN + all-sqlite fallback; a query outstanding when its plugin unloads DROPS (owner-liveness guard), never resolves into a dead context.
- **Safety:** owner-scoped opaque integer handles (query/execute/close verify the caller owns the handle — the `db.rs::get_owned` discipline); ledgered `Resource::RemoteDbConn` → pool closed on teardown.
- **Type mapping → `SqlValue` (`string|number|boolean|null`):** int-fits-f64 → number; BIGINT/DECIMAL/NUMERIC → decimal **string**; float → number; bool → boolean; text → string; DATE/TIMESTAMP → ISO string; NULL → null; BLOB/bytea → null (deferred). Postgres: rewrite `?`→`$1..$n`.
- Core tests run serial (`RUST_TEST_THREADS=1`).

---

## File Structure

- **Create** `core/src/sqldb.rs` — the sqlx engine: `PoolKind`, an owner-scoped pool registry (thread-local, like `db.rs::CONNS`), a process-global completion channel (`OnceLock`, like `http.rs::ENGINE`), `connect`/`get_pool`/`close` (main-thread registry ops), `spawn_query`/`spawn_execute` (spawn on the shared runtime + send a completion), `try_recv_completed`, and the pure `pg_translate_placeholders` + type-decode/bind helpers. Reuses `crate::db::{DbValue, QueryResult, ExecResult}`.
- **Modify** `core/Cargo.toml` — add `sqlx`.
- **Modify** `core/src/lib.rs` (or wherever modules are declared) — `mod sqldb;`.
- **Modify** `core/src/http.rs` — add `pub fn enter() -> Option<tokio::runtime::EnterGuard<'static>>` so `sqldb::connect` builds a pool inside the runtime context.
- **Modify** `core/src/plugin.rs` — `Resource::RemoteDbConn(u64)` + `record_remote_db_conn`.
- **Modify** `core/src/v8host.rs` — the 4 natives + `resolve_db` + the `frame_async_drain` sqldb loop + register the natives + the `RemoteDbConn` teardown arm + a shared `query_result_to_js` builder + the `@s2script/db` prelude changes.
- **Modify** `docker/docker-compose.yml` — `mysql` + `postgres` sidecars.
- **Create** `examples/db-remote-demo/` — the live-gate demo.

---

## Task 1: `core/src/sqldb.rs` — the sqlx engine module

**Files:**
- Create: `core/src/sqldb.rs`
- Modify: `core/Cargo.toml` (add sqlx), `core/src/lib.rs` (`mod sqldb;`), `core/src/http.rs` (add `enter()`)

**Interfaces:**
- Consumes: `crate::db::{DbValue, QueryResult, ExecResult}`; `crate::http::{spawn, enter}`.
- Produces: `PoolKind`; `connect(config_json, owner) -> Result<u64,String>`; `get_pool(handle, owner) -> Result<PoolKind,String>` (clones); `close(handle, owner) -> bool`; `spawn_query(id, pool, sql, params)`; `spawn_execute(id, pool, sql, params)`; `try_recv_completed() -> Option<DbCompletion>`; `pg_translate_placeholders(sql) -> String`. `DbCompletion { id, result: DbOutcome }`, `DbOutcome = Query(QueryResult) | Exec(ExecResult)` (or `Result<_,String>`).

- [ ] **Step 1: Add sqlx to `core/Cargo.toml`** (after the tokio-tungstenite line). NOTE — sqlx 0.8 splits runtime/TLS features; if these exact names fail to resolve, `cargo` prints the valid set — use `runtime-tokio` + `tls-rustls`. Do NOT enable the `macros` feature (that needs a compile-time `DATABASE_URL`; we use runtime `sqlx::query`).

```toml
# Engine-generic remote SQL subsystem (core/src/sqldb.rs): MySQL + Postgres over the shared tokio
# runtime from http.rs. rustls-only (matches reqwest/tungstenite). Runtime queries only (no `macros`).
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "mysql", "postgres"] }
```

- [ ] **Step 2: Add `mod sqldb;`** beside the other `mod` lines in `core/src/lib.rs` (find `mod db;` / `mod http;` / `mod ws;` and add `mod sqldb;`).

- [ ] **Step 3: Add `http::enter()`** to `core/src/http.rs` (so `sqldb::connect` constructs a pool with an ambient runtime):

```rust
/// Enter the shared runtime's context (an RAII guard) so runtime-requiring constructors (e.g. a sqlx
/// `connect_lazy_with`) can be called from the main thread without blocking. None if uninitialized.
pub fn enter() -> Option<tokio::runtime::EnterGuard<'static>> {
    ENGINE.get().map(|e| e.runtime.enter())
}
```

- [ ] **Step 4: Write the failing test** — the pure Postgres placeholder translation (create `core/src/sqldb.rs` with just this + the fn stub):

```rust
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
```

- [ ] **Step 5: Run — expect FAIL** (`pg_translate_placeholders` undefined).

Run: `cd core && cargo test pg_placeholder_translation`
Expected: FAIL (compile error / unresolved).

- [ ] **Step 6: Write `core/src/sqldb.rs`** (the full module). The placeholder translator skips `?` inside `'…'` string literals:

```rust
//! Engine-generic remote SQL (MySQL + Postgres) over the shared tokio runtime from http.rs. Holds
//! NO V8 handles. Pools live in a thread-local owner-scoped registry keyed by opaque integer handles
//! (a wrong owner reads "invalid db handle"); the main thread clones a pool + hands off the query to
//! the runtime, which sends a completion the frame drain resolves — the isolate thread never blocks.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use sqlx::{MySqlPool, PgPool, Column, Row, TypeInfo, ValueRef};
use sqlx::mysql::MySqlPoolOptions;
use sqlx::postgres::PgPoolOptions;
use crate::db::{DbValue, QueryResult, ExecResult};

#[derive(Clone)]
pub enum PoolKind { MySql(MySqlPool), Postgres(PgPool) }

pub enum DbOutcome { Query(QueryResult), Exec(ExecResult) }
pub struct DbCompletion { pub id: u64, pub result: Result<DbOutcome, String> }

thread_local! {
    // handle -> (pool, owner plugin id). Main-thread registry (the native clones the pool before
    // spawning); mirrors db.rs::CONNS ownership.
    static POOLS: RefCell<HashMap<u64, (PoolKind, String)>> = RefCell::new(HashMap::new());
    static NEXT: Cell<u64> = Cell::new(1);
}

// Process-global completion channel (like http.rs::ENGINE). The runtime tasks send here; the frame
// drain polls try_recv_completed().
struct Chan { tx: Sender<DbCompletion>, rx: Mutex<Receiver<DbCompletion>> }
static CHAN: OnceLock<Chan> = OnceLock::new();
fn chan() -> &'static Chan {
    CHAN.get_or_init(|| { let (tx, rx) = channel(); Chan { tx, rx: Mutex::new(rx) } })
}
pub fn try_recv_completed() -> Option<DbCompletion> { chan().rx.lock().ok()?.try_recv().ok() }

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
    let tx = chan().tx.clone();
    crate::http::spawn(async move {
        let result = run_query(pool, sql, params).await.map(DbOutcome::Query);
        let _ = tx.send(DbCompletion { id, result });
    });
}
pub fn spawn_execute(id: u64, pool: PoolKind, sql: String, params: Vec<DbValue>) {
    let tx = chan().tx.clone();
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
```

IMPLEMENTER NOTE for the bind/decode helpers (`bind_mysql`/`bind_pg`/`rows_to_result_mysql`/`rows_to_result_pg`): sqlx's `Query` type has backend-specific generics, so the `bind_*` helpers return the rebound query (thread it as shown). Bind mapping: `DbValue::Int(i)`→`.bind(i)`, `Real(f)`→`.bind(f)`, `Text(s)`→`.bind(s.clone())`, `Null`→`.bind(Option::<String>::None)`. Decode mapping — per column, dispatch on `col.type_info().name()` (uppercase): `TINYINT/SMALLINT/INT/INT2/INT4/MEDIUMINT`→ try `i64` → `DbValue::Int`; `BIGINT/INT8/DECIMAL/NUMERIC/BIT`→ decode to a **string** (`try_get::<String,_>` where the backend allows, else `try_get::<i64,_>().map(|n| n.to_string())`) → `DbValue::Text`; `FLOAT/DOUBLE/FLOAT4/FLOAT8/REAL`→ `f64` → `DbValue::Real`; `BOOL/BOOLEAN/TINYINT(1)`→ bool → `DbValue::Int(0|1)` (SqlValue has no bool distinct from the sqlite convention; the JS side maps Int(0|1) — OR add a `DbValue::Bool` if you prefer a real boolean: extend the JS `query_result_to_js` builder accordingly); `CHAR/VARCHAR/TEXT/*`→ `String`→`DbValue::Text`; `DATE/TIME/DATETIME/TIMESTAMP*`→ `String` (chrono not enabled → try `try_get::<String,_>`, else format via the DB's text protocol) → `DbValue::Text`; NULL (via `row.try_get_raw(i)?.is_null()`) → `DbValue::Null`; anything else → `try_get::<String,_>` fallback → `DbValue::Text`, and on decode failure → `DbValue::Null`. Keep every `try_get` fallible → a decode miss becomes `Null`, never a panic.

- [ ] **Step 7: Run the placeholder test — expect PASS** (the rest compiles).

Run: `cd core && cargo test pg_placeholder_translation`
Expected: PASS. Also `cargo build` succeeds (sqlx resolves; adjust the feature names per Step 1's note if not).

- [ ] **Step 8: Commit.**

```bash
git add core/src/sqldb.rs core/Cargo.toml core/src/lib.rs core/src/http.rs
git commit -m "feat(db): core sqlx engine (mysql+postgres pools, async, off-thread)"
```

---

## Task 2: V8 natives + async resolve + ledger

**Files:**
- Modify: `core/src/plugin.rs` (`Resource::RemoteDbConn` + `record_remote_db_conn`)
- Modify: `core/src/v8host.rs` (4 natives, `resolve_db`, the drain loop, registration, teardown arm, `query_result_to_js`)

**Interfaces:**
- Consumes: Task 1's `sqldb::{connect, get_pool, close, spawn_query, spawn_execute, try_recv_completed, DbOutcome, DbCompletion}`; the `s2_fetch` spine (`RESOLVERS`, `record_job`, `PENDING_JOBS`, `resolver_owner_tag`, `next_async_id`, `refresh_detour`, `current_plugin`).
- Produces (JS natives): `__s2_db_remote_connect(configJson) -> number` (handle, 0 on failure); `__s2_db_remote_query(handle, sql, params) -> Promise<Row[]>`; `__s2_db_remote_execute(handle, sql, params) -> Promise<{changes,lastInsertId}>`; `__s2_db_remote_close(handle) -> Promise<void>`.

- [ ] **Step 1: Add the ledger resource.** In `core/src/plugin.rs`, beside `DbConn(u64)` (line ~27) add `RemoteDbConn(u64)`, and beside `record_db_conn` add:

```rust
pub fn record_remote_db_conn(&mut self, handle: u64) { self.order.push(Resource::RemoteDbConn(handle)); }
```

- [ ] **Step 2: Add the teardown arm.** In `core/src/v8host.rs`, in the ledger-teardown match (beside the `Resource::DbConn(h)` arm ~7455):

```rust
                plugin::Resource::RemoteDbConn(h) => {
                    // Late/never close() — teardown drops the pool now (idempotent; a wrong/absent
                    // handle is a harmless no-op inside sqldb::close). Passes the unloading plugin's
                    // own id (it owns every handle in its ledger).
                    crate::sqldb::close(h, id);
                }
```

- [ ] **Step 3: Extract a shared `query_result_to_js` builder.** Refactor the JS-array building out of `s2_sqlite_query` into a fn both it and `resolve_db` use (find `s2_sqlite_query`'s row-building loop):

```rust
/// Build the JS `Row[]` (array of {col: value}) from a QueryResult. Shared by the sync SQLite path
/// and the async remote-resolve path. DbValue → JS: Int→Number, Real→Number, Text→String, Null→null.
fn query_result_to_js<'s>(scope: &mut v8::HandleScope<'s>, q: &crate::db::QueryResult) -> v8::Local<'s, v8::Value> {
    let arr = v8::Array::new(scope, q.rows.len() as i32);
    for (ri, row) in q.rows.iter().enumerate() {
        let obj = v8::Object::new(scope);
        for (ci, col) in q.columns.iter().enumerate() {
            let key = v8::String::new(scope, col).unwrap();
            let val: v8::Local<v8::Value> = match &row[ci] {
                crate::db::DbValue::Null => v8::null(scope).into(),
                crate::db::DbValue::Int(n) => v8::Number::new(scope, *n as f64).into(),
                crate::db::DbValue::Real(f) => v8::Number::new(scope, *f).into(),
                crate::db::DbValue::Text(s) => v8::String::new(scope, s).unwrap().into(),
            };
            obj.set(scope, key.into(), val);
        }
        arr.set_index(scope, ri as u32, obj.into());
    }
    arr.into()
}
```
(Then rewrite `s2_sqlite_query`'s builder to `let result = query_result_to_js(scope, &qr); resolver.resolve(scope, result);`.)

- [ ] **Step 4: Add the four natives** (mirroring the exact functions cited). `s2_db_remote_connect` mirrors `s2_sqlite_open` (v8host.rs:~begin) — **synchronous**, ledgers via `record_remote_db_conn`, returns the handle as a Number (0 on failure, no throw):

```rust
/// `__s2_db_remote_connect(configJson) -> number` — build+register a lazy pool; 0 on failure. Ledgers
/// the connection against the caller (RemoteDbConn) so an unclosed pool is dropped at teardown.
fn s2_db_remote_connect(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let cfg = args.get(0).to_rust_string_lossy(scope);
        let owner = current_plugin(scope).unwrap_or_default();
        match crate::sqldb::connect(&cfg, &owner) {
            Ok(handle) => {
                if let Some((ref oid, _)) = resolver_owner_tag(scope) {
                    REGISTRY.with(|r| { if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_remote_db_conn(handle); } });
                }
                rv.set(v8::Number::new(scope, handle as f64).into());
            }
            Err(_e) => rv.set(v8::Number::new(scope, 0.0).into()),
        }
    }));
}
```

`s2_db_remote_query` / `s2_db_remote_execute` mirror **`s2_fetch` (v8host.rs:2217)** exactly for the resolver/owner/ledger(`record_job`)/`RESOLVERS`/`PENDING_JOBS`/`refresh_detour`/return-promise block, substituting the hand-off: parse `handle` (arg0), `sql` (arg1), `params` (arg2 → `Vec<DbValue>` via a small `js_params_to_dbvalues(scope, arg)` helper: Number→Int if integral else Real, String→Text, Boolean→Int(0|1), Null/undefined→Null), then `let pool = match sqldb::get_pool(handle, &owner) { Ok(p)=>p, Err(e)=> { reject the resolver with Error(e) synchronously + return } };` then `sqldb::spawn_query(id, pool, sql, params)` (or `spawn_execute`). (An invalid handle rejects the returned Promise immediately — build the resolver, `resolver.reject(...)`, still `rv.set(promise)`, and do NOT touch RESOLVERS/PENDING_JOBS for that early-reject path.)

`s2_db_remote_close` mirrors `s2_sqlite_close` (returns a resolved `Promise<void>` after `sqldb::close(handle, owner)`).

- [ ] **Step 5: Add `resolve_db`** (mirrors `resolve_fetch` v8host.rs:7102 / `resolve_ws_connect` — same owner-liveness + context-clone + HandleScope/ContextScope preamble; resolves with the rows array or the exec object, rejects with an `Error` on `Err`):

```rust
/// Resolve (or drop, on the async-liveness guard) a completed remote DB query/execute in its OWNING
/// plugin's context. MIRRORS resolve_fetch's owner-liveness/context-clone/scope preamble.
fn resolve_db(host: &mut Host, entry: &ResolverEntry, result: Result<crate::sqldb::DbOutcome, String>) {
    // [same preamble as resolve_fetch: resolve g_ctx from entry.owner via REGISTRY.is_live + PLUGINS,
    //  return (DROP) if not live; open HandleScope + ContextScope on g_ctx; Global->Local the resolver]
    // then:
    match result {
        Ok(crate::sqldb::DbOutcome::Query(qr)) => { let v = query_result_to_js(scope, &qr); resolver.resolve(scope, v); }
        Ok(crate::sqldb::DbOutcome::Exec(er)) => {
            let obj = v8::Object::new(scope);
            let k1 = v8::String::new(scope, "changes").unwrap(); let v1 = v8::Number::new(scope, er.changes as f64);
            let k2 = v8::String::new(scope, "lastInsertId").unwrap(); let v2 = v8::Number::new(scope, er.last_insert_id as f64);
            obj.set(scope, k1.into(), v1.into()); obj.set(scope, k2.into(), v2.into());
            resolver.resolve(scope, obj.into());
        }
        Err(e) => { let msg = v8::String::new(scope, &e).unwrap(); let ex = v8::Exception::error(scope, msg); resolver.reject(scope, ex); }
    }
}
```

- [ ] **Step 6: Add the drain loop.** In `frame_async_drain`, after the http-completion `while let` loop (mirrors it exactly):

```rust
        // Remote SQL completions (core/src/sqldb.rs). Mirrors the http loop: pop a completion, remove
        // its RESOLVERS entry, decrement PENDING_JOBS, resolve/reject (or DROP on the liveness guard).
        while let Some(c) = crate::sqldb::try_recv_completed() {
            let Some(entry) = RESOLVERS.with(|m| m.borrow_mut().remove(&c.id)) else { continue };
            PENDING_JOBS.with(|cnt| cnt.set(cnt.get().saturating_sub(1)));
            resolve_db(host, &entry, c.result);
        }
```

- [ ] **Step 7: Register the natives** (beside `__s2_sqlite_*` ~5511):

```rust
    set_native(scope, global_obj, "__s2_db_remote_connect", s2_db_remote_connect);
    set_native(scope, global_obj, "__s2_db_remote_query", s2_db_remote_query);
    set_native(scope, global_obj, "__s2_db_remote_execute", s2_db_remote_execute);
    set_native(scope, global_obj, "__s2_db_remote_close", s2_db_remote_close);
```

- [ ] **Step 8: Build + run the full suite.**

Run: `cd core && cargo test`
Expected: PASS (existing suite + Task 1's placeholder test; the natives compile). No live DB is exercised here.

- [ ] **Step 9: Commit.**

```bash
git add core/src/plugin.rs core/src/v8host.rs
git commit -m "feat(db): remote-db V8 natives + async resolve + RemoteDbConn ledger"
```

---

## Task 3: `@s2script/db` prelude — `databases.json` resolver + mysql/postgres drivers

**Files:**
- Modify: `core/src/v8host.rs` (the `@s2script/db` module block, ~1711–1747)
- Test: `core/src/v8host.rs` (a db-resolver in-isolate test)

**Interfaces:**
- Consumes: Task 2 natives; `__s2_config_read_raw`/`__s2_config_write_raw`.
- Produces (JS): `Database.open` routes a configured name to the mysql/postgres driver; a `databases.json` template auto-generates; test hook `globalThis.__s2_db_resolveConfig(name)`.

- [ ] **Step 1: Write the failing test** (in the db test area — inject a config + a fake driver, assert routing):

```rust
    #[test]
    fn db_open_routes_by_config() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        // seed the per-context config directly (bypass the config bridge, unavailable in tests) + a fake driver
        eval_in_context("p", "\
            globalThis.__s2_db_config = { stats: { driver:'mysql', name:'stats', host:'h' } };\
            var seen=null;\
            __s2pkg_db.Database.registerDriver({ name:'mysql', connect:function(c){ seen=c; return Promise.resolve({query:function(){},execute:function(){},close:function(){}});} });\
            __s2pkg_db.Database.open('stats');\
            globalThis.__test_seen_driver = seen ? seen.driver : 'none';\
        ").unwrap();
        assert_eq!(eval_in_context_string("p", "globalThis.__test_seen_driver"), "mysql");
        // an UNconfigured name falls back to sqlite
        assert_eq!(eval_in_context_string("p", "__s2_db_resolveConfig('whatever').driver"), "sqlite");
        // a configured name resolves to its driver
        assert_eq!(eval_in_context_string("p", "__s2_db_resolveConfig('stats').driver"), "mysql");
        shutdown();
    }
```

- [ ] **Step 2: Run — expect FAIL** (`__s2_db_resolveConfig` undefined; open ignores config).

Run: `cd core && cargo test db_open_routes_by_config`
Expected: FAIL.

- [ ] **Step 3: Rewrite the `@s2script/db` module block** (replace ~1711–1747) — add the config read, the resolver, and the mysql/postgres built-in drivers:

```javascript
  // --- @s2script/db — Database.open/query/execute/close over the built-in drivers. SQLite is
  //     sync-behind-Promise (__s2_sqlite_*); mysql/postgres are async off-thread (__s2_db_remote_*).
  //     Database.open resolves a name via databases.json (operator-owned; absent name -> sqlite). ---
  var __s2_db_drivers = {};
  var __s2_db_config = {};   // name -> { driver, host, port, user, password, database } (from databases.json)
  function __s2_db_loadConfig() {
    var text = __s2_config_read_raw("databases");
    if (text == null) {
      __s2_config_write_raw("databases", '{\n  "_help": "connection name -> { driver: \\"mysql\\"|\\"postgres\\", host, port, user, password, database }. Names not listed here default to a local SQLite file. e.g. \\"stats\\": { \\"driver\\": \\"mysql\\", \\"host\\": \\"db\\", \\"port\\": 3306, \\"user\\": \\"cs2\\", \\"password\\": \\"...\\", \\"database\\": \\"stats\\" }"\n}\n');
      return;
    }
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: databases.json malformed — all connections default to sqlite"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var name in obj) {
      if (name === "_help" || !Object.prototype.hasOwnProperty.call(obj, name)) continue;
      var c = obj[name];
      if (c && typeof c === "object" && (c.driver === "mysql" || c.driver === "postgres")) __s2_db_config[name] = c;
    }
  }
  function __s2_db_resolveConfig(connName) {
    var c = __s2_db_config[connName];
    if (c) { return { driver: c.driver, name: connName, host: c.host, port: c.port, user: c.user, password: c.password, database: c.database }; }
    return { driver: "sqlite", name: connName };
  }
  globalThis.__s2_db_resolveConfig = __s2_db_resolveConfig;   // test hook

  __s2_db_drivers["sqlite"] = {
    name: "sqlite",
    connect: function (config) {
      return __s2_sqlite_open(config.name).then(function (handle) {
        return { query: function (s, p) { return __s2_sqlite_query(handle, s, p || []); },
                 execute: function (s, p) { return __s2_sqlite_execute(handle, s, p || []); },
                 close: function () { return __s2_sqlite_close(handle); } };
      });
    },
  };
  function __s2_makeRemoteDriver(driverName) {
    return {
      name: driverName,
      connect: function (config) {
        var handle = __s2_db_remote_connect(JSON.stringify(config));
        if (!handle) return Promise.reject(new Error("could not open " + driverName + " connection '" + config.name + "'"));
        return Promise.resolve({
          query:   function (s, p) { return __s2_db_remote_query(handle, s, p || []); },
          execute: function (s, p) { return __s2_db_remote_execute(handle, s, p || []); },
          close:   function () { return __s2_db_remote_close(handle); },
        });
      },
    };
  }
  __s2_db_drivers["mysql"] = __s2_makeRemoteDriver("mysql");
  __s2_db_drivers["postgres"] = __s2_makeRemoteDriver("postgres");

  __s2_db_loadConfig();

  var __s2_Database = {
    registerDriver: function (driver) { __s2_db_drivers[driver.name] = driver; },
    open: function (name) {
      var connName = name || "default";
      var config = __s2_db_resolveConfig(connName);
      var driver = __s2_db_drivers[config.driver];
      if (!driver) return Promise.reject(new Error("unknown db driver: " + config.driver));
      return driver.connect(config).then(function (conn) {
        return { query: function (s, p) { return conn.query(s, p); },
                 execute: function (s, p) { return conn.execute(s, p); },
                 close: function () { return conn.close(); } };
      });
    },
  };
  globalThis.__s2pkg_db = { Database: __s2_Database };
```

- [ ] **Step 4: Run tests — expect PASS.**

Run: `cd core && cargo test db_open_routes_by_config && cd .. && bash scripts/check-plugins-typecheck.sh`
Expected: core PASS; typecheck green (no `.d.ts` change — the API is identical).

- [ ] **Step 5: Commit.**

```bash
git add core/src/v8host.rs
git commit -m "feat(db): databases.json resolver + mysql/postgres built-in drivers"
```

---

## Task 4: docker sidecars, demo, sniper build, live gate

**Files:**
- Modify: `docker/docker-compose.yml` (mysql + postgres sidecars)
- Create: `examples/db-remote-demo/{package.json,tsconfig.json,src/plugin.ts}`

**Interfaces:**
- Consumes: `@s2script/db` (`Database.open/query/execute/close`), `@s2script/frame` (`OnGameFrame`) for the non-blocking proof.

- [ ] **Step 1: Add the DB sidecars** to `docker/docker-compose.yml` (under `services:`, same default network as `cs2` so they're reachable by service name):

```yaml
  mysql:
    image: mysql:8
    container_name: s2script-mysql
    environment:
      MYSQL_ROOT_PASSWORD: s2root
      MYSQL_DATABASE: stats
      MYSQL_USER: cs2
      MYSQL_PASSWORD: s2pass
    command: --default-authentication-plugin=caching_sha2_password
  postgres:
    image: postgres:16
    container_name: s2script-postgres
    environment:
      POSTGRES_DB: prefs
      POSTGRES_USER: cs2
      POSTGRES_PASSWORD: s2pass
```

- [ ] **Step 2: Write the demo** `examples/db-remote-demo/src/plugin.ts` (mirror `examples/clients-demo` package.json/tsconfig: `@demo/db-remote-demo`, minimal `s2script.apiVersion "1.x"`, tsconfig extends `../../tsconfig.base.json` + globals):

```typescript
// db-remote-demo — opens the operator-configured "stats" (mysql) + "prefs" (postgres) connections,
// round-trips CREATE/INSERT/SELECT against each, checks a BIGINT reads back as a decimal string, and
// proves the game frame advances WHILE a query is in flight (async, off-thread).
import { Database } from "@s2script/db";
import { OnGameFrame } from "@s2script/frame";

let frames = 0;
OnGameFrame.subscribe(() => { frames++; });

async function exercise(name: string, autoInc: string): Promise<void> {
  try {
    const db = await Database.open(name);
    await db.execute(`CREATE TABLE IF NOT EXISTS demo (id ${autoInc}, sid BIGINT, note TEXT)`);
    const before = frames;
    await db.execute("INSERT INTO demo (sid, note) VALUES (?, ?)", ["76561199000000001", "hello from " + name]);
    const rows = await db.query("SELECT sid, note FROM demo ORDER BY id DESC LIMIT 1");
    const sid = rows.length ? rows[0].sid : null;
    console.log(`[db-remote-demo] ${name}: rows=${rows.length} sid=${JSON.stringify(sid)} typeof=${typeof sid} frames+=${frames - before}`);
    await db.close();
  } catch (e) {
    console.log(`[db-remote-demo] ${name}: ERROR ${e}`);
  }
}

export function onLoad(): void {
  console.log("[db-remote-demo] onLoad — exercising mysql + postgres");
  exercise("stats", "INT AUTO_INCREMENT PRIMARY KEY");        // mysql
  exercise("prefs", "SERIAL PRIMARY KEY");                    // postgres
}
```

- [ ] **Step 3: Typecheck + core suite + sniper build.**

```bash
(cd examples/db-remote-demo && true) ; bash scripts/check-plugins-typecheck.sh && (cd core && cargo test) && \
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: typecheck green; core PASS; sniper build OK (compiles sqlx; GLIBC floors met).

- [ ] **Step 4: Deploy + seed `databases.json` + start.** (build-sniper wipes `dist/addons/s2script`.)

```bash
bash scripts/build-base-plugins.sh
node packages/cli/dist/cli.js build examples/db-remote-demo
mkdir -p dist/addons/s2script/configs dist/addons/s2script/data && chmod 777 dist/addons/s2script/configs dist/addons/s2script/data
cp plugins/*/dist/_s2script_*.s2sp examples/db-remote-demo/dist/*.s2sp dist/addons/s2script/plugins/
rm -f dist/addons/s2script/plugins/_s2script_zones-lib.s2sp
printf '%s\n' '{
  "stats": { "driver":"mysql",    "host":"mysql",    "port":3306, "user":"cs2", "password":"s2pass", "database":"stats" },
  "prefs": { "driver":"postgres", "host":"postgres", "port":5432, "user":"cs2", "password":"s2pass", "database":"prefs" }
}' > dist/addons/s2script/configs/databases.json
chmod 666 dist/addons/s2script/configs/databases.json
(cd docker && docker compose up -d)
```

- [ ] **Step 5: Verify the live gate** (after the boot window). Expect in `docker logs s2script-cs2`:
  - `[db-remote-demo] stats: rows=1 sid="76561199000000001" typeof=string frames+=<N>` — **N > 0** proves the frame advanced during the query (non-blocking), and `typeof=string` proves BIGINT→decimal string.
  - `[db-remote-demo] prefs: rows=1 sid="76561199000000001" typeof=string frames+=<N>` (postgres, `?`→`$n` worked).
  - `GAMEDATA VALIDATION: <n> ok, 0 FAILED`; `RestartCount=0`; no panic.

- [ ] **Step 6: Commit.**

```bash
git add docker/docker-compose.yml examples/db-remote-demo docs/superpowers/plans/2026-07-11-remote-sql-driver.md
git commit -m "feat(db): mysql/postgres live-gate sidecars + db-remote-demo"
```

---

## Deferred (do NOT build ahead)

- Transactions (pinned-session BEGIN/COMMIT); inline (non-config) connections; blobs/bytea; migrations; prepared-statement reuse; pool-tuning config knobs; moving SQLite to the async path; `@s2script/net`.
