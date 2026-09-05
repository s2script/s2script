# Task 6 candidate report

Status: implementation frozen for independent review and integrated Linux/live gates.
This report does not claim the live saturation/recovery soak has passed.

- Worktree: `/private/tmp/s2script-runtime-hardening`
- Branch: `core/hardening-06-async-budgets`
- Exact base: `ce544ba62fe2c678b2a702dab0e0a05c30289932`
- Exact implementation HEAD: `7de25aef1badb1f72be8178c07a9b84f92bf7d79`
- The following report-only commit changes no verified application source.
- Operator policy, numerical defaults/rationale, metrics schema, and memory exclusions:
  [`docs/ASYNC_LIMITS.md`](../../../docs/ASYNC_LIMITS.md).

## Implemented contracts

- One process-stable domain provides atomic count/byte admission and RAII leases.
  Captured owner generations, producer cancellation, queued completions, and staged events
  survive resolver removal and isolate reinitialization without resetting accounting.
  Publication and shutdown draining share a gate, preventing a canceled old producer from
  publishing into the replacement isolate. Host producers are explicitly charged.
- Native input measurement/reservation precedes owned strings, queued input, resolver
  registration and ledger insertion. This includes SQL parameters, HTTP options/headers,
  socket sends, SQLite actor creation, and remote pool configuration retention.
  SQLite open remains synchronous as before; the actual actor owns the lifetime permit.
- Failure space is reserved at admission. Socket connect/terminal diagnostics have additional
  reservations independent of data pressure. Incoming socket producers await capacity with
  control interruption; accepted outbound items retain both local/global permits through
  the write and preserve the Task 4 graceful-close FIFO contract.
- HTTP and SQL result growth is nonblocking. Builders that cannot grow release partial
  output and use the pre-reserved failure path. HTTP streams raw chunks and bounds retained
  headers plus final UTF-8, including replacement expansion. SQL streams rows, checks rows,
  columns and normalized cell bytes, and borrows driver text before bounded copies.
- SQLite actors and remote pool clones/close futures own their lifetime permits. Old actor
  work and its input remain globally charged after plugin reload until actual exit. Remote
  pool metadata has a separate aggregate byte partition. An idle pool permits at most four
  backend connections; that is not the global DB task cap.
- Timer admission is finite. The minimal linear `due_limited(now, frame, limit)` preserves
  stable eligible insertion order and retains everything at limit zero. It is compatible
  with the separately reviewed Task 8 indexed kernel, which has not been integrated here.
- Persistent source and connection cursors share finite poll/delivery/byte budgets across
  timers, completions and HOST-free cookie/socket callbacks. Alternating busy pre/post turns
  preserve progress at a one-item frame limit. An oversize delivery is allowed alone.
  Connect settlement precedes the single checkpoint and subsequent socket callbacks.
  Last-terminal callbacks retain a separate pending-microtask obligation after socket
  retirement, keeping the detour installed for their promise continuations.
- Cookie admission uses Task 5's existing single outbox; revision, stable epoch, ACK,
  `covers_from`, fence and account-FIFO contracts remain. Successful ACKs evict acknowledged
  offline cache keys only when there is no newer same-key write. Active snapshots remain
  separate, and many-account ACK history plateaus.
- `__s2_async_stats(): string` returns documented JSON producer/retained-byte, queue,
  staging, actor/pool/socket/timer, cache, rejection and drain-duration gauges.
- Public WS/TCP/UDP sends return boolean acceptance through native/prelude/SDK consumers.
  `threadSleep` now correctly declares its existing `Promise<void>` runtime result.
  Timer overload is explicit; an SDK minor changeset records the declaration changes.
  The two source-derived native-name lint scanners now accept both single-line and
  multiline Rust registrations; existing preludes exercise both forms in the JS gate.

## Final-source verification

All commands below completed against the exact implementation source committed above,
after normalization, formatting, and the terminal-microtask fix. No implementation
changes followed these final runs.

| Gate | Result | Evidence |
| --- | --- | --- |
| `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=$PWD/.superpowers/sdd/2026-09-04-runtime-hardening/macos-test-linker cargo test -p s2script-core` | 720 passed, 0 failed, 1 intentionally ignored; 8.81 s test phase | `/tmp/s2-task6-final-core.log` |
| Same linker environment, `bash scripts/test-async-pressure.sh` | 1 passed, 0 failed, 720 filtered; ignored test selected explicitly in a fresh tiny-policy process | `/tmp/s2-task6-final-pressure.log` |
| `PATH=/Applications/Docker.app/Contents/Resources/bin:$PATH bash scripts/ci-js.sh` | Entire JS gate passed, including SDK tests, generated-code gates, lint, plugin/example typechecks, workspace build, lifecycle tests, and final Docker gate | `/tmp/s2-task6-final-js.log` |
| `git diff --check` | Passed | Run immediately before implementation commit |

The ordinary suite intentionally excludes the process-policy test; `ci-native.sh` now
invokes its dedicated script after the ordinary core suite. Local `ci-js.sh` skips the
destructive `npm ci` lockfile reinstall when `CI` is unset; no dependency changes were made.
The ordinary Rust build emits 13 existing-style lint warnings; these are not hidden or
treated as a clean warning-free build.

Pressure coverage includes atomic count/byte boundaries, bounded worker and SQLite queues,
producer barriers across unload/shutdown/reinit, rollback without ledger growth,
interleaved partial result builders, malformed UTF-8 expansion, HTTP declared/chunked/header
limits, SQL row/cell/column limits, full socket data with independent terminal controls,
retained bytes through staging/write/materialization, bounded timer admission/selection,
cookie account-history plateau, and fair mixed progress with `frame_items=1`.

Two real-isolate regressions specifically exercise JS reentry: a SQL result's inherited
`then` getter observes its still-retained result bytes and schedules a timer while hostile
column/index prototype setters remain uncalled; a final socket close schedules a Promise
continuation that runs on the next retained checkpoint. The public WebSocket round trip
also asserts `true` on accepted send and `false` immediately after close.

Observed development failures were corrected and rerun, rather than omitted: initial
unbounded admission failed its edge test (`/tmp/s2-task6-red.log`); the final-terminal
continuation test first failed its missing checkpoint-obligation assertion
(`/tmp/s2-task6-terminal-red.log`). Earlier full runs exposed watch-control consumption
and process-domain test assumptions, fixed before the final pass. A formatting-induced
single-line native scanner failure was fixed in both configs. An earlier JS invocation
lacked the Docker executable on PATH; Docker is available, and the final full command
above uses the corrected PATH and passes.

## Limits and remaining gates

- The default capacities are conservative initial policy choices based on baseline runtime,
  HTTP and SQL limits, not production measurements. A socket retains a job/input/failure
  reservation for its full lifetime; this intentionally trades capacity for simple bounded
  connect/terminal ownership. The metrics make that attribution visible.
- Counts/bytes cover application-owned retained payloads and conservative metadata, not
  whole RSS. V8-retained objects, allocator overhead, upstream SQL/HTTP/WS decoding,
  SQLite caches, TLS and kernel buffers are outside the application queue partitions.
- A JS callback/materialization or a microtask checkpoint may exceed the soft duration.
  The current timer kernel is a finite linear scan; no hard Post wall-clock claim is made.
- This candidate does not integrate Task 8's timer index or Task 9's independent loader.
  The central domain remains owned by this slice; later loader integration uses adapters.
- Root owns independent Astra concurrency review, Linux native/shim integration, live CS2
  saturation/recovery soak, and any subsequent fixes. Those are pending at this commit.
  Remote SQL driver branches compile in the core gate; this subtask did not run live
  MySQL/Postgres services or claim their production behavior from SQLite tests.

