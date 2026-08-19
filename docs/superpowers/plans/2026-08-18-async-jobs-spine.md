# Deep async Jobs spine — implementation plan

Spec: `docs/superpowers/specs/2026-08-18-async-jobs-spine-design.md`.

One atomic slice. Behavior-preserving extraction of the async Jobs spine from `v8host.rs`. No public/native change. No changeset.

## Global constraints

- Engine halves (`http.rs` / `ws.rs` / `net.rs` / `db.rs` / `sqldb.rs`) keep moving only plain ids/data across threads. They never hold V8 handles.
- Two begin paths stay distinct: immediate `begin_job` vs mint-then-`commit_job`.
- Drain phase order is the existing order. Connect Promises settle before the one microtask checkpoint; WS/net drops wait until after it.
- Narrow `v8host` adapter for isolate/context/liveness/Job-ledger. Do **not** extract a shared owner-context type with Dispatch (this branch is from `main` and has no `dispatch.rs`).
- Callback-timer maps and engine adapters stay out of `jobs.rs`.
- Id allocator is process-global and is never reset.

---

### Task 1 — `core/src/jobs.rs` + `mod jobs`

Add the module and register it in `core/src/lib.rs`.

Owns: `next_id`, `ResolverEntry`, resolver map, `pending`, `insert_timer_resolver`, `begin_job`, `commit_job`, `take_resolver`, `complete_job`, `drop_if_present`, `settle_if_live` / `resolve_undefined`, `reset_resolvers` / `reset_pending`.

Adapter (in `v8host`, named `jobs_*`): `jobs_owner_tag`, `jobs_record_job`, `jobs_owner_is_live`, `jobs_clone_plugin_context`.

### Task 2 — retarget callers

- Timers: `next_id` + `insert_timer_resolver` (no pending increment). Callback `s2_timer_create` only needs `next_id`.
- Immediate `begin_job`: `threadSleep`, `http::__s2_fetch`, `s2_ws_connect`, `s2_net_tcp_connect`, `s2_net_udp_bind`. WS/net still ledger `WsConn`/`NetConn` separately.
- Mint-then-`commit_job`: `s2_sqlite_query` / `_execute` (mint, submit, commit on `Ok`); remote query/execute (pool resolve is the early-reject gate, then mint + commit + spawn).
- Payload builders (`resolve_fetch` / `resolve_ws_connect` / `resolve_net_connect` / `resolve_db`) stay in `v8host` and call `settle_if_live`.
- `frame_async_drain` becomes named phases: timers, threadpool, fetch, DB, WS connects, net connects; then checkpoint; then deferred drops; then `refresh_detour` / rejection flush / crash sweep / `finalize_loading_plugins`.
- `Resource::Job` teardown → `drop_if_present`. Timer teardown → `take_resolver` (no pending decrement). Connection/DB resources unchanged.
- Process-singleton slots `RESOLVERS` / `PENDING_JOBS` keep their historical names and positions; reset closures live in `jobs.rs`. The id allocator is not registered.

### Task 3 — tests

In `jobs.rs` (no isolate):

- Ids monotonic across singleton reset.
- Missing/double `complete_job` / `drop_if_present` do not undercount (no resolver present).

In the existing `v8host` harness:

- Missing + double complete of a real in-flight `threadSleep` job.
- Generic job unload mid-flight: capture the in-flight id, unload, inject a zero-work pool completion for that id, poll until it lands, drain — pending stays 0, disposed context is not enterable.
- WS connect unload mid-flight against `spawn_local_ws_echo_server` (completing handshake): unload before the first drain, poll until the late Connected/Closed is consumed, drain — pending/resolver stay 0, context stays disposed. Does not wait on the 10s handshake timeout.
- SQLite query unload after `commit_job` and **before the next drain** + late complete is a no-op. Document why a live actor-race is not the test: the in-process SQLite actor finishes in microseconds; the Jobs-visible in-flight window (map entry + pending, no drain yet) is the deterministic equivalent.

Keep existing fetch/ws/net/db, connect-before-checkpoint, immediate-close, stale-completion, detour, and microtask-reentry tests.

### Task 4 — docs

Focused design + this plan, dated 2026-08-18. Append a concise completion entry to `docs/PROGRESS.md`. No `CLAUDE.md`, no changeset, no unrelated cleanup.

Parent agent: commit/push/draft PR, then `cargo test -p s2script-core` and `make ci-native`. Live CS2 gate is not required unless drain ordering or public/native behavior changes.
