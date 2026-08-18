# Deep async Jobs spine — design

**Status:** Implemented (2026-08-18).
**Audience:** core maintainers.
**Builds on:** tick-integrated async (Slice 2, 2026-06-30); fetch / ws / net / SQLite / remote SQL async-network slices.

---

## 1. Rule

Every in-flight Promise that crosses a thread (timer, threadpool, fetch, DB, WebSocket connect, net connect) is one Jobs fact: a process-global id, a tagged resolver, and — for jobs, not timers — a pending count. Liveness is owner-generation. A late completion after unload is a no-op. The id allocator never resets.

`v8host` stays the isolate/lifecycle owner. Feature engine halves keep moving only plain ids/data across threads.

## 2. Mechanism

`core/src/jobs.rs` owns:

- `NEXT_ASYNC_ID` — process-global, monotonic, never reset on shutdown (a resettable counter is what made a stale threadpool completion resolve a later test's unrelated Promise).
- `ResolverEntry` — `owner: Option<(plugin id, generation)>` + `Global<PromiseResolver>`.
- The resolver map and the pending-job count.
- Timer resolver insertion (id + map; **no** pending increment).
- Immediate `begin_job` (fetch / `threadSleep` / ws / net).
- Mint-then-`commit_job` (SQLite / remote DB) so an immediate reject creates no ledger entry, map entry, pending count, or detour flicker.
- `take_resolver` (timers / timer teardown), `complete_job` / `drop_if_present` (jobs / Job teardown) — decrement pending only when a resolver was actually removed.
- `settle_if_live` / `resolve_undefined` — the single owner-generation resolve-or-drop protocol.
- Process-singleton reset of the map and the pending count (not the id allocator).

Isolate / context / liveness / Job-ledger facts arrive through a narrow `v8host` adapter (`jobs_owner_tag`, `jobs_record_job`, `jobs_owner_is_live`, `jobs_clone_plugin_context`). Jobs never names `HOST`, `PLUGINS`, or `REGISTRY`. This is a Jobs capability, not a shared owner-context type with Dispatch (Dispatch is a separate slice and is not on this branch).

## 3. Drain order (load-bearing, unchanged)

`frame_async_drain` is an explicit phase list. Existing order:

1. Timers (`take_resolver` — no pending decrement)
2. Threadpool (`complete_job`)
3. Fetch (`complete_job`)
4. DB (`complete_job`)
5. WebSocket connects (`complete_job`)
6. Net connects (`complete_job`)

Connect Promises settle **before** the single microtask checkpoint so `.then` can subscribe in the same frame. WS/net `drop_conn` runs **after** that checkpoint. `refresh_detour`, pending-rejection flush, crash sweep, and plugin-load finalization keep their historical positions after the HOST borrow is released.

Callback-timer maps (`TIMER_CBS` / `TIMER_KILLED`) and engine adapters stay out of `jobs.rs`.

## 4. Teardown

`Resource::Job` teardown calls `jobs::drop_if_present`: pending decrements only if a resolver was removed. A stale late completion then finds no entry and does not decrement again, and never enters a disposed context. `Resource::WsConn` / `NetConn` / DB connection teardown stay separate.

## 5. Acceptance

- Existing fetch / ws / net / db, connect-before-checkpoint, immediate-close, stale-completion, detour, and microtask-reentry tests stay green.
- New regressions: generic-job unload mid-flight (captured id + injected zero-work late complete); WS connect unload mid-flight against the local echo handshake (late Connected/Closed consumed, not a never-accepted listener); SQLite query unload before the next drain (deterministic Jobs-visible window — racing the in-process actor is inherently nondeterministic); late completion after unload leaves pending at zero and does not enter a disposed context; missing/double completion does not undercount; ids remain monotonic across singleton reset.

## 6. Out

A shared owner-context abstraction with Dispatch. Moving callback timers or engine adapters into `jobs.rs`. Changing public/native behavior. A changeset. Live CS2 gate (in-isolate suites prove this slice).
