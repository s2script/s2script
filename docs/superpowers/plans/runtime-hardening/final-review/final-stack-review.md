# Final whole-stack review

Reviewed production HEAD: `043861085902c43c6f53b63b3212e8a563e3382d`.
Base: `ebb3b3167d639d5b94e491ab27554e69fa373fc4`.
Checkout: `/Users/ghirakawa/projects/s2script`; clean throughout this review.

## Verdict

**Spec verdict: changes requested.** Three confirmed P2 findings remain: a lost config-watch edit, unbounded loader revision history, and undercharged HTTP/WS native request headers. The remaining inspected cross-slice behavior is consistent with the shared design and the prior independent-review outcomes. Required final Linux native/shim, installed-engine, and integrated soak acceptance remains incomplete independently of these findings.

**Quality verdict: changes requested; no Critical/P1 finding.** Each finding below has a focused failing reproduction against the final production source copied into scratch. All three belong in one root-owned consolidated fix wave, followed by a scoped independent re-review. No production edit, SSH action, subagent, remote publication, or broad-suite rerun was performed during this review.

## Findings

### F1 — P2: Do not acknowledge a first watch snapshot that the plugin never applied

Location: `core/src/loader.rs:1099–1101`; acknowledgement at `1120–1126`. First watch submission is at `794–811`.

`config.onChange` now schedules the first file read asynchronously. `handle_config` deliberately skips applying the first snapshot (`CONFIG_SEEDED` was absent), then acknowledges `WatchDelta::Seed` as handled. If a config changes after registration but before this queued read runs, the worker commits the new content as its baseline while the plugin keeps its previous values. Future reads return `Unchanged`, so the edit is lost until a second edit or reload.

**Reproduction:** Hold the real worker on an unrelated regular-file read. Create a V8 plugin with `greeting=A` and a real `config.onChange` listener, register its watch while the config file contains A, then write B after registration returns and release the worker. Deliver the actual worker result and one subsequent normal read. The final tree reports deltas `[Seed, Unchanged]`, the getter still returns A, and the listener has never fired. Expected state is B with the listener observing B. The base implementation synchronously seeded the file at the registration boundary, so the same after-registration edit was detected on the next poll; the widened asynchronous gap is introduced by Task 9.

Evidence: `final-review-repro/seed-repro.log`, test `loader::tests::final_review_edit_after_watch_registration_is_not_lost` (0 passed, 1 failed; 0.02s). This is an actual V8/worker/coordinator reproduction, not a model.

Requested correction: establish the watcher baseline from content known to correspond to the plugin's applied initial state, or otherwise reconcile the first returned snapshot before acknowledging it. Preserve suppression of genuinely unchanged auto-generated defaults. Cover initial queue delay, coalesced first reads, and a watcher joining a shared path; do not restore a synchronous filesystem read to the game thread.

### F2 — P2: Retire historical plugin-path revision entries

Location: `core/src/loader.rs:817–823`, table declaration at `663`; discovered-path call at `957`.

`next_path_revision` inserts an owned `PathBuf` into `PATH_REVISIONS` for every discovered candidate. Nothing removes these rows when preparation fails, a file vanishes, or its work finishes; only complete loader shutdown clears the map. Thus a directory with one candidate at a time can accumulate an unlimited native history of old filenames while all queue and retained-payload metrics are zero. This map is new in Task 9; it is separate from the pre-existing manually suppressed-path state.

**Reproduction:** Use an actual worker with one request/result obligation at a time. Repeat 64 distinct regular-file create → scan → malformed-archive preparation result → delete → empty scan cycles. Every iteration has at most one file and finishes its worker obligation. At the end there are zero files, watch rows, file stamps, obligations, results, pending/active/ready/waiting/applying rows, or retained payloads, but `PATH_REVISIONS.len()` is 64. Increasing the number of cycles increases this retained history linearly. The reported loader gauges do not expose it.

Evidence: `final-review-repro/revision-repro.log`, test `loader::tests::final_review_removed_plugin_paths_do_not_accumulate_revision_tombstones` (0 passed, 1 failed; 0.02s). The probe injects the tiny worker policy while main policy diagnostics remain default; neither policy bounds this map.

Requested correction: avoid per-path permanent tombstones, for example by assigning revisions from a lifecycle-scoped monotonically increasing allocator and keeping only currently needed path state. Preserve rejection of late results across cancellation and delete/recreate. Add a churn test that checks internal path-state retention after work becomes idle, beyond the existing payload gauges.

### F3 — P2: Charge request-header vector capacity before HTTP/WS admission

Locations: `core/src/http.rs:238–242` and `core/src/v8host.rs:962–967`; shared string charge at `core/src/jobs.rs:copy_string`.

Both native request builders charge each copied header key/value through `copy_string`, including 32 bytes of allowance per string, but append `(String, String)` tuples to a geometrically growing `Vec` without reserving or charging that capacity. A tuple is 48 bytes on this target; near a growth boundary, the retained tuple capacity exceeds the total 64-byte-per-header allowance. The new input admission policy therefore accepts requests whose application-owned native buffers exceed their reservation and configured item cap. The earlier Task 6 correction handles HTTP **response** tuples and SQL vectors; these two **request** builders still contain the same capacity gap.

**Reproduction:** Start a fresh process with `S2SCRIPT_ASYNC_LIMITS_JSON={"input_item_bytes":40000}`. Through the actual V8 natives, submit 513 short headers (`h0`…`h512`, empty values) to an invalid URL. Test-only observers immediately before native submission measure Vec capacity plus owned String capacities, while the lease is still present. HTTP retains 51,104 bytes, WS retains 51,101 bytes; each is charged only 34,813 input bytes. Both exceed the 40,000-byte per-item cap before URL validation. The discrepancy even exceeds the separate default failure allowance. Invalid URLs prevent network activity, so this measurement excludes protocol buffers, transport allocations, V8 storage, and allocator overhead.

Evidence: `final-review-repro/headers-repro.log`, test `v8host::frame_tests::final_review_request_header_capacity_is_covered_by_input_reservation` (0 passed, 1 failed; 0.03s). Diagnostic instrumentation observes the production builders; it does not change their allocation or admission behavior.

Requested correction: charge actual tuple capacity growth before allocation, or reserve exact already-charged tuple slots, in both builders. Keep input-byte admission ahead of copies and avoid double-charging the existing per-string metadata allowance. Test the just-over-power-of-two boundary and named overload behavior under a tiny fresh-process policy.

## Cross-slice review coverage

I read the supplied whole-stack diff, shared spec, branch-local slice evidence, progress/review rulings, Task 9 integration and round-5 outcomes, Task 10 equivalence review/report, and the final implementation paths. The 8,316-line test extraction was treated as a move rather than new behavior.

- **Ledger, subscription, and teardown:** inspected active-only acquisition/removal indexes, reverse order, duplicate multiplicity, generation-bound resource release, job completion/cancellation, interface-consumer cleanup, subscription pruning, socket retirement, selected timers, and shutdown order. No further introduced issue found.
- **Client identity:** inspected separate nonresetting connection books, synchronous departing snapshots, v2 deferred envelopes, shim bootstrap/map reconciliation, guarded raw native adapters, Client/Player references including coercion checks, cookie delivery tokens, menus/HUD retained views, and affected plugin consumers. No additional stale-occupant bug found in the changed paths.
- **Sockets and async:** inspected monotonic control lanes, connection/terminal phases, write failure/close/shutdown selection, graceful deadlines, reservation lifetimes through worker/registry/queued/staged dispatch, connect checkpoint ordering, fair polling and callback cursors, oversized progress, SQLite actor input destruction before publication, remote-pool lifetime leases, and HTTP/SQL result sizing. F3 is a remaining input-accounting defect, not a reopening of the fixed SQL/response-buffer reproductions.
- **Cookie persistence:** inspected stable outbox epoch/revisions, coalescing and `covers_from`, per-account FIFO, owner-bound ACKs, reclaim across reload/reinit, database revision UPSERT, read fences, generation-bound session delivery, and per-key acknowledged offline cache eviction. No additional persistence or stale-account finding.
- **Indexes:** inspected deadline heap/location/ID indexes, frame buckets, insertion-order ready selection, due/cancel/rearm ownership, hook kind/entity/subscription reverse indexes, VP last-subscriber teardown, registration order, and SetTransmit enumeration. No additional correctness finding. Timer selection remains finite under admitted timer count but its returned-item limit does not bound all due-discovery work; code and slice evidence disclose this.
- **Loader:** inspected joined worker shutdown, stable nonblocking regular-file reads, parse/discovery budgets, coalesced requests, prepared leases, READY/WAITING/application locators, stale generation retirement, main-thread permissions/config application, and loader metrics. F1/F2 are distinct from the already closed proposal-retirement and detached-lease-locator findings.
- **ABI/extraction:** versioned client/config additions leave the unversioned `S2EngineOps` order unchanged. The separate resolver remains registered before loader startup. Reviewed the mechanical extraction evidence for preserved API paths, store ownership, reset ordering, native names/scanners, and CMake integration; no added semantic/extraction finding.

## Nonblocking observations and validation limits

- The Task 10 `use super::*` observation remains nonblocking. Child modules are private; the imports do not expose mutable stores or change runtime ownership. Import cleanup should remain a separately reviewed refactor, not widen the mechanical extraction.
- Whole-stack `git diff --check` reports one documentation-only new blank line at EOF in `.superpowers/sdd/2026-09-04-runtime-hardening/task-6-report.md:189`. No source correctness implication. The working tree itself is clean.
- I considered the supplied final 791-core, two fresh-pressure, full-JS/584-SDK/Docker, ABI/boundary/native-name, and Linux static 126-op/sentinel evidence. These broad suites were not rerun without a new broad uncertainty; the three focused reproductions establish the uncovered defects despite those passes.
- The last full Linux native/shim gate was on slice 8. Final integrated 9/10 Linux compile/link/symbol checks, installed-engine validation, and the 60-minute mixed loader-aware soak remain pending SSH authentication. Static parser and C++ syntax checks cannot substitute for linkage/live behavior.
- Bot slot-reuse evidence is useful for connection-lifetime guards but does not prove real human CheckTransmit viewer traffic. Do not declare that live acceptance covered by bot-only tests.
- Root owns final benchmark collection. This review makes no speedup claim from asymptotic structure, test-module movement, or modeled hook benchmarks, and makes no native-RSS/whole-process plateau claim from logical gauges.

## Reproduction handoff

`final-review-repro.patch` contains the focused tests and test-only observers against the frozen production tree. `final-review-repro-README.md` records commands and exact limitations. All scratch sources and raw failure logs are in `final-review-repro/`. Scratch compilation reused the existing target directory, so a raw scratch test binary is not a new production-suite witness. No production file was changed.
