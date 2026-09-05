# Runtime Hardening Implementation Plan

> **For agentic workers:** The user authorized implementation with subagents and model choices for speed, quality, and cost. Use superpowers:subagent-driven-development in the current session. Execute all slices with independent review; this is not a scheduled automation.

**Goal:** Complete all six correctness/resource fixes and all four performance/organization improvements from the September 4 review.

**Architecture:** Repair ownership and lifetime behavior first, then introduce bounded async processing and indexed hot paths. Preserve engine/V8 access on the game thread and isolate mechanical host extraction from behavior changes.

**Tech Stack:** Rust, V8 149.4.0, Tokio, C++, TypeScript, Node.js, SQLite/sqlx, Linux Docker CS2 gate.

**Spec:** [Runtime hardening and performance scope](../specs/2026-09-04-runtime-hardening-design.md).

## Global constraints

- The core owns every engine touchpoint.
- Core is engine-generic; games are packages. Dependencies point one way: game → core, never core → game.
- Never expose a raw pointer or raw cross-plugin reference across time.
- The ledger is the teardown authority.
- Degrade per-descriptor, never crash globally.
- A slice is one branch and one PR.
- Preserve the remaining compatibility, gate, and live-environment constraints in the spec.
- Keep production changes out of the planning commit. Do not mix unrelated cleanup into fixes.
- Treat tests below as required scenarios; implement them in the listed existing suites or new
  test files and wire new suites into the script gates in the same slice.

## Execution order and tracking

| Order | Slice | Dependency | Completion evidence | Status |
| --- | --- | --- | --- | --- |
| 1 | Active-resource ledger | Baseline | Retained entries return to active count; native and live gates pass | Complete |
| 2 | Subscription cleanup | 1 | Both indexes return to baseline after churn | Complete |
| 3 | Connection-safe Client | 2 | Reused-slot actions and notifications rejected | Automated acceptance complete; human checks limited |
| 4 | Socket terminal handling | 1 | Exactly-once cleanup under failure and cancellation | Automated acceptance complete |
| 5 | Reliable cookie persistence | 3 | Failure/retry/reconnect tests preserve newest values | Automated acceptance complete |
| 6 | Async limits and frame batches | 4, 5 | Saturation stays bounded with fair progress | Implemented/reviewed; final soak pending |
| 7 | Indexed hook lookup | 2 | Equivalent dispatch, measured scaling improvement | Implemented/reviewed; human viewer gate pending |
| 8 | Indexed timer scheduling | 1, 6 | Timing parity and measured scaling improvement | Implemented/reviewed; final live gate pending |
| 9 | Background file preparation | 4, 6 | No periodic reads/parsing on frame thread | Implemented/reviewed; Linux/live gate pending |
| 10 | Host module extraction | 1–9 | Behavior/ABI parity and integrated soak | Implemented/reviewed; final integration gates pending |

The sequential order also avoids overlapping edits to v8host.rs. Dependency entries describe
technical prerequisites. The user explicitly requested a dependent Git stack. Create each branch from its listed parent,
and rebase descendants whenever an ancestor changes. Merge from the bottom upward.

## Subagent workflow and model choices

Use a fresh implementer for each slice and a separate reviewer after its commit. Run one production
implementation at a time because the lifetime adapters overlap; parallelize bounded read-only
investigation, test-environment work, and benchmark preparation. The controller owns integration,
external gates, and restacking. A review finding returns to the implementer, followed by scoped
re-review before advancing.

| Work | Default model | Reason |
| --- | --- | --- |
| Mechanical inventory, lifecycle maps, baseline harness preparation | GPT-5.6 Luna | Bounded tasks with independently checked output |
| Focused fixes, ordinary reviews, environment diagnosis | GPT-5.6 Sol | Efficient implementation and verification |
| Cross-layer lifetime/ABI changes, concurrency design, final audit | GPT-6 Astra | Reserve deeper reasoning for consequential boundaries |

Escalate based on the code's risks or an unresolved finding, rather than using the most expensive
model for every task. Commit each slice's tests and evidence with its implementation; the final
review covers the whole stack, including the integrated live soak and measured performance results.

## Local Git stack

The user explicitly requested this dependent stack, overriding CLAUDE.md's usual prohibition
on splitting dependent work across PRs. These are planning commits: no runtime fix is implemented
by creating the branches. Each branch adds its own scoped checklist; slice 1 also carries this
workflow and the shared scope document. Branches are local until explicitly published.

| Slice | Branch | Parent / eventual PR base |
| --- | --- | --- |
| 1 | `core/hardening-01-ledger` | `main` |
| 2 | `core/hardening-02-channels` | `core/hardening-01-ledger` |
| 3 | `core/hardening-03-client-identity` | `core/hardening-02-channels` |
| 4 | `core/hardening-04-socket-lifecycle` | `core/hardening-03-client-identity` |
| 5 | `plugins/hardening-05-cookie-persistence` | `core/hardening-04-socket-lifecycle` |
| 6 | `core/hardening-06-async-budgets` | `plugins/hardening-05-cookie-persistence` |
| 7 | `core/hardening-07-hook-index` | `core/hardening-06-async-budgets` |
| 8 | `core/hardening-08-timer-index` | `core/hardening-07-hook-index` |
| 9 | `core/hardening-09-loader-worker` | `core/hardening-08-timer-index` |
| 10 | `refactor/hardening-10-host-modules` | `core/hardening-09-loader-worker` |

### Implement, restack, review, merge

1. Implement on the branch for the first incomplete slice. Its scoped checklist lives under
   `docs/superpowers/plans/runtime-hardening/`; this full workflow remains the shared scope.
2. Before changing any ancestor, record all existing stack tips. After its implementation
   commits, rebase each descendant in order with `git rebase --onto <new-parent-tip>
   <old-parent-tip> <child-branch>`. Use the recorded old tip, not a parent ref that has moved.
   Resolve and review conflicts at each level; never discard descendant changes to restack.
3. Run the slice gates against its parent and integrated gates against the stack tip. Each PR
   diff contains only that slice's planning and implementation changes relative to its parent.
4. When publishing is requested, push the named branches and open PRs using the table's bases.
   After a published restack, use `--force-with-lease` only under the available authorization.
5. Merge slice 1 into main first. After squash merge, rebase the remaining series onto updated
   main, dropping the old merged-parent history using its recorded tip; retarget the new bottom
   PR to main. Repeat bottom-up. A squash merge requires this rebase, not just changing PR bases.
6. Update status/evidence in each slice's own checklist as work completes. Consolidate the
   overall status table after restacking so shared-file edits do not create unnecessary conflicts.

## Baseline and common slice loop

- [ ] Read AGENTS.md, CLAUDE.md, README.md, docs/BUILDING.md, and the spec. Record the starting
  commit, working-tree state, OS, compiler versions, and available Docker/live-server environment.
- [ ] Preserve unrelated changes. Use the pre-created branch for each implementation slice; use an isolated worktree when needed,
  following the repository's `<area>/<terse-change>` naming convention.
- [ ] Run the existing suites on a supported environment and record pre-existing failures.

```bash
npm --prefix packages/sdk test
cargo test -p s2script-core
make ci
```

- [ ] Capture comparable Linux baselines: 0/10/1,000/10,000 pending timers; 1/100/1,000 hook
  subscriptions; 100,000 create/complete operations; 1,000 owner reloads; and async bursts with
  a slow consumer. Record p50/p95/p99 frame-drain time, maximum frame time, queue depth/bytes,
  active/retained resource counts, native RSS, and V8 heap separately.
- [ ] Extend tools/s2bench/src/plugin.ts for live workloads and add deterministic engine-free
  counters/tests where timing is not needed. Record results under
  docs/benchmarks/2026-09-runtime-hardening/ with commit IDs and workload parameters.
- [ ] For each slice: add the regression test, run it to demonstrate the old failure, implement
  the change, rerun the focused suite, run applicable full gates, review the diff, and record
  evidence. Fix failed checks before marking the slice complete.
- [ ] Include contract changes, affected consumers, required changesets, and a docs/PROGRESS.md
  entry in the corresponding slice. Create one reviewable PR if PR execution is requested;
  merging and deployment follow the authorization available during execution.

## Slice 1: Make the ledger track active resources

**Modify:** core/src/plugin.rs, core/src/jobs.rs, core/src/v8host.rs and resource release paths
in core/src/{db,sqldb,ws,net,interfaces}.rs. **Tests:** plugin.rs, jobs.rs and the existing
in-isolate timer/job tests in v8host.rs.

**Boundary:** Resource acquisition records ownership; completion/disposal releases the same
resource from the matching owner generation. Unload consumes surviving entries in reverse order.

- [ ] Add tests for 100,000 one-shot timer completions, 100,000 job completions, explicit disposal
  followed by unload, and late completion after owner reload. Assert retained ledger entries
  equal active resources, not total historical acquisitions.
- [ ] Replace duplicated append-only vectors with an ordered active-resource representation.
  Use a monotonic acquisition sequence plus an index for removal; do not replace growth with
  an O(history) scan on every completion or permanent tombstones.
- [ ] Add a generation-aware release operation and call it on timer completion/cancellation,
  job completion, connection close, hook disposal, and interface subscription/import release.
  Preserve any acquisition multiplicity that existing consumers depend on.
- [ ] Test repeating timers retain one entry while active, self-kill releases it, and unload
  remains exactly once in reverse acquisition order. Verify teardown does not double-borrow
  REGISTRY while releasing entries from a registry already being removed.
- [ ] Run core tests and compare churn/teardown measurements against baseline.

## Slice 2: Prune subscription indexes

**Modify:** core/src/channels.rs, core/src/multiplexer.rs, core/src/owner_stores.rs if its
interface needs adjustment. **Tests:** channels.rs and owner-disposal integration tests.

**Boundary:** Removal returns emptied channel names for engine unsubscribe, even when the
descriptor itself has already been removed. Snapshot semantics remain unchanged.

- [ ] Pin the shared-channel case: persistent owner A, 1,000 subscribe/unload cycles for B;
  assert one live subscriber and one reverse ID mapping.
- [ ] Pin unique-channel churn: create and dispose 1,000 channels; assert zero subscribers,
  zero reverse mappings, and zero retained empty descriptors.
- [ ] Store sufficient ownership in the reverse index or return exact removed IDs from the
  descriptor. Remove only the disposed subscriptions' mappings, including remove_by_owner_on.
- [ ] Remove empty descriptors after collecting engine unsubscribe notifications. Cover
  duplicate disposal, unknown IDs, and a handler subscribing/unsubscribing during dispatch.
- [ ] Run focused channels tests, core tests, and owner teardown/reload tests.

## Slice 3: Bind Client to a connection lifetime

**Modify:** core/src/client.rs, core/js/prelude.js, packages/sdk/clients.d.ts,
core/src/cookies.rs, plugins/clientprefs/src/plugin.ts; inspect shim/src/s2script_mm.cpp's
client lifecycle bookkeeping and games/cs2/js/pawn.js consumers.
**Tests:** client.rs, cookies.rs, v8host.rs; create plugins/clientprefs/src/plugin.test.mjs.

**Boundary:** A connection token belongs to the host's client-liveness books, not SteamID alone.
Preserve the departing identity through its disconnect callback, then invalidate it. Account
for clients already present at runtime initialization and engine map-transition semantics.

- [ ] Add tests where A disconnects, B occupies the same slot, and a saved Client attempts
  kick/chat/command/voice operations. None may affect B; isValid must be false for the old handle.
- [ ] Add tests for the same SteamID reconnecting, late cookie query completion, a cached event
  queued before slot reuse but dispatched afterward, and disconnect-handler identity access.
- [ ] Mint a host connection generation on a new connection. Capture it when constructing Client
  and gate getters and actions against it. Document stale-access return values consistently
  with the existing API's safe-access conventions.
- [ ] Carry slot plus connection generation through the pending cookie notification queue and
  recheck at dispatch. Drop stale loads before cache mutation, not only before notification.
- [ ] Inspect generated/manual Player wrappers and retained Client uses; update affected callers
  and classify SDK/host API compatibility together. Do not hand-edit generated files.
- [ ] Wire the new plugin test into scripts/ci-js.sh. Run core, SDK, plugin typecheck, and live
  disconnect/reconnect tests; confirm normal connect, disconnect, and map-change behavior.

## Slice 4: Unify socket termination and cancellation

**Modify:** core/src/net.rs, core/src/ws.rs, core/src/http.rs only if its spawn API must return
an owned cancellation handle; connection-ledger adapters in core/src/v8host.rs.
**Tests:** net.rs, ws.rs and existing in-isolate network tests.

**Boundary:** One connection produces at most one terminal signal. Explicit owner teardown
removes state without dispatching into the unloaded context. Cancellation is not queued behind data.

- [ ] Add injected writer tests for immediate write failure and a write future that never
  completes. Assert error then close on failure and bounded worker termination on owner unload.
- [ ] Add connect cancellation, TCP connect timeout, peer-close/local-close races, and
  Connected followed immediately by Closed tests. Preserve subscription-before-close ordering.
- [ ] Route write errors through the same terminal path as read errors. Use cancellation/abort
  handles or a separate cancellation signal selected while connecting and writing. Put a finite
  deadline on graceful close and abort the transport when that deadline expires.
- [ ] Retire connection state, handlers, and active-ledger entries exactly once. Ensure stale
  completions after reload cannot settle a new connection's promise.
- [ ] Run local loopback integration tests and core tests. Check no worker/connection count
  growth after 1,000 connect/fail/close cycles.

## Slice 5: Retain cookie writes until acknowledged

**Modify:** plugins/clientprefs/src/plugin.ts, core/src/cookies.rs and their tests; DB adapters
only if a transaction primitive is necessary. **Test:** plugins/clientprefs/src/plugin.test.mjs.

**Boundary:** Separate connection cache eviction from persistence ownership. Pending writes
carry SteamID, cookie name, value, and monotonic revision; retry order cannot restore old values.

- [ ] Test a database rejection on the first and middle write, delayed query versus a newer
  local change, reconnect during retry, and offline updates for the same key while a write is
  in flight. Assert the newest accepted value eventually reaches storage.
- [ ] Move disconnected/offline dirty values into a host-owned pending outbox before clearing
  connection state. Keep it across clientprefs reload; acknowledge only the written revision.
- [ ] Drain bounded batches with capped exponential retry delay (100ms initial, 5s maximum).
  Coalesce unsent updates by key without acknowledging newer updates on an older completion.
  Do not use second-resolution wall time as the ordering authority.
- [ ] Define outbox capacity and overload reporting with slice 6's admission policy. Until that
  slice, expose counters and preserve values; do not silently drop accepted writes on failure.
- [ ] Test plugin reload during an in-flight batch and document shutdown/crash durability limits.
  Run plugin/core tests and a live SQLite contention/recovery scenario.

## Slice 6: Bound async admission and frame processing

**Create:** core/src/async_limits.rs for policy/counters. **Modify:** core/src/{lib,http,ws,net,
db,sqldb,async_rt,jobs,v8host,cookies}.rs and affected SDK declarations/prelude adapters.
**Tests:** each engine's saturation tests and in-isolate frame ordering tests.

**Boundary:** Count bytes before copying/enqueueing where possible. Reserve capacity before
ledgering a job. Never block the game thread on a bounded synchronous sender.

- [ ] Add deterministic tests with tiny injected capacities: admission rejection creates no job
  or ledger leak; queues plateau; control/close signals survive a full data queue; one busy
  source cannot prevent another source's completion or timer progress.
- [ ] Introduce configurable policy values in one module. Establish shipping defaults from the
  baseline workloads; test code injects its own small values rather than relying on defaults.
  Record the chosen count/byte limits and their rationale in the slice's PR and operator docs.
- [ ] Bound per-owner outstanding jobs, per-connection outbound data, global inbound/completion
  bytes, database result rows/bytes, and the cookie outbox. Oversized results become named
  errors before unbounded materialization; streaming producers await capacity off-thread.
- [ ] Preserve existing return contracts: send returns false when not accepted; rejected async
  admission settles with a named error. Any API without an overload signal must add one with
  appropriate compatibility/version handling. Already accepted cookie writes remain owned.
- [ ] Replace drain-until-empty loops with fair count/byte-limited batches and a soft elapsed-time
  stop between items. Leave remaining work queued for subsequent frames. Preserve ordering of
  connect settlement, microtasks, message delivery, terminal delivery, and registry pruning.
- [ ] Export queue depth, queued bytes, rejections, and drain-duration metrics for the dev
  harness. Explain that one JS callback/microtask checkpoint can exceed the soft budget.
- [ ] Run sustained producer/slow-consumer tests, core gates, and a live saturation/recovery soak.

## Slice 7: Index entity hooks

**Modify:** core/src/sdkhooks.rs, core/src/sdkhooks_transmit.rs and corresponding tests;
tools/s2bench/src/plugin.ts. Keep shim/src/s2script_mm.cpp's visibility merge semantics.

**Boundary:** Dispatch addresses an entity host identity and hook kind. Per-kind active counts
and entity membership are updated with the subscription store, not rebuilt per dispatch.

- [ ] Pin registration order, result folding, disabled handlers, owner unload, entity removal,
  slot/serial reuse, and subscribe/unsubscribe during a callback against the existing behavior.
- [ ] Replace full HOOKS scans with keyed ordered subscriber collections and reverse indexes
  needed for owner/entity cleanup. Keep snapshots detached before entering JS.
- [ ] Maintain per-kind entity sets/counts so SetTransmit can enumerate just its hooked entities.
  Preserve hide-only AND merging with native visibility rules.
- [ ] Benchmark 1/100/1,000 total hooks while keeping addressed subscribers constant; vary
  viewers and hooked entities separately. Report lookup work and frame cost separately from JS.
- [ ] Run core and live damage/SetTransmit tests. Accept only behavioral parity with improved
  scaling and no material small-workload regression across repeated comparable runs.

## Slice 8: Replace linear timer scheduling

**Modify:** core/src/async_rt.rs and timer integration/tests in core/src/v8host.rs.
**Boundary:** Preserve push/remove/due behavior or update all callers atomically. Ledger release
from slice 1 remains authoritative; scheduling indexes must not create a second lifetime owner.

- [ ] Add a deterministic oracle test comparing old and new schedules over interleaved
  insertion, time/frame advancement, cancellation, equal deadlines, and repeated cancellation.
- [ ] Use a deadline min-heap with stable sequence ordering and frame-target buckets. Add an
  ID index for cancellation; remove or compact cancelled heap entries so far-future cancellation
  cannot accumulate permanent tombstones.
- [ ] Preserve ordering when both frame and deadline timers become due in one drain. Test
  nextTick/nextFrame chains, repeating timers, self-kill, callback-created timers, and unload.
- [ ] Integrate due-work batching with slice 6 without dropping overdue timers or changing
  nextTick/nextFrame target calculation. Record any deliberate API-semantic change explicitly.
- [ ] Compare idle/due/cancel-heavy workloads at baseline sizes. Run core tests and live
  frame-order checks; publish results before claiming an improvement.

## Slice 9: Prepare plugin/config changes on a worker

**Create:** core/src/loader_worker.rs. **Modify:** core/src/loader.rs, core/src/v8host.rs,
core/src/lib.rs, shim config-path plumbing and shared engine-op declarations only if required.
**Tests:** loader.rs, loader_worker.rs, config reload integration tests.

**Boundary:** Worker inputs are owned paths/bytes and revision tags. Worker outputs are owned
prepared data. No V8 handles, engine callbacks, or pointers cross this boundary.

- [ ] Test slow reads without delaying the frame thread, malformed/truncated archives, edit A
  overtaken by edit B, delete/recreate while parsing, plugin unload during preparation, and
  worker shutdown while a result is pending.
- [ ] Capture resolved plugin/config paths on the main thread; preserve current sanitization
  and override rules. Perform periodic discovery, reads, content comparison, and archive parsing
  on the worker with bounded submissions/results and per-path coalescing.
- [ ] Tag requests/results with lifecycle epoch and path revision. Discard obsolete results.
  Apply current prepared results on the main thread with dependency ordering and existing
  failed-reload behavior; never let a worker decide engine liveness or publish interfaces.
- [ ] Preserve watch baselines, missing files, invalid config diagnostics, permissions, and
  incompatible API-version handling. Ensure replacing a file mid-read cannot apply mixed data.
- [ ] Trace periodic frames to verify no directory scan, watched-config read, or archive parsing
  remains there. Run reload/config tests and live hot reload under slow filesystem conditions.

## Slice 10: Extract host responsibilities without changing behavior

**Modify:** core/src/v8host.rs, core/src/lib.rs, shim/src/s2script_mm.cpp, shim/CMakeLists.txt.
**Create proposed destinations:** core/src/v8host/{lifecycle,timers,natives,tests}.rs and
shim/src/config_ops.{h,cpp}. Move existing code only after validating each responsibility's
imports and state ownership; keep the isolate-owning entry point in v8host.rs.

**Boundary:** Child modules use narrow adapters. Keep HOST/PLUGINS/REGISTRY ownership explicit;
do not create a universal context object or expose mutable global stores for convenience.

- [ ] Record exports, native registration names, reset order, ledger teardown order, and shared
  engine-op ABI field order before moving code. Use existing boundary/ABI checks as witnesses.
- [ ] Move the existing test module to v8host/tests.rs, preserving test names and execution
  constraints. Move lifecycle/loading, timer adapters, and native installation in separate
  mechanical commits within this slice; run focused tests after each move.
- [ ] Extract C++ config operations and path handling into config_ops.h/.cpp, keeping their
  owned buffers and signatures together. Update CMake and all call sites atomically.
- [ ] Review each move for new dependencies, accidental public visibility, changed initialization
  order, nested RefCell borrows, and handles dropped after the isolate. Preserve assertions.
- [ ] Run make ci and live lifecycle/reentry/hook gates. Re-run the exact benchmark workloads;
  investigate regressions. File-size reduction is a maintainability result, not speed evidence.

## Final integrated acceptance

- [ ] All ten rows link to their implementation commit/PR and evidence; all unchecked steps
  are either completed or explicitly reported as remaining work, never silently waived.
- [ ] Run make ci on the integrated branch. A missing-Docker test-gate failure can be recorded
  as an environment limitation for local JS work; it does not satisfy the final native/live gate.
- [ ] Build loadable Linux binaries and base plugins using the existing sniper/package scripts.
  Use the Docker server setup from README.md and docs/BUILDING.md; do not assume Docker, a
  downloaded CS2 installation, or Linux-compatible binaries are present on the current Mac.

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/build-base-plugins.sh
```

- [ ] Package/install the resulting artifacts into the designated test server, then restart
  its cs2 service using the repository runbook. Preserve existing server data and config.
- [ ] Run a 60-minute mixed-workload soak: client reconnects/slot reuse, plugin reloads, timer
  churn, hook churn, slow peers, database lock/recovery, and config edits. Repeat bounded bursts
  after warm-up and allow quiescence between them.
- [ ] Assert no wrong-client action, lost accepted cookie update under transient failure,
  stranded connection/promise, stale-owner callback, or resource/index growth per completed
  cycle. Compare counts first; allocator RSS retention alone is not proof of a leak.
- [ ] Publish before/after measurements with commit, machine, map, player count, workload,
  repetitions, and p50/p95/p99/max results. Keep unsupported speed claims out of release notes.
- [ ] Update docs/PROGRESS.md and operator/API documentation for any new limits, overload
  behavior, lifecycle semantics, and migration requirements.

## Resume instruction

Read the spec and this workflow, inspect current commits and the status table, and execute the
first incomplete slice using the common slice loop. Preserve completed evidence, keep changes
within that slice, and update its status only after its completion gates pass. Continue in the
listed order under the user's execution authorization; do not infer permission to merge or
deploy production from this planning document.

## Integrated execution status (September 5)

All ten slices are implemented and independently reviewed in the local dependent Git stack.
Branch-local records in `runtime-hardening/` contain their specific evidence and limitations.
Main is preserved; publication and merge are outside this implementation run.

The final integrated runtime at `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1` passes
797 core tests and all three fresh-process pressure cases. The full JavaScript/Docker gate
previously passed 584 SDK tests; the final fixes only changed Rust and its pressure script.
Linux-container static ABI checks also pass on the final integrated source. The final [Linux native gate](runtime-hardening/final-review/linux-native-acceptance.md)
also passes locally in Docker: 797 core tests, three pressure processes, sanitizer tests,
full shim linking and its core-entry-point checks. Game-library symbol resolution skipped
for lack of a CS2 installation in the isolated checkout; installed-engine acceptance remains required.

The owned test server last ran the reviewed slice-6 code. Its bounded pressure and mixed
component checks passed, but the 300-second mixed pilot was not accepted because it had
engine navigation errors and insufficient slot-reuse cycles. A corrected fixture later
proved two actual reuses in 20 attempts with no failures. The final loader-aware 60-minute
mixed soak is still pending; its collector has 34 passing deterministic tests. Headless
bots do not provide actual CheckTransmit viewer traffic, and human auth/map/visual behavior
has not been fully exercised. Native/model benchmark results do not establish whole-engine
throughput or a whole-process RSS bound.

Model allocation followed the requested speed/quality/cost workflow: Sol handled normal
implementation and mechanical extraction; Luna handled bounded preparation/helper work;
Astra handled concurrency/architecture work, difficult fixes and independent safety review.
Root owned stack integration, evidence, live-server coordination and reviewer dispatch.
Independent work ran in isolated worktrees with recorded fixed bases; parent validation and
restacking stayed ordered. Escalations were driven by concrete review failures, including
fresh Astra implementers for the final loader fix rounds.

The [final paired benchmarks](../../benchmarks/2026-09-runtime-hardening/README.md)
preserve source snapshots, five raw baseline/candidate runs, and p50/p95/p99/max results.
The final fixes leave the measured timer and hook source files byte-identical. Timer idle
and large cancellation improve; all-due draining and tiny cancellation regress, as explicitly
reported. Hook results are native models, not end-to-end engine measurements.

The [final independent re-review](runtime-hardening/final-review/final-fix-review.md)
closes all three whole-stack findings: initial config edits, historical path revisions,
and request-header capacity accounting. The consolidated correction remains in slices
six and nine; extraction preserves it. No actionable review finding remains.

Remaining gates: game-library symbol resolution and installed-engine acceptance, then
the 60-minute mixed soak. The final release package is built and checksum-verified, including all 14 enabled
base plugins; it has not been installed on Nebula. SSH to
Nebula currently needs the configured 1Password agent to sign again; LTS Node is installed
and its explicit noninteractive PATH has already passed the harness tests there.
