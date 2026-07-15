# Off-thread SQLite (rusqlite connection-actor) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move SQLite `query`/`execute` off the game thread so a DB write (e.g. surftimer's finish-zone `INSERT`) no longer stalls a frame, without changing the `@s2script/db` API.

**Architecture:** Each SQLite connection gets a dedicated actor thread that owns the rusqlite `Connection` and processes `query`/`execute` commands from a FIFO channel; the game thread only hands off a command and returns. Completions flow through the *same* process-global channel + `resolve_db` + frame-drain loop the remote sqlx driver already uses (owner-liveness guard, ledger teardown, `PENDING_JOBS`), so SQLite inherits the async-result spine.

**Tech Stack:** Rust (`core/src/db.rs`, `sqldb.rs`, `v8host.rs`), `std::thread` + `std::sync::mpsc` (no new crates, no tokio), rusqlite 0.31 (bundled), rusty_v8 149.4.0.

## Global Constraints

- **No new crate deps.** Actor is pure `std::thread` + `std::sync::mpsc`; `http.rs` (tokio) untouched.
- **No API change.** `@s2script/db` (`open`/`query`/`execute`/`close`) stays Promise-returning, identical signatures. No plugin, no `packages/*`, no `.d.ts` change → **local-merge slice, no changeset**.
- **Core-only.** No shim change, no `S2EngineOps` op, no ABI change → core-only sniper rebuild.
- **Engine-generic.** `db.rs` knows nothing about any game; `check-core-boundary.sh` must stay green.
- **Degrade-never-crash.** Every native runs under `catch_unwind`; the actor wraps each statement in `catch_unwind` (a panicking statement → `Err` completion, never a dead actor); a dead-actor send → immediate Promise rejection with no `PENDING_JOBS`/`RESOLVERS` leak.
- **Owner-scoping preserved.** Opaque integer handles; a wrong owner reads "invalid db handle" (not probeable), enforced synchronously on the game thread before any command is sent.
- **FIFO per connection.** One actor thread + one ordered channel = submission-order execution (matches today's synchronous ordering).
- **Tests run serial.** `.cargo/config.toml` sets `RUST_TEST_THREADS = "1"`; the process-global completion channel relies on it.

## File Structure

- `core/src/db.rs` — the connection-actor: `Command` enum, actor loop, `open` (eager main-thread open + spawn actor + `busy_timeout`), `submit_query`/`submit_execute` (owner-checked + send), `close` (send `Shutdown`); **owns** the shared completion channel + `DbCompletion`/`DbOutcome` (moved here from `sqldb.rs`) + `completion_tx`/`try_recv_completed`; the pure `run_query`/`run_execute` helpers (the current query/execute bodies). Tests rewritten to drive the actor.
- `core/src/sqldb.rs` — imports the completion types/channel from `db.rs` instead of defining them; `spawn_query`/`spawn_execute` send via `crate::db::completion_tx()`.
- `core/src/v8host.rs` — `s2_sqlite_query`/`execute` natives go async (mirror the remote natives); `resolve_db` and the drain loop switch their `crate::sqldb::` completion references to `crate::db::`.

---

## Task 1: Move the shared DB completion plumbing into `db.rs`

Pure move/rename — **no behavior change**. SQLite stays synchronous; the remote driver keeps working. This isolates the "unify the completion channel" refactor into one green, reviewable step so Task 2 is purely the actor switch.

**Files:**
- Modify: `core/src/db.rs` (add the completion types + channel + `completion_tx`/`try_recv_completed`)
- Modify: `core/src/sqldb.rs:16-36,112-125` (delete the moved definitions; import from `db.rs`; send via `crate::db::completion_tx()`)
- Modify: `core/src/v8host.rs` (`resolve_db` signature/matches + the drain loop reference: `crate::sqldb::` → `crate::db::`)

**Interfaces:**
- Produces (in `db.rs`):
  - `pub(crate) enum DbOutcome { Query(QueryResult), Exec(ExecResult) }`
  - `pub(crate) struct DbCompletion { pub id: u64, pub result: Result<DbOutcome, String> }`
  - `pub(crate) fn completion_tx() -> std::sync::mpsc::Sender<DbCompletion>`
  - `pub(crate) fn try_recv_completed() -> Option<DbCompletion>`
- Consumes: `sqldb.rs` already imports `DbValue`/`QueryResult`/`ExecResult` from `db.rs`.

- [ ] **Step 1: Add the completion channel + types to `db.rs`**

At the top of `core/src/db.rs`, extend the imports:

```rust
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
```

Then add (near the top, after the `DbValue`/`QueryResult`/`ExecResult` definitions):

```rust
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
```

- [ ] **Step 2: Delete the moved definitions from `sqldb.rs` and import them back**

In `core/src/sqldb.rs`, delete the local `DbOutcome`/`DbCompletion` (currently lines 19-20), the `Chan` struct + `CHAN` static + `chan()` + `try_recv_completed()` (currently lines 30-36), and the now-unused imports `use std::sync::mpsc::{channel, Receiver, Sender};` and `use std::sync::{Mutex, OnceLock};` (lines 7-8). Change the `db` import (line 14) to pull the moved types:

```rust
use crate::db::{DbValue, QueryResult, ExecResult, DbOutcome, DbCompletion};
```

In `spawn_query` and `spawn_execute` (currently lines 112-125), replace `let tx = chan().tx.clone();` with:

```rust
        let tx = crate::db::completion_tx();
```

- [ ] **Step 3: Point `v8host.rs` at the moved types/channel**

In `core/src/v8host.rs`:
- `resolve_db` (line ~7409): change the parameter `result: Result<crate::sqldb::DbOutcome, String>` to `result: Result<crate::db::DbOutcome, String>`, and the two match arms `crate::sqldb::DbOutcome::Query(qr)` / `crate::sqldb::DbOutcome::Exec(er)` to `crate::db::DbOutcome::Query(qr)` / `crate::db::DbOutcome::Exec(er)`.
- The drain loop (line ~8040): change `while let Some(c) = crate::sqldb::try_recv_completed()` to `while let Some(c) = crate::db::try_recv_completed()`.

- [ ] **Step 4: Build + run the full core suite (no behavior change → all existing tests pass)**

Run: `cargo test -p s2script-core`
Expected: compiles clean; all existing tests PASS (the SQLite tests still exercise the sync path; the remote `pg_placeholder_translation` test still passes).

- [ ] **Step 5: Verify the boundary gate**

Run: `./scripts/check-core-boundary.sh`
Expected: PASS (no game-package import introduced).

- [ ] **Step 6: Commit**

```bash
git add core/src/db.rs core/src/sqldb.rs core/src/v8host.rs
git commit -m "refactor(db): move shared DB completion channel into db.rs (no behavior change)"
```

---

## Task 2: Rework `db.rs` SQLite to the connection-actor + switch the natives to async

The behavior change: SQLite `query`/`execute` now run on a per-connection actor thread; the natives hand off and resolve later via the shared spine.

**Files:**
- Modify: `core/src/db.rs` (registry type, `Command`, actor loop, `open`, `submit_query`/`submit_execute`, `close`, `run_query`/`run_execute`; rewrite tests)
- Modify: `core/src/v8host.rs` (`s2_sqlite_query`/`s2_sqlite_execute` → async)

**Interfaces:**
- Consumes: `DbValue`, `QueryResult`, `ExecResult`, `DbOutcome`, `DbCompletion`, `completion_tx`, `try_recv_completed` (Task 1); `next_async_id`, `resolver_owner_tag`, `ResolverEntry`, `RESOLVERS`, `PENDING_JOBS`, `record_job`, `refresh_detour`, `resolve_db` (existing spine, unchanged).
- Produces (in `db.rs`, replacing the sync `query`/`execute`):
  - `pub fn open(data_dir: &Path, name: &str, owner: &str) -> Result<u64, String>` (signature unchanged; now spawns an actor)
  - `pub(crate) fn submit_query(id: u64, handle: u64, sql: String, params: Vec<DbValue>, owner: &str) -> Result<(), String>`
  - `pub(crate) fn submit_execute(id: u64, handle: u64, sql: String, params: Vec<DbValue>, owner: &str) -> Result<(), String>`
  - `pub fn close(handle: u64, owner: &str) -> bool` (signature unchanged; now sends `Shutdown`)
- Note: the old `pub fn query(...)` / `pub fn execute(...)` are REMOVED (their bodies become the private `run_query`/`run_execute` run on the actor thread). Only `v8host` called them.

- [ ] **Step 1: Write the failing actor tests**

Replace the `#[cfg(test)] mod tests` block in `core/src/db.rs` with tests that drive the actor. (`tmp()` and `O` are kept.)

```rust
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
```

Add `#[derive(Debug)]`... not needed. Note: `DbValue` needs `Debug` for the `panic!("...{:?}", other.is_ok())` — that uses `.is_ok()` (bool), so no `Debug` on `DbValue` required.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p s2script-core db::`
Expected: FAIL to COMPILE — `submit_query`/`submit_execute` don't exist yet, `DbOutcome`/`try_recv_completed` are `pub(crate)` (visible) but the actor API is missing.

- [ ] **Step 3: Rework `db.rs` — registry, Command, actor loop, run_* helpers**

In `core/src/db.rs`, add `use std::thread;` and `use std::time::Duration;` to the imports. Replace the `CONNS` `thread_local!` block and the sync `query`/`execute` functions with the actor model. The `impl rusqlite::ToSql for DbValue`, `valid_name`, `open`, and `close` are updated as shown here (replace the whole region from the `thread_local!` down through `close`):

```rust
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
```

- [ ] **Step 4: Rework `open` / add `submit_*` / rework `close` in `db.rs`**

Replace the existing `open`, `get_owned`, `query`, `execute`, `close` functions with:

```rust
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
```

- [ ] **Step 5: Run the actor tests — expect PASS**

Run: `cargo test -p s2script-core db::`
Expected: PASS (all seven `db::tests::*`). This is the whole actor, tested off the V8 path.

- [ ] **Step 6: Switch the natives in `v8host.rs` to async**

Replace `s2_sqlite_query` and `s2_sqlite_execute` (the sync bodies at ~7161-7224) with the async block below (mirrors `s2_db_remote_query`/`execute`; the `id` is created before the submit, on Ok the resolver is registered — happens-before any later-frame drain — on Err the Promise is rejected with no `RESOLVERS`/`PENDING_JOBS` entry). `s2_sqlite_open` and `s2_sqlite_close` are UNCHANGED (their `db::` signatures did not change). `query_result_to_js` is UNCHANGED.

```rust
/// Native `__s2_sqlite_query(handle, sql, params) -> Promise<Row[]>`. Owner-checks + queues the SELECT
/// on the connection's actor thread (`db::submit_query`); the Promise resolves later via `resolve_db`
/// with the row array. An invalid handle / closed connection rejects the Promise immediately, with no
/// RESOLVERS/PENDING_JOBS/ledger entry (no pending job to track). MIRRORS `s2_db_remote_query`.
fn s2_sqlite_query(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();

        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);

        let id = next_async_id();
        match crate::db::submit_query(id, handle, sql, params, &owner) {
            Ok(()) => {
                let job_owner = resolver_owner_tag(scope);
                if let Some((ref oid, _)) = job_owner {
                    REGISTRY.with(|r| {
                        if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_job(id); }
                    });
                }
                RESOLVERS.with(|m| {
                    m.borrow_mut()
                        .insert(id, ResolverEntry { owner: job_owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
                });
                PENDING_JOBS.with(|c| c.set(c.get() + 1));
                refresh_detour();
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
        rv.set(promise.into());
    }));
}

/// Native `__s2_sqlite_execute(handle, sql, params) -> Promise<{changes, lastInsertId}>`. Same shape as
/// `s2_sqlite_query` but queues an INSERT/UPDATE/DELETE/DDL (`db::submit_execute`); resolves later via
/// `resolve_db` with `{changes, lastInsertId}`.
fn s2_sqlite_execute(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let handle = args.get(0).integer_value(scope).unwrap_or(-1) as u64;
        let sql = args.get(1).to_rust_string_lossy(scope);
        let params = js_params_to_db(scope, args.get(2));
        let owner = current_plugin(scope).unwrap_or_default();

        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);

        let id = next_async_id();
        match crate::db::submit_execute(id, handle, sql, params, &owner) {
            Ok(()) => {
                let job_owner = resolver_owner_tag(scope);
                if let Some((ref oid, _)) = job_owner {
                    REGISTRY.with(|r| {
                        if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_job(id); }
                    });
                }
                RESOLVERS.with(|m| {
                    m.borrow_mut()
                        .insert(id, ResolverEntry { owner: job_owner, resolver: v8::Global::new(scope.as_ref(), resolver) })
                });
                PENDING_JOBS.with(|c| c.set(c.get() + 1));
                refresh_detour();
            }
            Err(e) => {
                let msg = v8::String::new(scope, &e).unwrap();
                let ex = v8::Exception::error(scope, msg);
                resolver.reject(scope, ex);
            }
        }
        rv.set(promise.into());
    }));
}
```

Also update the doc-comment block above these natives (the "sync-behind-Promise / no threadpool this slice" note at ~7041-7048) to describe the actor: SQLite `query`/`execute` are now off-thread on a per-connection actor and resolve via the shared `resolve_db` spine like the remote driver.

- [ ] **Step 7: Build + run the FULL core suite**

Run: `cargo test -p s2script-core`
Expected: compiles clean; ALL tests PASS (the reworked `db::tests::*`, the remote `pg_placeholder_translation`, and every existing v8host test). Confirm the total count went up vs. before (new FIFO / bad-statement / concurrent-handles tests).

- [ ] **Step 8: Boundary gate**

Run: `./scripts/check-core-boundary.sh`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add core/src/db.rs core/src/v8host.rs
git commit -m "feat(db): run SQLite off the game thread via a per-connection actor"
```

---

## Task 3: Sniper rebuild + live gate (surftimer no-stall proof)

Produce the GLIBC-floored core `.so`, deploy, and prove on a live CS2 server that a finish-zone write persists AND does not stall the frame.

**Files:** none (build + deploy + verify).

- [ ] **Step 1: Sniper rebuild (core + shim relink, packaged)**

Run the project's sniper flow (a `rust:bullseye` container with the repo bind-mounted at `/repo`, running `scripts/build-sniper.sh`). Example:

```bash
docker run --rm -v "$PWD":/repo -w /repo rust:bullseye bash scripts/build-sniper.sh
```

Expected: `=== CORE glibc requirement ... must be <= 2.31` prints `<= GLIBC_2.31` (typically 2.30) for `libs2script_core.so`; `s2script.so needs: <= 2.31`; `=== DONE ===`. (No shim source changed, but the shim relinks against the rebuilt core — expected and harmless.)

- [ ] **Step 2: Static gates**

Run: `./scripts/check-core-boundary.sh && ./scripts/check-plugins-typecheck.sh`
Expected: both PASS (no game-package import; plugins still typecheck against the unchanged `.d.ts`).

- [ ] **Step 3: Deploy + restart the CS2 container**

Copy the packaged addon into the deploy dir and re-bind via a plain restart (NOT `--force-recreate`, which resets `gameinfo.gi`):

```bash
docker compose restart cs2
```

Expected on boot (server logs): `=== GAMEDATA VALIDATION: <n> ok, 0 FAILED ===`, base plugins load, `RestartCount=0`, no crash. (If Metamod didn't load after a prior update, re-run `docker exec s2script-cs2 /patch-gameinfo.sh` first — see [[cs2-update-metamod-treadmill]].)

- [ ] **Step 4: Deploy the surftimer plugin (the forcing function)**

Build + drop `s2s-surftimer-port`'s `.s2sp` into the plugins dir (file-watch hot-loads it; no restart). Confirm `[surftimer] records DB ready` in the logs (the actor-backed `open` + `CREATE TABLE`s completed).

- [ ] **Step 5: Live verification — write persists AND no frame stall**

In-game (or via a bot run through the map), cross the **finish zone** to trigger `records.submitTime()` → `db.execute(INSERT ...)`. Verify BOTH:
- **Persistence:** the record row is written — `/pb` (or a `SELECT` via an admin command / re-query) shows the time; it survives a `docker compose restart cs2` (on-disk persistence through the actor).
- **No stall (the decisive proof):** the game frame does NOT hitch on the crossing. Watch the frame-time / frame counter across the write the same way fetch/WS proved non-blocking (the counter keeps advancing during the write; no long-frame log). Contrast with the pre-change behavior the user reported (a long game frame on zone-cross).

Expected: the row persists and the frame time stays flat across the finish-zone write; `RestartCount=0`, no crash.

- [ ] **Step 6: Record the outcome**

Note the live-gate result (persist + no-stall) for the branch's finishing summary. No commit (verification only).

---

## Self-Review

**Spec coverage:**
- Actor model (dedicated thread + FIFO channel per connection) → Task 2, Steps 3-4. ✓
- Eager main-thread open → Task 2, Step 4 (`open`). ✓
- `busy_timeout=5000` default → Task 2, Step 4 (`open`). ✓
- Reuse the existing spine (shared channel + `resolve_db` + drain; natives mirror remote; RESOLVERS-before-drain happens-before) → Task 1 (channel move) + Task 2, Step 6 (natives). ✓
- Owner-scoping / opaque handles / no raw ref to JS → Task 2 (`get_sender`/`submit_*` owner-check; handle is an integer; Connection stays on the actor) + `submit_to_wrong_owner` test. ✓
- FIFO per connection → `fifo_ordering_*` test. ✓
- Degrade-never-crash (native `catch_unwind`; actor per-statement `catch_unwind`; dead-actor → reject, balanced) → Task 2 (actor loop, native Err arm) + `bad_statement_errors_but_actor_survives` test. ✓
- Ledger teardown unchanged (`Resource::DbConn` → `db::close` sends Shutdown; `record_job` per query) → `close` signature unchanged (teardown arm at v8host ~8253 keeps working) + natives `record_job`. ✓
- Same-file contention note → `busy_timeout` (Task 2). ✓
- Tests (roundtrip, bad-sql, invalid-name, closed-handle, owner-scoping, FIFO, concurrent-handles, actor-survives) → Task 2, Step 1. ✓
- Live gate (surftimer persist + no-stall) → Task 3. ✓
- Blast radius / core-only sniper / no changeset → Global Constraints + Task 3. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; commands have expected output. ✓

**Type consistency:** `submit_query`/`submit_execute(id: u64, handle: u64, sql: String, params: Vec<DbValue>, owner: &str) -> Result<(), String>` used identically in Task 2 Steps 1 (tests), 4 (defs), 6 (natives). `open`/`close` signatures unchanged (used by unchanged `s2_sqlite_open`/`close` + teardown). `DbOutcome`/`DbCompletion`/`completion_tx`/`try_recv_completed` defined in Task 1, consumed in Task 2 + sqldb + drain. `resolve_db` takes `Result<crate::db::DbOutcome, String>` (Task 1 Step 3) and is fed by the drain unchanged. ✓
