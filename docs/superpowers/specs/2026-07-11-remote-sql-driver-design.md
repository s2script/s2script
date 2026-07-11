# Remote SQL driver (MySQL + Postgres via sqlx) — design

**Date:** 2026-07-11
**Status:** approved (design)
**Slice:** remote-sql-driver (the DB primitive's remote backend — MySQL + Postgres)

## Goal

Let `Database.open("stats")` resolve — by an operator-owned config file — to a **MySQL or PostgreSQL** connection instead of local SQLite, with the query running **async off the game thread**. Zero plugin-code change: the same `@s2script/db` API (`open`/`query`/`execute`/`close`) works against a remote SQL server. This is the long-deferred remote backend the DB `Driver` seam was built for.

## Background — what exists

- **`@s2script/db` (Slice DB)** — `Database.open(name) → Promise<Database>` with `query`/`execute`/`close`. The `Driver`/`DriverConnection` interface is the extension seam; the built-in **SQLite driver dogfoods it**. `Database.open` **resolves** a name → a `ConnectionConfig` → looks up `config.driver` in a per-context registry → `driver.connect(config)`. Today the resolver stubs **every** name to `{ driver: "sqlite", name }` (the `databases.cfg`-style remap was always the intended seam).
- **SQLite is synchronous behind the Promise API** (`core/src/db.rs`, rusqlite inline) — fine for a local file (sub-ms), but it **blocks the isolate thread**. A remote query over the network can take tens–hundreds of ms, so it **cannot** use this path — it must run off-thread.
- **The async-network spine already exists** (`fetch`/WebSocket): an internal `tokio` multi-threaded runtime (`core/src/http.rs`), the async-result resolver (`RESOLVERS`/`record_job`/`PENDING_JOBS` → a frame-drain step builds the result + resolves the Promise, with an owner-context + liveness guard so a job outstanding when its plugin unloads **drops** instead of resolving into a dead context). WebSocket added `resolve_ws_connect` alongside `resolve_fetch` — the same pattern generalizes to a DB result.

## Scope decisions (locked)

- **Approach:** a **core-Rust driver using `sqlx`** (MySQL + Postgres from one crate, async on the existing tokio runtime). NOT a TS-over-`@s2script/net` driver — hand-writing the MySQL/Postgres wire protocols (auth, TLS, framing, prepared statements) in TypeScript is impractical and security-sensitive. `@s2script/net` stays a separate future primitive, off this slice's path.
- **Engines:** **both MySQL and PostgreSQL** this slice.
- **Connections are config-only** — a plugin opens by *name*; it never passes host/credentials inline. The operator owns `databases.json` (credentials out of plugin code; a plugin cannot point at an arbitrary host → no SSRF). SM's `databases.cfg` model.
- **Async off-thread** — remote query/execute run on tokio and resolve on the frame drain (the "truly non-blocking DB execution" the DB spec deferred, now real for remote). Local SQLite stays sync-behind-Promise, unchanged.
- **No `packages/*` change** — this slots entirely behind the existing `Database`/`Driver` API and the existing `SqlValue` union. Core + config + the `@s2script/db` prelude only → **local-merge cadence, not a PR/changeset**.

## A. Connection config + resolution — `databases.json`

An operator-owned `databases.json` read through the existing config bridge (`__s2_config_read_raw("databases")` → `addons/s2script/configs/databases.json`), **auto-generated as a valid-JSON `_help` template when absent** (the `admins.json`/`bans.json` pattern):

```json
{
  "stats": { "driver": "mysql",    "host": "db.host", "port": 3306, "user": "cs2", "password": "…", "database": "stats" },
  "prefs": { "driver": "postgres", "host": "db.host", "port": 5432, "user": "cs2", "password": "…", "database": "prefs" }
}
```

The `@s2script/db` resolver changes from "every name → sqlite" to:
- name present in `databases.json` → **that config** (`driver` ∈ `{mysql, postgres, sqlite}`; the remote configs carry `host`/`port`/`user`/`password`/`database`).
- name absent → **default `{ driver: "sqlite", name }`** — backward-compatible; every existing sqlite-by-name connection keeps working with no config file.

The config is read + parsed once per context (cached), like the admin file. Malformed JSON → WARN + treat as empty (all names fall back to sqlite) — degrade, never crash.

The `@s2script/db` module **auto-registers built-in `mysql` and `postgres` drivers** (thin `Driver` wrappers over the core natives) in every context, alongside the existing `sqlite` built-in — so `Database.open` finds `config.driver` in the same per-context registry for all three, and each dogfoods the public `Driver` seam a community driver would use.

## B. The core-Rust driver (`core/src/sqldb.rs`, sqlx)

`sqlx` added to core (features `runtime-tokio-rustls`, `mysql`, `postgres`) like rusqlite/reqwest/tokio-tungstenite.

- **`connect(config)`** builds a **lazy** sqlx pool (`MySqlPoolOptions`/`PgPoolOptions` → `connect_lazy_with`, modest `max_connections` e.g. 4, an `acquire_timeout` e.g. 10 s). Lazy so `connect` returns an opaque integer **handle** instantly (no blocking); the first query establishes the connection, and a dead DB surfaces as a **query rejection** on the acquire-timeout, not a frozen frame. The handle lives in an owner-scoped registry `POOLS: HashMap<u64, (PoolKind, owner)>` where `PoolKind = MySql(MySqlPool) | Postgres(PgPool)` (same ownership discipline as `db.rs::get_owned` — a wrong owner reads "invalid db handle", not probeable). Core assembles the connection options from the config fields and **never logs the password**.
- **`query`/`execute`** run **async on the existing tokio runtime** (via a shared `http::spawn` accessor). `query` → `sqlx::query(sql).bind(…).fetch_all(&pool)`; `execute` → `.execute(&pool)` → `{ rows_affected, last_insert_id }` (MySQL `last_insert_id()`; Postgres has none → `0`, documented — use `RETURNING` for PG ids).
- **Type mapping → the existing `SqlValue` union** (`string | number | boolean | null`) — AS BUILT: integers that fit f64 → `number`; **`BIGINT` → decimal `string`** (precision — steamids as BIGINT; the 5B.4 "64-bit as decimal string" rule; `string` is already in the union); float/double → `number`; text/varchar/char → `string`; bool → `number` (`0|1`, consistent with the existing SQLite driver — `DbValue` has no Bool variant); NULL → `null`; BLOB/`bytea` → `null`. **DECIMAL/NUMERIC and DATE/TIME/TIMESTAMP → `null` this slice (DEFERRED):** decoding them to a string needs the `rust_decimal`/`chrono` sqlx features, deliberately not enabled to keep the dep tree lean (the sniper build already compiles sqlx); the fallible `try_get` fallback degrades them to `null`, never a panic — enabling those features + a decode-to-string is a follow-up. **LIVE-GATE NOTE:** Postgres strictly rejects a text-bound param into a `BIGINT` column (MySQL coerces, PG does not) — so a big value bound as a decimal string can't go into a PG `bigint` column; use a numeric literal, a `?::int8` cast, or a number that fits f64. Inherent PG behavior, not a driver bug. Params (`SqlValue` in): string → text bind, number → int/float bind, bool → bool, null → null.
- **Postgres placeholder translation:** plugin SQL keeps the `?` convention (SQLite/MySQL style); the Postgres path rewrites `?` → `$1..$n` before executing, so the same SQL string is portable. (Dialect differences beyond placeholders are the plugin author's responsibility — this is a convenience, not full portability.)

## C. Async spine + safety (reuse the fetch/ws pattern)

- **The remote query/execute natives mirror `__s2_fetch`:** record a job → spawn the query onto the tokio runtime → return a Promise instantly. The task runs the sqlx call, sends `(id, Result<QueryResult, String>)` (or `Result<ExecResult, String>`) down an mpsc channel; a new **frame-drain step** drains completions and `resolve_db_query` / `resolve_db_execute` builds the JS rows array / result object and resolves the Promise (exactly how ws added `resolve_ws_connect` beside `resolve_fetch`). **Liveness/UAF guard:** a query outstanding when its plugin unloads **drops** (the owner-context check — never resolves into a disposed context).
- **Safety:** owner-scoped opaque integer pool handles (query/execute/close verify `current_plugin` owns the handle); **ledgered** (a new `Resource::RemoteDbConn` → teardown closes the pool even if `close()` was never called); `catch_unwind` on every native; a connect/query/SQL error **rejects the Promise** (the plugin's `.catch`), never crashes; the acquire-timeout bounds a dead-DB query.
- **`close()`** drops the pool (async pool close spawned on tokio; the handle is removed from the registry synchronously so it's immediately invalid).

## D. Threading model

The tokio runtime already runs (~4 background threads, `http.rs`). sqlx uses it; remote queries execute on tokio's threads, fully off the game thread. The isolate thread only does the instant handoff (`connect`/`query` return immediately) + the resolve-on-drain — identical to the proven `fetch`/WebSocket non-blocking model. Many concurrent queries multiplex over the pool + few threads.

## E. Testing

**In-isolate (pure, no live DB):**
- The config resolver: a name in `databases.json` → its mysql/postgres config; an absent name → the sqlite default; malformed JSON → all-sqlite fallback; template auto-gen when absent.
- The `?` → `$1..$n` Postgres placeholder translation (including no-placeholder and repeated-param cases).
- The type mapping (pure fn): int→number, BIGINT→string, float→number, bool→boolean, NULL→null, text→string.

**Live gate (Docker):**
- Add `mysql:8` + `postgres:16` **sidecar services** to `docker/docker-compose.yml` on a shared network the CS2 container reaches; seed a `databases.json` pointing `"stats"`→mysql, `"prefs"`→postgres.
- A demo plugin opens both, runs `CREATE TABLE` / `INSERT` / `SELECT` round-trips against each, asserts a `BIGINT` column reads back as a **decimal string**, and logs the **game-frame counter advancing while a query is in flight** (the fetch-demo non-blocking proof).
- Sniper rebuild (core adds sqlx); confirm `GAMEDATA n/0`, no panic, `RestartCount=0`.

## Boundary check

- `sqldb.rs` + the natives are engine-generic (connection config + SQL strings → no CS2/game symbol). ✓
- `@s2script/db` is an engine-generic prelude module. ✓
- sqlx is a core Rust dependency (like rusqlite/reqwest) — no shim change. **No new `S2EngineOps` op** (the natives are `set_native`'d; the data dir already comes from the `db_data_dir` op for the sqlite path, unused by remote). One sniper rebuild (core `.so`).

## Out of scope (do not build ahead)

- **Transactions** (multi-statement `BEGIN`/`COMMIT`) — the `DriverConnection` is per-query; a real transaction needs a pinned session/connection held across calls. Deferred.
- **Inline (non-config) connections** — a plugin passing host/credentials directly. Config-only this slice (secure + SM parity).
- **Blobs / `bytea`** (binary columns), migrations, prepared-statement caching/reuse across calls, connection-pool tuning knobs in config.
- Moving **SQLite** to the async path (it stays sync-behind-Promise — local, sub-ms).
- **`@s2script/net`** (raw TCP/UDP sockets) — a separate future primitive, not needed for DB.
