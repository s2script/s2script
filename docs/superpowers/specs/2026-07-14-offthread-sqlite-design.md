# Off-thread SQLite (rusqlite connection-actor) — design

**Date:** 2026-07-14
**Status:** approved (design)
**Slice type:** core-only (sniper rebuild); no shim, no engine op, no ABI change, no `packages/*` change → local-merge slice (no changeset).

## Problem

SQLite (`core/src/db.rs`) is the only database code still running **on the game thread**. The
original DB slice deliberately shipped it "synchronous-behind-Promise": `s2_sqlite_query` /
`s2_sqlite_execute` call rusqlite inline and resolve the Promise immediately. The remote SQL
drivers (`core/src/sqldb.rs`, MySQL/Postgres over sqlx+tokio) are already fully off-thread.

The synchronous SQLite path stalls the frame on any write. The forcing case is the surftimer
port (`~/projects/s2s-surftimer-port`): crossing the finish zone calls `records.submitTime()` →
`db.execute("INSERT ... ON CONFLICT ...")` (`src/db/records.ts:65`), which runs rusqlite inline on
the game thread — the disk write + fsync produce a long game frame. The plugin author already had
to contort around this (WAL + `synchronous=NORMAL` at `records.ts:18`; batching splits off the hot
path at `records.ts:74`) and it *still* stalls "no matter what."

## Goal / non-goals

**Goal:** SQLite `query`/`execute` (and the DDL/PRAGMA statements that ride through them) run off
the game thread. Per-call game-thread cost drops to a channel hand-off; the Promise resolves on a
later frame drain — reusing the existing async-result spine.

**Non-goals:**
- No `@s2script/db` API change. `Database.open`/`query`/`execute`/`close` stay Promise-returning
  with identical signatures. No plugin code changes.
- Remote (sqlx) drivers untouched — already off-thread.
- No transactions, no blobs (still out of scope, as today).
- **No new crate deps.** SQLite is blocking work with nothing async about it; the actor is pure
  `std::thread` + `std::sync::mpsc`. No tokio involvement; `core/src/http.rs` is untouched.

## Approach: rusqlite connection-actor

Chosen over unifying on sqlx-sqlite. The actor keeps rusqlite exactly as-is (same bundled/pinned
SQLite, same type mapping, same tests), gives a focused diff, preserves per-connection submission
ordering by construction, and adds no dependency. (Unifying on sqlx-sqlite would delete code and
give one SQL path, but carries type-parity / bundling / last-insert-rowid / PRAGMA risk and would
force re-verifying every SQLite consumer — a bigger refactor than "move it off-thread" warrants.)

### The actor

`db.rs`'s registry changes from `HashMap<u64, (Connection, owner)>` to
`HashMap<u64, (ConnHandle, owner)>`, where `ConnHandle = { cmd_tx: Sender<Command> }`. The
`Connection` no longer lives on the main thread — it is owned by a dedicated actor thread
(`s2db-<name>`), one per open connection. A dedicated thread (not a shared blocking pool) gives
FIFO ordering for free, keeps the connection pinned to one thread (rusqlite's happy path), and a
parked thread per open DB is negligible (a handful of connections process-wide).

`Command` enum:
- `Query { id: u64, sql: String, params: Vec<DbValue> }`
- `Execute { id: u64, sql: String, params: Vec<DbValue> }`
- `Shutdown`

### open — eager, on the main thread

`open(data_dir, name, owner)` (confirmed decision — eager, main-thread open):
1. Validate name (unchanged), compute `<data_dir>/<name>.sqlite` (unchanged).
2. `Connection::open(path)` on the main thread (a bad path / open error → `Err`, so `s2_sqlite_open`
   can reject synchronously, unchanged).
3. `PRAGMA busy_timeout = 5000` (confirmed decision — see "Same-file contention").
4. Create the command channel; `thread::spawn(move || actor_loop(conn, cmd_rx))` — the `Connection`
   (which is `Send`) moves into the actor thread.
5. Register `(ConnHandle { cmd_tx }, owner)` under a fresh handle id; return the handle.

Rationale for eager open: `open` runs at plugin-load / map-start, not a hot gameplay frame; keeping
it eager preserves the synchronous `open`→`Result` contract and keeps the diff minimal. Fully
off-thread open (open inside the actor, resolve the `Database.open` Promise on open-completion) is a
noted deferral if a cold-open stall ever matters.

### query / execute — off-thread

To mirror the remote path's ordering guarantee, `db.rs` exposes an **owner-checked sender getter**
`get_sender(handle, owner) -> Result<Sender<Command>, String>` (the exact analog of
`sqldb::get_pool`; `mpsc::Sender` is `Clone`). The native then follows the remote block verbatim:
1. `get_sender(handle, owner)` — a wrong/absent owner → `Err("invalid db handle")`, early-rejected
   immediately with **no** RESOLVERS/PENDING_JOBS/ledger entry (unchanged semantics; not probeable).
2. `next_async_id()` → ledger `record_job` → `RESOLVERS.insert(id, entry)` → `PENDING_JOBS += 1`.
3. **Then** `sender.send(Command::Query { id, sql, params })` — the resolver is registered *before*
   the command is sent, so the actor's completion (buffered on the channel, drained on a later frame
   by the main thread) can never be processed before its RESOLVERS entry exists. Same happens-before
   as remote (`get_pool` → insert → `spawn_query`). **Return — the main thread does not block.**
4. A failed `send` (dead actor) is surfaced as an error; the native rejects the Promise and, because
   step 2 already ran, removes the just-inserted RESOLVERS entry and decrements `PENDING_JOBS`
   (balance-safe) — or, simpler, performs the `send` before step 2 is committed. The plan picks the
   exact shape; the invariant is that PENDING_JOBS/RESOLVERS stay balanced on the dead-actor path.

The actor thread runs the **same rusqlite bodies as today** (`conn.prepare`/`stmt.query` for a
SELECT, `conn.execute` + `last_insert_rowid()` for a write), wrapped in `catch_unwind`, and sends a
`DbCompletion { id, result }` back over the shared completion channel. A panicking statement sends
an `Err` completion — it does not kill the actor or leak the handle.

### close / teardown

`close(handle, owner)`: remove-if-owned in one borrow (returns the `ConnHandle`), then send
`Command::Shutdown` outside the borrow (mirrors `sqldb::close` — never a nested `borrow_mut`). The
actor drains its remaining queue, drops the `Connection`, and exits. The thread is **detached** (no
main-thread `join`), so `close` never blocks. Idempotent — an already-closed / wrong-owner handle
is a harmless `false`. Plugin teardown sends `Shutdown` to each of the plugin's ledgered handles.

## Reusing the existing async spine

The completion channel + `DbCompletion` / `DbOutcome` + `resolve_db` + the frame-drain loop are
already backend-agnostic — they carry only `QueryResult` / `ExecResult`. Plan:

- **Move the completion types into `db.rs`.** `DbCompletion`, `DbOutcome`, the process-global
  completion channel, and `try_recv_completed()` move from `sqldb.rs` into `db.rs` (the shared types
  module `sqldb.rs` already imports `DbValue`/`QueryResult`/`ExecResult` from). `sqldb.rs` imports
  them back. SQLite completions and remote completions flow through the **one channel**, drained by
  the **one existing `sqldb`/`db` completion loop** in `frame_async_drain`, resolved by the **one
  existing `resolve_db`**. Both are otherwise untouched.

- **The natives go async.** `s2_sqlite_query` / `s2_sqlite_execute` change from "run inline +
  resolve now" to the **identical async block** as `s2_db_remote_query` / `s2_db_remote_execute`:
  `next_async_id()` → `resolver_owner_tag(scope)` → ledger `record_job(id)` → `RESOLVERS.insert(id,
  entry)` → `PENDING_JOBS += 1` → send the command to the actor → `refresh_detour()` → return the
  Promise. The invalid-handle / dead-actor early-reject path makes **no** RESOLVERS / PENDING_JOBS /
  ledger entry (there is no pending job to track or tear down), exactly like the remote natives.

- `s2_sqlite_open` stays synchronous-resolving (returns/rejects the handle at open time).
  `s2_sqlite_close` keeps its shape (always resolves `undefined`; idempotent).

Inherited for free from the spine:
- **Owner-liveness guard:** a query outstanding when its plugin unloads/reloads is DROPPED by
  `resolve_db` (the `is_live` check), never resolved into a disposed/replaced context (UAF-safe).
- **Ledger teardown:** the connection handle is ledgered (`Resource::DbConn`, `record_db_conn`) and
  closed at teardown; each in-flight query is ledgered (`record_job`) so its resolver is dropped on
  unload. A completion that arrives after teardown finds no RESOLVERS entry and is dropped
  (`saturating_sub` keeps PENDING_JOBS balanced) — existing behavior.
- **Frame liveness:** `PENDING_JOBS > 0` keeps the frame detour armed so drains keep running until
  the completion arrives.

## Invariants preserved

- **No raw reference crosses to JS** — opaque integer handles only; the `Connection`/actor never
  crosses the boundary.
- **Owner-scoping** — enforced synchronously on the main thread at registry lookup, before any
  command is sent; a wrong owner is "invalid db handle" (not probeable). Unchanged.
- **FIFO per connection** — one actor thread + one ordered channel means commands execute in
  submission order, matching today's synchronous ordering exactly. (This is why the actor is chosen
  over an `Arc<Mutex<Connection>>` + shared blocking pool, which cannot guarantee FIFO for
  concurrent unawaited ops on one connection.)
- **Degrade-never-crash** — `catch_unwind` on the natives (unchanged) and around each command in the
  actor loop; a panicking statement → `Err` completion, not a dead actor; a dead actor → immediate
  error resolution without a PENDING_JOBS leak.

## New concern this introduces: same-file contention

Off-thread writes mean two connections to the **same file** (e.g. nominations + rockthevote both
`open("mapvote")`) can genuinely race → `SQLITE_BUSY`, which could not happen while everything
serialized on the game thread. Mitigation: the framework sets `PRAGMA busy_timeout = 5000` on every
connection at open (confirmed decision) — a locked write waits rather than erroring, the standard
SQLite fix. No on-disk format change; WAL is **not** forced (a plugin can still opt in, as surftimer
does at `records.ts:20`).

## Testing / validation

- **Core in-isolate tests** (`db.rs`): rewrite the existing tests (roundtrip, bad-sql,
  invalid-name, closed-handle, owner-scoping) to drive the async actor — submit a command, poll
  `try_recv_completed()` to a completion, assert — mirroring `http.rs`'s `drain_blocking` helper.
  Add: FIFO ordering (execute-then-query on one handle observes the insert), concurrent
  different-handles, actor-survives-a-bad-statement (an `Err` completion, the next command still
  works), and open-failure rejects.
- **Regression:** all SQLite consumers (cookies, zones, nominations, mapvote, clientprefs) are
  API-unchanged; verify the base-plugin DBs still round-trip live.
- **Live gate (surftimer):** cross the finish zone → confirm the record row persists **and** the
  game-frame time does *not* spike during the write (the decisive non-blocking proof, the same way
  fetch proved non-blocking by watching the frame counter advance during I/O). The author's
  mitigations (WAL, `synchronous=NORMAL`, off-hot-path batching) become optional after this change.

## Blast radius

- `core/src/db.rs` — actor rework (registry, `Command`, actor loop, completion types moved in;
  `open`/`query`/`execute`/`close` reshaped; tests rewritten).
- `core/src/sqldb.rs` — import the completion types/channel from `db.rs` instead of defining them.
- `core/src/v8host.rs` — `s2_sqlite_query`/`execute` become async (mirror the remote natives); the
  drain loop unchanged (shared channel); `resolve_db` unchanged.
- No `http.rs` change, no shim, no engine op, no ABI change → **core-only sniper rebuild**. No
  `packages/*` change → local-merge slice (no changeset).

## Deferred (do NOT build ahead)

- Fully off-thread open (open inside the actor; resolve `Database.open` on open-completion).
- Transactions (a pinned-session BEGIN/COMMIT on the actor), blobs, streaming/large results.
- Unifying SQLite onto sqlx-sqlite (the alternative not taken).
