# SQLite Database Primitive — Design

**Status:** Approved (brainstorm), ready for implementation plan.
**Slice:** the persistence primitive — the first new I/O capability in s2script's core-primitive surface.

## Goal

Give plugins a real, non-blocking SQL database — **SQLite as the zero-config local default** — through an engine-generic API (`@s2script/db`) with a **`Driver` interface** so a remote backend (MySQL/Postgres) can be added later without changing plugin code. This is the persistence foundation that clientprefs (and eventually basebans/admins) build on.

## Motivation & context

SourceMod persists per-client cookies, bans, and admin data in a SQL database — SQLite by default, with the option to point at a shared MySQL server via `databases.cfg`. s2script has no database today; `admins.json`/`bans.json` are whole-file JSON dictionaries via the config bridge, which does not scale to per-client, unbounded data (clientprefs).

This slice delivers the SQLite primitive. It is deliberately the **first concrete instance of s2script's extension model**: rather than a native-`.so` extension SDK (which would betray the "core owns every engine touchpoint" safety charter), s2script delivers SourceMod-extension-level *capability* by growing a **curated set of engine-generic core primitives** (I/O first: DB now, then a socket, HTTP, fs) that the community composes in TypeScript (as packages + inter-plugin interfaces + the `unsafe` escape hatch). SQLite is the first such primitive. (The full extension-model mapping is a separate follow-up design doc.)

## Scope

**In scope (this slice):**
- `core/src/db.rs` — an engine-generic SQLite subsystem over `rusqlite` (bundled), running on the existing `async_rt` threadpool with frame-drain Promise marshalling.
- The SQLite natives (`__s2_sqlite_open`/`query`/`execute`/`close`).
- `@s2script/db` — the `Database` API, the `Driver`/`DriverConnection` interfaces, `registerDriver`, connection-name resolution, and the built-in SQLite driver (which dogfoods the `Driver` interface).
- Ledgered connections + degrade-never-crash + liveness.
- Core Rust unit tests + a bots-provable live gate proving persistence across a server restart.

**Deferred (named follow-on slices, NOT built here):**
- **MySQL / external SQL** — as a TS driver over a future `@s2script/net` socket primitive (preferred) or a core-Rust driver; plus a `databases.cfg`-style named-connection config file.
- **clientprefs** (CS2) — cookie registry, SteamID keying, connect/disconnect caching, `sm_settings`/`sm_cookies` (the menu also needs the deferred menu primitive).
- **The extension-model design doc** — mapping every SM-extension category → the s2script layer that covers it.
- Migrating `admins.json`/`bans.json` onto the DB.
- Transactions, prepared statements, an `escape()` helper, streaming result sets, blob values.

## Architecture & layering

One-way dependencies (game → core, never the reverse):

1. **Core (Rust, engine-generic) — `core/src/db.rs`.** `rusqlite` with the `bundled` feature (vendors + compiles SQLite's C source in — no system `libsqlite3`). Connections are opaque **integer handles**; core holds a registry `handle → (Arc<Mutex<rusqlite::Connection>>, owner_plugin)`. Every operation runs on the existing `async_rt` threadpool and marshals its result back to a V8 Promise on the **frame drain** (the Slice-2 pattern), so the game tick never blocks on I/O. Core knows only "SQLite," nothing about any game.
2. **`@s2script/db` (engine-generic TS module) — the contract.** The `Database` API plugins use, the `Driver`/`DriverConnection` interface (the extension seam), a `registerDriver` registry, and the connection-name → config resolver. The built-in **SQLite driver is itself a `Driver` implementation** (a thin wrapper over the core natives) — our own code goes through the same door a community driver would.
3. **Consumers (later slices).** clientprefs (CS2) on top; eventually basebans/admins migrate off JSON.

**Boundary:** a database is engine-generic, so it lives in core + `@s2script/db`; only clientprefs is CS2. The core-boundary gate (`db.rs` contains no game/CS2 names) and the typecheck gate stay green.

## The `Database` API (what plugins use)

```ts
const db = await Database.open("clientprefs");     // name optional; defaults to "default"

const rows = await db.query(
  "SELECT value FROM cookies WHERE steamid = ? AND name = ?",
  [player.steamId, "hud_color"]
);   // rows: Row[]  where  Row = Record<string, SqlValue>

const res = await db.execute(
  "INSERT INTO cookies (steamid, name, value) VALUES (?, ?, ?)",
  [player.steamId, "hud_color", "#ff0000"]
);   // res: { changes: number, lastInsertId: number }

await db.close();   // optional — the ledger auto-closes on plugin teardown
```

- `type SqlValue = string | number | boolean | null;`
- `type Row = Record<string, SqlValue>;`
- `interface ExecuteResult { changes: number; lastInsertId: number; }`
- **`open(name)` takes a connection *name*, not a path/options** — the operator-remap seam (see Data location). Default `"default"`.
- **All queries are parameterized** (`?` + a `params` array). No string concatenation, no escape helper, injection-safe by construction.
- **64-bit ids bind/store as strings** (`player.steamId` is already a decimal string) — avoids the 2⁵³ float-precision issue; no `bigint` in the API.
- **All async / Promise-based.** A SQL error (bad syntax, constraint) **rejects** the Promise for the plugin to catch; it never crashes the server.

## The `Driver` interface & extensions paradigm

`Database` depends only on the `Driver` interface; SQLite dogfoods it.

```ts
interface Driver {
  readonly name: string;                                     // "sqlite", "mysql", …
  connect(config: ConnectionConfig): Promise<DriverConnection>;
}
interface DriverConnection {
  query(sql: string, params: SqlValue[]): Promise<Row[]>;
  execute(sql: string, params: SqlValue[]): Promise<ExecuteResult>;
  close(): Promise<void>;
}
Database.registerDriver(driver: Driver): void;
```

`ConnectionConfig` is **driver-specific**: for SQLite it is `{ driver: "sqlite"; name: string }` (the sanitized connection name; core turns it into `data/<name>.sqlite`). A future MySQL config would carry `{ driver: "mysql"; host; user; password; database }`. The resolver (below) produces it from the connection name.

`Database.open(name)`: **resolve** the name → a `ConnectionConfig` → **look up** `config.driver` in the registry → `driver.connect(config)` → wrap the `DriverConnection` in a ledgered `Database` handle whose methods delegate to it.

The built-in SQLite driver (auto-registered by `@s2script/db` in every plugin context):
```ts
const SqliteDriver: Driver = {
  name: "sqlite",
  async connect(config) {
    const h = await __s2_sqlite_open(config.name);   // core sanitizes + composes data/<name>.sqlite
    return {
      query:   (sql, p) => __s2_sqlite_query(h, sql, p),
      execute: (sql, p) => __s2_sqlite_execute(h, sql, p),
      close:   () => __s2_sqlite_close(h),
    };
  },
};
```

**Two future driver flavors, both implementing this same interface (deferred):**
- **Core-native driver** (server-wide, auto-registered like SQLite, threaded) — the robust path for heavy wire protocols; a candidate for MySQL.
- **Community JS driver** (a `@<community>/*` TS package; a plugin imports it and calls `registerDriver`; does I/O over the future `@s2script/net` socket primitive) — userland-implementable, per-context.

`registerDriver` is **per-plugin-context** (a JS `Driver` is a live object in one V8 context and cannot cross to another — charter). Built-in/native drivers like SQLite are the ones registered globally (in every context).

## Async & native mechanism

- **New core dependency: `rusqlite` with the `bundled` feature** — self-contained (compiles SQLite's C in; no system dependency). Cost: added compile time + ~1–2 MB in the core `.so`; the sniper's `rust:bullseye` has the `cc` toolchain.
- A connection is an **opaque integer handle** — no raw pointer ever crosses to JS (charter).
- Each operation runs the rusqlite call and settles the Promise. **Implementation note (this slice):** to avoid surgery on the generic async resolver (`ResolverEntry`/`resolve_or_drop` carry no payload — they resolve `undefined` only), the natives execute **synchronously** and resolve/reject the Promise inline. The **API stays Promise-based regardless**, so moving execution onto the `async_rt` threadpool (the truly-non-blocking design) is a later, no-API-change follow-up; local SQLite queries are sub-millisecond, so the tick impact is negligible for the intended workloads. (Threaded design, deferred: submit a closure to the pool that locks the connection's `Mutex`, runs the call, and enqueues a frame-drain resolver — the Slice-2 marshalling; the per-connection `Mutex` serializes queries, which SQLite wants anyway.)
- **Natives** (all return Promises): `__s2_sqlite_open(name) → handle` (core sanitizes `name` and composes `<data>/<name>.sqlite` — path composition and validation live in core, which holds the data dir), `__s2_sqlite_query(handle, sql, params) → Row[]`, `__s2_sqlite_execute(handle, sql, params) → ExecuteResult`, `__s2_sqlite_close(handle)`.
- **Value marshalling.** Params: `string→TEXT`, `number→INTEGER` if integral else `REAL`, `boolean→0/1`, `null→NULL`. Results map back to `string | number | boolean | null`. Two documented (standard-SQLite) quirks: booleans read back as `0`/`1` numbers, and INTEGER columns > 2⁵³ lose precision → store 64-bit ids as TEXT. Blobs are out of scope this slice.

## Data location & connection resolution

- SQLite files live in **`addons/s2script/data/<name>.sqlite`**. The shim owns filesystem layout (as it already does for `configs/`), so it provides the **s2script data directory** (an engine-generic absolute-path string) to core; **core** composes `<data>/<name>.sqlite` and auto-creates the dir.
- `name` is **sanitized in core** — `^[A-Za-z0-9_-]+$` (it becomes a filename, so core validates it before composing the path — never trust the caller for a path component); default `"default"`. An invalid name rejects.
- **Name → config resolution is the operator-remap seam.** This slice, the `@s2script/db` resolver maps every name to `{ driver: "sqlite", name: "<name>" }`. Later, a `databases.cfg`-style config file remaps specific names to a remote driver — plugin code unchanged. No config file is required this slice.

## Errors, degrade, teardown

- **Degrade-never-crash.** Core not wired / an invalid or closed handle → the native rejects gracefully. A SQL error → the Promise **rejects** with SQLite's message. Nothing crashes the server.
- **Ledgered.** `open` registers the connection as a plugin resource; teardown closes every connection the plugin left open (drops the rusqlite `Connection`) — no leak even if the plugin forgets `close()`.
- **Liveness.** A query in flight when the plugin unloads → its resolver is dropped (the Slice-2 `REGISTRY.is_live` guard) and the connection is closed by the ledger — no use-after-free; the Promise simply never settles (the plugin is gone).

## Shim changes

- Provide the **s2script data directory** absolute path to core (engine-generic base-path string).
- No other shim work — `rusqlite` lives in core; the natives are core.
- One sniper rebuild (new natives + `rusqlite` + the data-dir wiring).

## Testing & live gate

- **Core Rust unit tests** (`db.rs`, in-isolate — rusqlite is self-contained, no engine needed): open a temp/`:memory:` DB; create/insert/query; param binding + the value-marshalling quirks; SQL error **rejects** (not crashes); handle lifecycle + stale-handle rejection; ledger closes on owner teardown.
- **Module / typecheck:** `@s2script/db` typechecks under the strict gate; the `Driver` interface + SQLite reference compile.
- **Live gate (bots-provable, no human):** a demo plugin opens a DB, `CREATE TABLE IF NOT EXISTS`, `INSERT` a row (keyed by a counter/timestamp), queries it back and logs it; then **restart the server and query again → the row persists** (proves real on-disk persistence + the shim data path). Confirm the query does not stall the tick.
- **Gates:** core-boundary (`db.rs` engine-generic — no CS2 names), typecheck (`@s2script/db`). No codegen → no freshness gate.

## Risks & open questions

- **`rusqlite` dependency.** New core dep; `bundled` keeps it self-contained but adds compile time + `.so` size. Accepted (SQLite is the de-facto embedded DB; SourceMod uses it).
- **Threadpool saturation.** DB queries share the fixed-size `async_rt` pool with `delay`/`threadSleep`. SQLite queries are fast; if this ever becomes a bottleneck, a dedicated DB pool is a later change. Not addressed this slice.
- **Data-dir provisioning on deploy.** Like `configs/`, `addons/s2script/data/` must be container-writable (the 5E.2 nested-rw-mount pattern) — a deploy note, mirrored from the config system.

## Boundary compliance summary

- `core/src/db.rs`, the SQLite natives, and `@s2script/db` are **engine-generic** — no game/CS2 names. Both the core-boundary and typecheck gates stay green.
- clientprefs (the CS2 consumer) is a **separate later slice** in the game-package layer.
