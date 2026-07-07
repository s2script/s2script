# SQLite Database Primitive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or a Workflow) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Tasks are SEQUENTIAL and DEPENDENT — implement in order, commit each.

**Goal:** An engine-generic, async-API SQLite database primitive (`@s2script/db`) that plugins use to persist data, with a `Driver` seam so MySQL slots in later without an API change.

**Architecture:** `core/src/db.rs` holds the engine-generic SQLite subsystem over `rusqlite` (bundled). The DB natives in `v8host.rs` expose it as `__s2_sqlite_*`, returning **Promises** (the async contract) but — this slice — executing **synchronously** (see the simplification note). `@s2script/db` (types package + `__s2pkg_db` prelude runtime) wraps the natives behind `Database`/`Driver`, with SQLite as the dogfooded reference driver. Consumers (clientprefs) come later.

**Tech Stack:** Rust (`rusqlite` bundled), rusty_v8, C++ shim (one new engine op), TypeScript (`@s2script/db` types).

## SIMPLIFICATION FROM THE SPEC (execution mechanism only — the API is unchanged)

The spec (`docs/superpowers/specs/2026-07-06-sqlite-db-primitive-design.md`) designed queries to run on the `async_rt` threadpool. That would require surgery on the generic async resolver (`ResolverEntry`/`resolve_or_drop` resolve `undefined` only — they carry no payload). To de-risk this slice, we **keep the async *API* (every native returns a Promise) but execute rusqlite SYNCHRONOUSLY** and resolve/reject the Promise inline. Local SQLite queries are sub-millisecond, so the tick impact is negligible for the real workloads (clientprefs/bans/admins). **The public API contract is identical** (Promise-based), so moving execution onto the threadpool later is a no-API-change follow-up. Everything else in the spec stands.

## Global Constraints

- **Core is engine-generic.** `core/src/db.rs`, the DB natives, and `@s2script/db` contain NO game/CS2 names. The `scripts/check-core-boundary.sh` gate must stay green.
- **Never a raw pointer across to JS.** A connection is an **opaque integer handle**; core maps handle → `rusqlite::Connection`.
- **Degrade-never-crash.** A bad handle / SQL error → the Promise **rejects** (never a panic, never a crash). All native bodies wrap in `std::panic::catch_unwind(AssertUnwindSafe(...))` like the existing natives.
- **Ledgered teardown.** An open connection is a plugin resource; teardown closes it even if the plugin never calls `close()`.
- **Parameterized only.** `?` placeholders + a params array. No SQL string interpolation anywhere.
- **`SqlValue = string | number | boolean | null`.** 64-bit ids are strings. Booleans store as `0`/`1` and read back as numbers (standard SQLite). Blobs out of scope.
- **ABI-append discipline.** The new `db_data_dir` engine op is APPENDED last in the `S2EngineOps` struct, in the SAME order across: the C header, the Rust mirror, the shim `ops.` assignment, AND both in-test op-structs. Never inserted mid-struct.
- **`cargo test` runs serial** (`.cargo/config.toml` `RUST_TEST_THREADS=1`, already present) — do not change it.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. No backticks inside `git commit -m` (use `-F -` heredoc).

## File Structure

- `core/Cargo.toml` — add the `rusqlite` dependency (bundled).
- `core/src/db.rs` (NEW) — engine-generic SQLite subsystem: value types, a thread-local connection registry, `open`/`query`/`execute`/`close` + name validation + unit tests.
- `core/src/lib.rs` (MODIFY) — `mod db;`.
- `core/src/plugin.rs` (MODIFY) — add `Resource::DbConn(u64)` + `record_db_conn`.
- `shim/include/s2script_core.h` (MODIFY) — append the `db_data_dir` op typedef + field.
- `shim/src/s2script_mm.cpp` (MODIFY) — implement `s2_db_data_dir` + assign `ops.db_data_dir`.
- `core/src/v8host.rs` (MODIFY) — mirror the `db_data_dir` op in `S2EngineOps` (+ both test structs); the `__s2_sqlite_*` natives + value marshalling; register them; the `Resource::DbConn` teardown arm; the `__s2pkg_db` prelude runtime.
- `packages/db/package.json` + `packages/db/index.d.ts` (NEW) — the `@s2script/db` types package.
- `plugins/db-demo/{package.json,tsconfig.json,src/plugin.ts}` (NEW) — the live-gate demo.

---

### Task 1: `core/src/db.rs` — the engine-generic SQLite subsystem

**Files:**
- Modify: `core/Cargo.toml`
- Create: `core/src/db.rs`
- Modify: `core/src/lib.rs` (add `mod db;`)

**Interfaces — Produces:**
```rust
pub enum DbValue { Null, Int(i64), Real(f64), Text(String) }
pub struct QueryResult { pub columns: Vec<String>, pub rows: Vec<Vec<DbValue>> }
pub struct ExecResult { pub changes: i64, pub last_insert_id: i64 }
pub fn open(data_dir: &std::path::Path, name: &str) -> Result<u64, String>;
pub fn query(handle: u64, sql: &str, params: &[DbValue]) -> Result<QueryResult, String>;
pub fn execute(handle: u64, sql: &str, params: &[DbValue]) -> Result<ExecResult, String>;
pub fn close(handle: u64) -> bool;   // true if a connection was present
```

- [ ] **Step 1: Add the dependency.** In `core/Cargo.toml` under `[dependencies]`:
```toml
rusqlite = { version = "0.31", features = ["bundled"] }
```
(Pick the latest 0.3x that builds; `bundled` compiles SQLite's C in — no system libsqlite3.)

- [ ] **Step 2: Write `core/src/db.rs`** with the full module below.
```rust
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
    static CONNS: RefCell<HashMap<u64, Connection>> = RefCell::new(HashMap::new());
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

pub fn open(data_dir: &Path, name: &str) -> Result<u64, String> {
    if !valid_name(name) {
        return Err(format!("invalid database name: {name:?}"));
    }
    std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create data dir: {e}"))?;
    let mut path: PathBuf = data_dir.to_path_buf();
    path.push(format!("{name}.sqlite"));
    let conn = Connection::open(&path).map_err(|e| format!("open failed: {e}"))?;
    let handle = NEXT.with(|n| { let h = n.get(); n.set(h + 1); h });
    CONNS.with(|c| c.borrow_mut().insert(handle, conn));
    Ok(handle)
}

pub fn query(handle: u64, sql: &str, params: &[DbValue]) -> Result<QueryResult, String> {
    CONNS.with(|c| {
        let map = c.borrow();
        let conn = map.get(&handle).ok_or_else(|| "invalid db handle".to_string())?;
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

pub fn execute(handle: u64, sql: &str, params: &[DbValue]) -> Result<ExecResult, String> {
    CONNS.with(|c| {
        let map = c.borrow();
        let conn = map.get(&handle).ok_or_else(|| "invalid db handle".to_string())?;
        let changes = conn
            .execute(sql, rusqlite::params_from_iter(params.iter()))
            .map_err(|e| e.to_string())? as i64;
        Ok(ExecResult { changes, last_insert_id: conn.last_insert_rowid() })
    })
}

pub fn close(handle: u64) -> bool {
    CONNS.with(|c| c.borrow_mut().remove(&handle).is_some())
}
```

- [ ] **Step 3: Write the unit tests** at the bottom of `core/src/db.rs`:
```rust
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

    #[test]
    fn open_create_insert_select_roundtrip() {
        let dir = tmp();
        let h = open(&dir, "t1").unwrap();
        execute(h, "CREATE TABLE kv (k TEXT, v TEXT)", &[]).unwrap();
        let r = execute(h, "INSERT INTO kv (k, v) VALUES (?, ?)",
            &[DbValue::Text("color".into()), DbValue::Text("red".into())]).unwrap();
        assert_eq!(r.changes, 1);
        let q = query(h, "SELECT k, v FROM kv WHERE k = ?", &[DbValue::Text("color".into())]).unwrap();
        assert_eq!(q.columns, vec!["k".to_string(), "v".to_string()]);
        assert_eq!(q.rows.len(), 1);
        match &q.rows[0][1] { DbValue::Text(s) => assert_eq!(s, "red"), _ => panic!("wrong type") }
        close(h);
    }

    #[test]
    fn bad_sql_errors_not_panics() {
        let h = open(&tmp(), "t2").unwrap();
        assert!(query(h, "SELECT * FROM nope", &[]).is_err());
    }

    #[test]
    fn invalid_name_rejected() {
        assert!(open(&tmp(), "../evil").is_err());
        assert!(open(&tmp(), "a/b").is_err());
        assert!(open(&tmp(), "").is_err());
    }

    #[test]
    fn closed_handle_is_stale() {
        let h = open(&tmp(), "t3").unwrap();
        assert!(close(h));
        assert!(query(h, "SELECT 1", &[]).is_err());
        assert!(!close(h)); // already gone
    }

    #[test]
    fn int_and_null_params_roundtrip() {
        let h = open(&tmp(), "t4").unwrap();
        execute(h, "CREATE TABLE n (a INTEGER, b REAL, c TEXT)", &[]).unwrap();
        execute(h, "INSERT INTO n VALUES (?, ?, ?)",
            &[DbValue::Int(7), DbValue::Real(1.5), DbValue::Null]).unwrap();
        let q = query(h, "SELECT a, b, c FROM n", &[]).unwrap();
        match q.rows[0][0] { DbValue::Int(i) => assert_eq!(i, 7), _ => panic!() }
        match q.rows[0][1] { DbValue::Real(f) => assert_eq!(f, 1.5), _ => panic!() }
        match q.rows[0][2] { DbValue::Null => {}, _ => panic!() }
    }
}
```

- [ ] **Step 4: Add the module.** In `core/src/lib.rs`, add `mod db;` (near the other `mod` lines).

- [ ] **Step 5: Run tests.** `cargo test --manifest-path core/Cargo.toml db::` — expect all `db::tests::*` green (and the full suite still green: `cargo test --manifest-path core/Cargo.toml`). First build compiles bundled SQLite (slow, one-time).

- [ ] **Step 6: Commit.**
```bash
git add core/Cargo.toml core/src/db.rs core/src/lib.rs
git commit -F - <<'EOF'
feat(db): engine-generic SQLite subsystem (rusqlite bundled)

core/src/db.rs — open/query/execute/close over an opaque-handle connection
registry; parameterized; name-validated; unit-tested. Engine-generic, V8-free.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 2: The `db_data_dir` engine op (shim → core)

**Files:**
- Modify: `shim/include/s2script_core.h`
- Modify: `shim/src/s2script_mm.cpp`
- Modify: `core/src/v8host.rs` (the `S2EngineOps` Rust mirror + BOTH in-test op-structs)

**Interfaces — Produces:** a `db_data_dir()` op returning a `const char*` absolute path to the s2script data directory (`<addon>/data`), created if absent. Consumed by Task 3's `__s2_sqlite_open`.

**Context:** find the LAST field of `S2EngineOps` in the C header and the Rust mirror — the new field is appended AFTER it, in both, plus the shim assignment and both test structs. Mirror `s2_config_read` (`shim/src/s2script_mm.cpp:1076`) for the static-buffer return + `ConfigPath`-style path resolution.

- [ ] **Step 1: C header.** In `shim/include/s2script_core.h`, append after the current last op field:
```c
    // Slice DB: absolute path to the s2script data directory (<addon>/data), created if absent.
    const char* (*db_data_dir)(void);
```

- [ ] **Step 2: Shim implementation.** In `shim/src/s2script_mm.cpp`, near `s2_config_read`, add (adapt `ConfigPath` to a sibling `data/` dir — reuse whatever base `ConfigPath` uses):
```cpp
static std::string s_dbDataDirBuf;
static const char* s2_db_data_dir(void) {
    // <addon base>/data  — sibling of the configs dir. Reuse the same base as ConfigPath.
    std::string dir = /* <base>/ */ AddonBaseDir() + "/data";   // use the existing base helper
    std::error_code ec; std::filesystem::create_directories(dir, ec);
    s_dbDataDirBuf = dir;
    return s_dbDataDirBuf.c_str();
}
```
Then in the ops-assignment block (near `ops.config_read = &s2_config_read;`): `ops.db_data_dir = &s2_db_data_dir;`. (If there is no `AddonBaseDir()` helper, derive the base the same way `ConfigPath(id)` does — read that function and mirror its base, appending `/data`.)

- [ ] **Step 3: Rust mirror.** In `core/src/v8host.rs`, in the `S2EngineOps` struct (the `type XFn = extern "C" fn...` + `pub field: Option<XFn>` pattern), append LAST:
```rust
type DbDataDirFn = extern "C" fn() -> *const std::os::raw::c_char;
// ... in the struct:
pub db_data_dir: Option<DbDataDirFn>,
```

- [ ] **Step 4: Both test op-structs.** Search `core/src/v8host.rs` tests for the two places that construct a full `S2EngineOps { ... }` literal (e.g. `mock_event_ops()` and any `all-None` default). Add `db_data_dir: None,` to BOTH, in the appended position, so they compile.

- [ ] **Step 5: Verify the mirror compiles.** `cargo test --manifest-path core/Cargo.toml` — expect green (the struct + test structs line up). The shim C++ compiles at sniper time (not here).

- [ ] **Step 6: Commit.**
```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp core/src/v8host.rs
git commit -F - <<'EOF'
feat(db): db_data_dir engine op (shim provides <addon>/data)

ABI-appended after the last op (C header + Rust mirror + both test structs +
shim assignment). Core composes <data>/<name>.sqlite from it.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 3: The `__s2_sqlite_*` natives + ledgered teardown

**Files:**
- Modify: `core/src/plugin.rs` (add `Resource::DbConn(u64)` + `record_db_conn`)
- Modify: `core/src/v8host.rs` (natives + marshalling + registration + teardown arm)

**Interfaces — Consumes:** `db::{open,query,execute,close}` (Task 1); the `db_data_dir` op (Task 2). **Produces (JS natives):**
- `__s2_sqlite_open(name: string) -> Promise<number>` (handle)
- `__s2_sqlite_query(handle: number, sql: string, params: any[]) -> Promise<Row[]>`
- `__s2_sqlite_execute(handle: number, sql: string, params: any[]) -> Promise<{changes, lastInsertId}>`
- `__s2_sqlite_close(handle: number) -> Promise<void>`

**Note:** synchronous execution behind a Promise — mirror `s2_thread_sleep` (`core/src/v8host.rs:1208`) for the resolver/`catch_unwind`/owner-tag shape, but instead of submitting a job, run the `db::*` call inline and `resolver.resolve(scope, value)` / `resolver.reject(scope, exception)` before returning the promise.

- [ ] **Step 1: Ledger resource.** In `core/src/plugin.rs`, add to `enum Resource` a `DbConn(u64)` variant and a `pub fn record_db_conn(&mut self, handle: u64) { self.order.push(Resource::DbConn(handle)); }` mirroring `record_job`.

- [ ] **Step 2: Teardown arm.** In `core/src/v8host.rs`, in the `for res in entry.ledger.teardown_order()` walk (near line 4559), add:
```rust
plugin::Resource::DbConn(h) => { crate::db::close(*h); }
```

- [ ] **Step 3: Value marshalling helpers** (private fns in `v8host.rs`):
```rust
// JS array (params) -> Vec<DbValue>. bool -> Int(0|1); integral number -> Int else Real; string -> Text; null/undef -> Null.
fn js_params_to_db(scope: &mut v8::PinScope, val: v8::Local<v8::Value>) -> Vec<crate::db::DbValue> {
    use crate::db::DbValue;
    let mut out = Vec::new();
    if let Ok(arr) = v8::Local::<v8::Array>::try_from(val) {
        for i in 0..arr.length() {
            let el = arr.get_index(scope, i).unwrap();
            let dv = if el.is_null_or_undefined() { DbValue::Null }
                else if el.is_boolean() { DbValue::Int(if el.boolean_value(scope) { 1 } else { 0 }) }
                else if el.is_string() { DbValue::Text(el.to_rust_string_lossy(scope)) }
                else if el.is_number() {
                    let n = el.number_value(scope).unwrap_or(0.0);
                    if n.fract() == 0.0 && n.abs() < 9.007e15 { DbValue::Int(n as i64) } else { DbValue::Real(n) }
                } else { DbValue::Text(el.to_rust_string_lossy(scope)) };
            out.push(dv);
        }
    }
    out
}
// DbValue -> v8 (Int/Real -> Number [>2^53 loses precision, documented]; Text -> String; Null -> null).
fn db_value_to_v8<'s>(scope: &mut v8::PinScope<'s, '_>, v: &crate::db::DbValue) -> v8::Local<'s, v8::Value> {
    use crate::db::DbValue;
    match v {
        DbValue::Null => v8::null(scope).into(),
        DbValue::Int(i) => v8::Number::new(scope, *i as f64).into(),
        DbValue::Real(f) => v8::Number::new(scope, *f).into(),
        DbValue::Text(s) => v8::String::new(scope, s).unwrap().into(),
    }
}
```
(Verify the exact rusty_v8 signatures against neighboring natives — e.g. `to_rust_string_lossy`, `v8::null`, `PinScope` lifetimes — and adjust to match what compiles in this crate.)

- [ ] **Step 4: The four natives.** Add, each wrapped in `catch_unwind(AssertUnwindSafe(...))` like `s2_thread_sleep`. Shape for `open` (the others follow the same resolve/reject pattern, calling `db::query`/`db::execute`/`db::close` and building the value with the helpers):
```rust
fn s2_sqlite_open(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let name = args.get(0).to_rust_string_lossy(scope);
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        // data dir via the op:
        let data_dir = ENGINE_OPS.with(|o| o.get()).and_then(|ops| ops.db_data_dir)
            .map(|f| unsafe { std::ffi::CStr::from_ptr(f()) }.to_string_lossy().into_owned());
        let result = match data_dir {
            Some(dir) => crate::db::open(std::path::Path::new(&dir), &name),
            None => Err("db not available".to_string()),
        };
        match result {
            Ok(handle) => {
                // ledger the connection against the calling plugin (mirror s2_thread_sleep's owner/ledger block)
                if let Some((ref oid, _)) = resolver_owner_tag(scope) {
                    REGISTRY.with(|r| { if let Some(l) = r.borrow_mut().ledger_mut(oid) { l.record_db_conn(handle); } });
                }
                resolver.resolve(scope, v8::Number::new(scope, handle as f64).into());
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
For `query`: parse `args.get(0)` handle (number → u64), `args.get(1)` sql (string), `args.get(2)` params (`js_params_to_db`); call `db::query`; on Ok build a `v8::Array` of row objects (`v8::Object::new`, set each `columns[c]` key → `db_value_to_v8(row[c])`), resolve it; on Err reject. For `execute`: build an object `{ changes, lastInsertId }` (both numbers). For `close`: `db::close(handle)`, resolve `undefined` (`v8::undefined(scope)`); it need not remove the ledger entry (a later teardown `close` on an already-closed handle is a harmless no-op).

- [ ] **Step 5: Register the natives.** Near the timer-native registrations (`core/src/v8host.rs:3157`), add:
```rust
set_native(scope, global_obj, "__s2_sqlite_open", s2_sqlite_open);
set_native(scope, global_obj, "__s2_sqlite_query", s2_sqlite_query);
set_native(scope, global_obj, "__s2_sqlite_execute", s2_sqlite_execute);
set_native(scope, global_obj, "__s2_sqlite_close", s2_sqlite_close);
```

- [ ] **Step 6: In-isolate tests.** In the `frame_tests` module, add a test that wires a mock `db_data_dir` op (returning a temp dir), loads a plugin that does `__s2_sqlite_open("t").then(...)`, drains, and asserts the round-trip through the natives. Mirror an existing native test's structure. Minimum: open → execute(CREATE+INSERT) → query returns the row; and a bad-SQL path rejects. If wiring the op in-test is heavy, at least assert the natives are registered and `open` with no op rejects gracefully (degrade path).

- [ ] **Step 7: Run tests.** `cargo test --manifest-path core/Cargo.toml` — expect green.

- [ ] **Step 8: Commit.**
```bash
git add core/src/plugin.rs core/src/v8host.rs
git commit -F - <<'EOF'
feat(db): __s2_sqlite_* natives (sync-behind-Promise) + ledgered teardown

open/query/execute/close resolve/reject a Promise inline; connections are a
Resource::DbConn in the ledger (auto-closed on plugin teardown). Value
marshalling: params<->DbValue, rows->JS objects.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 4: `@s2script/db` — types package + `__s2pkg_db` prelude runtime

**Files:**
- Create: `packages/db/package.json`, `packages/db/index.d.ts`
- Modify: `core/src/v8host.rs` (the `__s2pkg_db` prelude block, near `__s2pkg_config` ~line 610-625)

**Interfaces — Consumes:** the `__s2_sqlite_*` natives (Task 3). **Produces:** `@s2script/db` with `Database`, `Driver`, `DriverConnection`, `SqlValue`, `Row`, `ExecuteResult`, `registerDriver` — resolved at runtime as `globalThis.__s2pkg_db` (the generic `s2require` rule maps `@s2script/<name>` → `__s2pkg_<name>`; no per-module wiring needed).

- [ ] **Step 1: `packages/db/package.json`:**
```json
{ "name": "@s2script/db", "version": "0.1.0", "types": "index.d.ts" }
```

- [ ] **Step 2: `packages/db/index.d.ts`:**
```ts
/** @s2script/db — engine-generic async SQLite database. NO runtime code (injected as __s2pkg_db). */
export type SqlValue = string | number | boolean | null;
export type Row = Record<string, SqlValue>;
export interface ExecuteResult { changes: number; lastInsertId: number; }

export interface DriverConnection {
  query(sql: string, params?: SqlValue[]): Promise<Row[]>;
  execute(sql: string, params?: SqlValue[]): Promise<ExecuteResult>;
  close(): Promise<void>;
}
export interface ConnectionConfig { driver: string; name: string; [k: string]: unknown; }
export interface Driver {
  readonly name: string;
  connect(config: ConnectionConfig): Promise<DriverConnection>;
}
/** A live database connection (delegates to its driver). */
export interface Database {
  query(sql: string, params?: SqlValue[]): Promise<Row[]>;
  execute(sql: string, params?: SqlValue[]): Promise<ExecuteResult>;
  close(): Promise<void>;
}
export declare const Database: {
  /** Open a connection by name (default "default"). Resolves the driver + config, then connects. */
  open(name?: string): Promise<Database>;
  /** Register a custom driver (per-plugin context). SQLite is built in. */
  registerDriver(driver: Driver): void;
};
```

- [ ] **Step 3: The prelude runtime.** In `core/src/v8host.rs`, near the `__s2pkg_config` assignment (~line 625), add a `__s2pkg_db` block. Write it as a JS string consistent with the surrounding prelude style:
```js
// --- @s2script/db: async SQLite (sync-behind-Promise this slice) + Driver seam ---
var __s2_db_drivers = {};
var __s2_sqliteDriver = {
  name: "sqlite",
  connect: function (config) {
    return __s2_sqlite_open(config.name).then(function (handle) {
      return {
        query:   function (sql, params) { return __s2_sqlite_query(handle, sql, params || []); },
        execute: function (sql, params) { return __s2_sqlite_execute(handle, sql, params || []); },
        close:   function () { return __s2_sqlite_close(handle); },
      };
    });
  },
};
__s2_db_drivers["sqlite"] = __s2_sqliteDriver;
var __s2_Database = {
  registerDriver: function (driver) { __s2_db_drivers[driver.name] = driver; },
  open: function (name) {
    var connName = name || "default";
    // This slice: every name resolves to the local SQLite driver. (databases.cfg remap is a follow-on.)
    var config = { driver: "sqlite", name: connName };
    var driver = __s2_db_drivers[config.driver];
    if (!driver) return Promise.reject(new Error("unknown db driver: " + config.driver));
    return driver.connect(config).then(function (conn) {
      return {
        query:   function (sql, params) { return conn.query(sql, params); },
        execute: function (sql, params) { return conn.execute(sql, params); },
        close:   function () { return conn.close(); },
      };
    });
  },
};
globalThis.__s2pkg_db = { Database: __s2_Database };
```
(Match the exact quoting/escaping of the surrounding prelude string; the `.d.ts` exports `Database` as a namespace-with-methods, so `__s2pkg_db = { Database }` matches `import { Database } from "@s2script/db"`.)

- [ ] **Step 4: In-isolate test.** In `frame_tests`, add a test: load a plugin that `require`s `@s2script/db` (or reads `__s2require("@s2script/db")`), asserts `typeof m.Database.open === "function"` and `typeof m.Database.registerDriver === "function"`. If a `db_data_dir` mock op is wired (from Task 3), also drive `Database.open("t").then(db => db.execute(...))` end-to-end and assert on the drained result.

- [ ] **Step 5: Typecheck the package.** Ensure `@s2script/db` resolves under the CLI typecheck (the wildcard `@s2script/*` externalization already covers it). Add a one-liner to a hermetic fixture only if the existing typecheck test enumerates packages; otherwise the demo (Task 5) is the typecheck proof.

- [ ] **Step 6: Run tests.** `cargo test --manifest-path core/Cargo.toml` — green.

- [ ] **Step 7: Commit.**
```bash
git add packages/db core/src/v8host.rs
git commit -F - <<'EOF'
feat(db): @s2script/db — Database API + Driver seam + SQLite reference driver

Types package + the __s2pkg_db prelude runtime. Database.open/query/execute/
close over the __s2_sqlite_* natives; SQLite dogfoods the Driver interface;
registerDriver seam for future drivers. name->config resolution stubbed to
local SQLite (databases.cfg remap deferred).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 5: The `db-demo` plugin (live-gate proof)

**Files:**
- Create: `plugins/db-demo/package.json`, `plugins/db-demo/tsconfig.json`, `plugins/db-demo/src/plugin.ts`

**Interfaces — Consumes:** `@s2script/db` (Task 4). Mirror an existing plugin's `package.json`/`tsconfig.json` (e.g. `plugins/funcommands/`).

- [ ] **Step 1: `package.json`** (mirror `plugins/funcommands/package.json`):
```json
{ "name": "@s2script/db-demo", "version": "0.1.0", "main": "src/plugin.ts", "s2script": { "apiVersion": "1.x" } }
```

- [ ] **Step 2: `tsconfig.json`** (copy `plugins/funcommands/tsconfig.json` verbatim).

- [ ] **Step 3: `src/plugin.ts`:**
```ts
// @s2script/db-demo — proves the SQLite primitive persists across a server restart.
import { Database } from "@s2script/db";

export async function onLoad(): Promise<void> {
  try {
    const db = await Database.open("demo");
    await db.execute("CREATE TABLE IF NOT EXISTS boots (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT)");
    const res = await db.execute("INSERT INTO boots (at) VALUES (?)", ["load"]);
    const rows = await db.query("SELECT COUNT(*) AS n FROM boots", []);
    const n = rows.length ? rows[0].n : 0;
    console.log("[db-demo] onLoad — inserted id=" + res.lastInsertId + " total boots=" + n);
    await db.close();
  } catch (e) {
    console.log("[db-demo] onLoad ERROR: " + String(e));
  }
}

export function onUnload(): void { console.log("[db-demo] onUnload"); }
```

- [ ] **Step 4: Build it.** `node packages/cli/dist/cli.js build plugins/db-demo` — expect a typecheck pass + `plugins/db-demo/dist/_s2script_db-demo.s2sp` emitted (no type errors → the `@s2script/db` `.d.ts` surface is validated).

- [ ] **Step 5: Commit.**
```bash
git add plugins/db-demo
git commit -F - <<'EOF'
feat(db): db-demo plugin (persist-across-restart live gate)

Opens a DB, CREATE TABLE, INSERT a boot row, SELECT COUNT — the total climbs
each boot, proving on-disk persistence. Validates the @s2script/db .d.ts.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Post-implementation (controller / me — NOT a workflow task)

1. **Sniper rebuild** (compiles the shim C++, `rusqlite` bundled, the new natives): `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`.
2. **Package + deploy**: `scripts/package-addon.sh`; recreate `dist/addons/s2script/{configs,data}` as gkh (container-writable — the DB dir needs writes, like configs); copy the `.s2sp`; `docker compose -f docker/docker-compose.yml restart cs2`.
3. **Live gate (bots-provable):** confirm `[db-demo] onLoad — inserted id=1 total boots=1`; then `docker compose restart cs2` again → `total boots=2` (the row PERSISTED across restart, and the id climbed). Confirm no crash, server ticking. If `gameinfo.gi` was reset, re-run `patch-gameinfo.sh`.
4. **Gates:** `scripts/check-core-boundary.sh` (db.rs engine-generic), full `cargo test`, `scripts/check-plugins-typecheck.sh`.
5. **Final whole-branch review**, then merge `slice-sqlite-db` → main + push.

## Self-review notes

- **Spec coverage:** architecture (T1/T3/T4), Database API (T4 `.d.ts` + prelude), Driver seam + SQLite reference (T4), async natives (T3 — sync-behind-Promise per the noted simplification), data location + name sanitization (T1 `valid_name` + T2 data dir), ledger/degrade/liveness (T3), tests + live gate (all tasks + post-impl). MySQL/clientprefs/net/extension-doc remain deferred (not in any task). ✓
- **Deviation from spec:** execution is synchronous-behind-Promise this slice (threadpool deferred) — API unchanged; noted at top + in the spec.
- **Type consistency:** `DbValue`/`QueryResult`/`ExecResult` (Rust) ↔ `SqlValue`/`Row`/`ExecuteResult` (TS) ↔ the `__s2_sqlite_*` native signatures ↔ the `__s2pkg_db` runtime — all aligned. `Database.open(name)` / `query(sql, params)` / `execute` / `close` consistent across `.d.ts`, prelude, and demo.
