# Slice 5: Retain cookie writes until acknowledged

**Status:** Complete for automated acceptance; Task 6 accounting and final integrated soak remain stack-wide.
**Branch:** `plugins/hardening-05-cookie-persistence`
**Parent / PR base:** `core/hardening-04-socket-lifecycle`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** plugins/clientprefs/src/plugin.ts, core/src/cookies.rs and their tests; DB adapters
only if a transaction primitive is necessary. **Test:** plugins/clientprefs/src/plugin.test.mjs.

**Boundary:** Separate connection cache eviction from persistence ownership. Pending writes
carry SteamID, cookie name, value, and monotonic revision; retry order cannot restore old values.

- [x] Test a database rejection on the first and middle write, delayed query versus a newer
  local change, reconnect during retry, and offline updates for the same key while a write is
  in flight. Assert the newest accepted value eventually reaches storage.
- [x] Move disconnected/offline dirty values into a host-owned pending outbox before clearing
  connection state. Keep it across clientprefs reload; acknowledge only the written revision.
- [x] Drain bounded batches with capped exponential retry delay (100ms initial, 5s maximum).
  Coalesce unsent updates by key without acknowledging newer updates on an older completion.
  Do not use second-resolution wall time as the ordering authority.
- [x] Define outbox capacity and overload reporting with slice 6's admission policy. Until that
  slice, expose counters and preserve values; do not silently drop accepted writes on failure.
- [x] Test plugin reload during an in-flight batch and document shutdown/crash durability limits.
  Run plugin/core tests and a live SQLite contention/recovery scenario.

## Evidence required before completion

- [x] Record the regression test and its failure on the parent implementation.
- [x] Record implementation commits and passing focused checks.
- [x] Record applicable full-gate and live-server results, with environment limitations stated.
- [x] Review against the parent and carry the implementation into Task 6; retain exact bases for later in-progress descendants.
- [x] Set status to complete only when this slice's required gates pass.

## Implementation and compatibility

Implementation `83365c5`, composed retry proof `c6f3c28`, and map-integration test `7910152`
replace the transient retired/offline queues with one process-stable bounded outbox. Acceptance
precedes cache mutation; disconnect only evicts connection state. Immutable leased attempts and
coalesced ready values carry revisions and earliest unsatisfied fence coverage. Owner-generation
ACKs cannot remove newer writes; retries preserve ownership and versioned SQLite upserts reject
late older writes. Same-process core reset reclaims leases while retaining accepted values and
allocators. Process exit/library destruction is not crash-durable for unacknowledged writes.

`Cookies.set` and `setAuthId` return boolean admission results; SDK minor changeset and first-party
consumers are updated together. See [operator contract](../../../cookie-persistence.md), including
the required matched-addon stopped-server restart for first adoption of the versioned writer.

Provisional limits: 4,096 retained versions, 4 MiB retained outbox bytes, 64 KiB per write, four
live leases and 256 KiB per pump batch. These do not bound legacy cache copies or detached old DB
actors across plugin generations; Task 6 supplies that lifetime/admission work. No global four-DB-
operations or whole-process memory bound is claimed for this slice.

## Verification and live recovery

- Independent Sol review and scoped re-review approved specification and quality after adding
  the rollout wording and composed first/middle failure→retry→newest-storage test.
- Full combined core: **702 passed** on macOS and **702 passed in 12.13 seconds** on Linux.
  Full Linux `ci-native.sh`, release build, shim/game symbol and sanitizer gates passed.
- Full JS gate passed with the installed Docker CLI on PATH; **584 SDK tests**, 5 clientprefs
  tests including real SQLite int64 conditional-upsert/retry storage, all typechecks, and base
  archive builds passed. Fourteen normal base archives were installed; disabled plugin build
  checks also passed. The first missing-Docker result was a PATH error, not absent Docker.
- Real native SQLite barrier coverage forces an old actor to complete after unload, shutdown/init
  and a replacement write. Cookie/session tests cover same-account reconnect, newer pending
  revisions, capacity rejection without mutation, stale callbacks and load fences. No authenticated
  human reconnect was performed in the live bot-only environment.
- Installed only on `s2script-cs2-hardening:27016`, with matched core/clientprefs archives after
  stopping the server. Core SHA256 `18507fd89785a11ddf07a6a2e96cd6eacbc28a1fda5c94a7ce6ac768bb441c54`;
  shim SHA256 `0f0c601590168707d7b44e7207f23ffe8478299d50eced60ebd31967ee6fe822`.
- Held `BEGIN IMMEDIATE` on the isolated `clientprefs.sqlite` for 35 seconds. The actual SDK
  admitted values A, B and C; clientprefs reloaded while locked, and the third value was accepted
  afterward. The writer reported three locked-database retries. While blocked, the newest fence
  remained incomplete and payload ownership remained charged. After release, SQLite stored
  `locked-C-latest` at revision 3; the fence completed and ready/leased/outbox bytes returned to
  zero. All 14 base plugins and three disposable probes remained running.

Detailed evidence: controller scratch `task-5-report.md`, `task-5-review.md`, named test logs,
`cookie-live-evidence/`, and `/tmp/s2script-cookie-live-{lock,writes,recovery}.log`.
