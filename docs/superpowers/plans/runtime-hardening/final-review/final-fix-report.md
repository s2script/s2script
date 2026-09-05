# Consolidated final fix wave

## Stage A — F3, request-header capacity (slice 6)

Parent: `fab3463e794e004a1a1cc7d199256bf22a36a118`. Worktree: `/private/tmp/s2script-runtime-hardening`.

Read the full final-stack review, reproduction patch/README and all three raw failure logs. Adapted the actual HTTP/WS native capacity observer regression to the pre-extraction `v8host::frame_tests`. Parent RED (`final-f3-red.log`): 513 headers retain HTTP=51,104 / WS=51,101 bytes against 34,813 charged under a fresh 40,000-byte item cap. The test failed 0 passed / 1 failed.

Both request natives now call `jobs::push_request_header` only after both strings pass `copy_string` admission. The shared helper grows only on capacity exhaustion, with an exact target of `floor((len+1)*64/size_of::<(String,String)>())`; the existing 32-byte per-string allowance funds the tuple slots. On 64-bit targets this gives approximately 4/3 amortized growth, no additional charge, and no allocation before admission. `reserve_exact` avoids Vec's deliberate geometric over-allocation; the actual capacity invariant is checked after every push through 16,385 headers on this target. No transport/V8/allocator overhead claim is made.

A first exact-one-slot implementation was rejected on performance grounds before freezing. Its growth witness RED (`final-f3-growth-red.log`) showed 16,385 capacity changes; the final helper GREEN shows 31. Actual-native final GREEN (`final-f3-amortized-green.log`): HTTP=33,248 / WS=33,245 against the unchanged 34,813 charge. The fresh-policy test also submits 601 headers through both natives and verifies `AsyncPayloadTooLarge`, no submit observer, no retained job/resolver/bytes. The pressure script retains both earlier exact module/test names and adds the third independent process. Trimmed the tracked task-6-report trailing blank line only.

Exact commands (run in slice6 worktree; no `--test-threads` override):

```sh
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target S2SCRIPT_ASYNC_LIMITS_JSON='{"input_item_bytes":40000}' cargo test -p s2script-core final_review_request_header_capacity_is_covered_by_input_reservation -- --nocapture
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core request_header_growth -- --nocapture
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core request_header -- --nocapture
final_f3_tmp=$(mktemp -d /tmp/final-f3-tests.XXXXXX)
TMPDIR="$final_f3_tmp" RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core
TMPDIR="$final_f3_tmp" RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target bash scripts/test-async-pressure.sh
git diff --check
```

Final core: 725 passed, 0 failed, 3 ignored; 9.45s (`final-f3-core.log`). Pressure: all three separately selected fresh-process tests passed (`final-f3-pressure.log`). Diff check clean. Existing nine compiler warnings unchanged. A full run using the default reused macOS TMPDIR hit five existing SQLite table collisions from persisted PID-derived filenames; retained in `final-f3-core-stale-temp.log`. Fresh TMPDIR avoided the collision without touching DB source.

Stage A committed/frozen: `a28fccd8042af059cc952caed9b7ad77de44e90b` (clean tracked worktree). Root notified for the dependent restack. Stage B F1/F2 edits wait for root's restack and explicit slice9 handoff.

## Stage B — F1/F2, loader coordinator (slice 9)

Root restacked 7/8/9 after Stage A. Explicit handoff parent: `c0252e460c4dd26318e932f8313ba6b9714be889`, worktree `/private/tmp/s2script-loader-worker`. No slice9 edits preceded this handoff.

Parent RED: actual worker/V8 reproduction lost the after-registration A→B edit (`Seed`, then `Unchanged`, getter still A, no callback). Actual malformed-archive churn retained 64 historical revision paths with one obligation at a time and idle gauges. Both failed in `final-loader-red.log` (0 passed, 2 failed). Strengthened revision test also failed with `[1,1,...,1]` across distinct retired paths (`final-f2-unique-red.log`).

F1: first delivery now reconciles each unseeded plugin's snapshot with its own current applied config. This includes a shared path's `Unchanged` result for a joining plugin. Subsequent `Changed` handling retains the previous callback behavior. No periodic game-thread file read and no retained native config baseline were added. Two actual-worker scenarios hold an unrelated regular-file read, register/coalesce two first watchers, then exercise an after-registration edit or unchanged generated-default JSONC. A third watcher subsequently joins the shared path. In the changed case all three observe B exactly once; the unchanged case fires zero callbacks.

The current/proposed comparison normalizes both with V8 JSON serialization, then rejects unequal UTF-8 lengths before copying current content into Rust. Structural comparison of the bounded copies ignores property order. Normalizing both sides also preserves V8 number semantics: an intermediate implementation produced a spurious event for Rust `1.0` versus V8 `1`; the expanded no-event test failed (`final-f1-numeric-red.log`) and now passes with float1.0, reordered keys, escaped newline and Unicode (`final-f1-canonical-green.log`). Native comparison copies are bounded by the proposed serialized config; no new ownership table or metrics schema is introduced. V8 JSON serialization still creates V8-side temporaries and may invoke current-object getters/toJSON; this change does not claim to bound arbitrary plugin V8 execution or heap activity.

F2: `PATH_REVISIONS` was only an allocator, not a validation table. It is deleted and replaced by one never-reset scalar `NEXT_PATH_REVISION`; active expected revisions remain in `ACTIVE_BATCH.pending`. Cancellation drops pending work without retaining a path tombstone, and checked increment prevents token reuse. The churn test checks internal path tables, active/ready/waiting rows and retained gauges after64 distinct create→malformed prepare→delete→empty scan cycles. A separate actual-worker test with valid archives replays an old result after cancellation and again after delete/recreate admission; the pending replacement token survives and only version2 becomes ready (`final-f2-green.log`).

Exact commands run in slice9 (shared target; no thread-count override):

```sh
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core final_review_ -- --nocapture
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core removed_plugin_paths -- --nocapture
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core paths_ -- --nocapture
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core suppress_unchanged_defaults -- --nocapture
RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core first_watch -- --nocapture
final_loader_tmp=$(mktemp -d /tmp/final-loader-tests.XXXXXX)
TMPDIR="$final_loader_tmp" RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target cargo test -p s2script-core
TMPDIR="$final_loader_tmp" RUSTFLAGS='-C linker=/tmp/task9-linker' CARGO_TARGET_DIR=/Users/ghirakawa/projects/s2script/target bash scripts/test-async-pressure.sh
git diff --check
```

Final source verification: core797 passed /0 failed /3 ignored, 9.58s (`final-loader-core.log`); all3 independently selected fresh-pressure processes pass (`final-loader-pressure.log`); diff check clean. Existing nine compiler warnings unchanged. **Metrics schema unchanged.** Existing pressure module/test paths unchanged. Worker test read-gate visibility is test-only. During Stages A/B no SSH, pushes, PRs, rebases, subagents, style expansion or additional reviews were performed by this implementer. Root retains integration/extraction restack, one scoped independent re-review, Linux/live gates and benchmark ownership.

Stage B committed/frozen: `009ab2dec7b9df97bf8191ced71d90f91164fb15`; tracked slice9 worktree clean. Root notified for the extraction restack and integrated gates.

## Stage C — root-delegated extraction restack conflicts and integrated gates

After Stage B freeze, root explicitly delegated genuine source conflicts from its top10 rebase in `/Users/ghirakawa/projects/s2script`. Original top (including root benchmark docs): `726bea70658f0d9a8ec1ab5b25f242f589ce20f9`; old9 parent: `6b3bd9364594797797c4e45af230659905856b97`; new9 parent: `009ab2dec7b9df97bf8191ced71d90f91164fb15`.

One non-final resolution attempt accidentally matched an equals-sign source comment as a conflict separator, producing an incomplete intermediate tests move (`4026197`). Immediate line-count/test-body inspection detected it. Root was informed and authorized abort/restart from frozen726bea7. That intermediate commit was discarded; no tests or source from it are included in the final branch. The restarted resolution used the frozen slice9 test body directly and line-anchored conflict markers.

Resolved conflicts only: complete frame test body moved into `v8host/tests.rs`, preserving the new F3 native-capacity and pressure tests; F1 snapshot/reconciliation/notification block moved byte-for-byte into `v8host/lifecycle.rs`, adding its existing parent-path re-export; style-ending conflict preserved the new tests. Remaining root docs/benchmark and extraction commits replayed cleanly. Final top10 **frozen HEAD: `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1`**, branch `refactor/hardening-10-host-modules`, tracked worktree clean.

Equivalence witnesses (`final-integrated-move-witness.log`): full extracted test content identical excluding blank-only lines (8410→8409 lines; the style commit removes one internal blank line). F1 production snapshot/reconciliation/callback block byte-identical. `loader.rs`, `loader_worker.rs`, `http.rs`, `jobs.rs`, `ffi.rs`, `engine_ops.generated.rs`, `shim/include/s2script_core.h`, and the three-process pressure script remain byte-identical to frozen9. `shutdown()` unchanged; HOST/PLUGINS/REGISTRY still defined only in parent v8host.rs. Recursive lexical set_native name sets identical before/after (241 literal installation names in this witness, missing/added empty).

Integrated commands, in the original checkout:

```sh
final_integrated_tmp=$(mktemp -d /tmp/final-integrated-tests.XXXXXX)
TMPDIR="$final_integrated_tmp" RUSTFLAGS='-C linker=/tmp/task9-linker' cargo test -p s2script-core
TMPDIR="$final_integrated_tmp" RUSTFLAGS='-C linker=/tmp/task9-linker' bash scripts/test-async-pressure.sh
bash scripts/check-core-boundary.sh
bash scripts/test-boundary-nameleak.sh
bash scripts/check-invoke-abi.sh
bash scripts/check-core-js-lint.sh
c++ -std=c++17 -Wall -Wextra -fsyntax-only shim/src/config_ops.cpp
git diff --check 009ab2dec7b9df97bf8191ced71d90f91164fb15..HEAD
git diff --check 043861085902c43c6f53b63b3212e8a563e3382d..HEAD
```

Integrated final core: **797 passed, 0 failed, 3 ignored**, 9.61s (`final-integrated-core.log`). All three exact fresh-process pressure tests passed (`final-integrated-pressure.log`); no `--test-threads` override. Boundary, name-leak negative probes, invoke ABI (`lifetime=10 flags=4`), recursive JS/native-name lint, C++ config syntax and both diff checks passed (`final-integrated-static.log`; C++/diff checks are silent on success). Nine existing Rust warnings unchanged. Metrics schema unchanged. Root notified with frozen SHA before its one scoped independent re-review; Linux full link/live gates and broader integration remain root-owned.
