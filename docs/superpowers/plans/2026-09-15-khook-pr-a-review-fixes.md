# KHook PR A review fixes implementation plan

> **For agentic workers:** Use a dynamic subagent workflow with isolated task branches, regression tests, task review, and a final integration review. Track changing assignments and evidence in the execution ledger.

**Goal:** Resolve the twelve findings against PR #221 at `394402f2a08fbbbd7c40f8fef1f18f170bbf04ee` and finish the executable acceptance paths.
**Architecture:** Retain the corrected Metamod provider design, make forced unload and removal completion safe, then make suite A evidence immutable and bound to actual artifacts. Native and JS fixtures must record observed counts and lifecycle transitions.
**Tech stack:** C++17/KHook/Metamod, Python 3, TypeScript, AMBuild, Linux x86_64 sniper.
**Spec:** [PR A remediation design](../specs/2026-09-14-khook-pr-a-remediation-design.md).

## Global constraints

- Metamod remains `7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33`; KHook remains `1e200e4cc8e0badcb7cf941525268d6977f6a4e6`. Deliver source changes as reproducible tracked patches.
- Preserve public JS APIs, HookResult semantics, and PR A scope. Do not migrate damage/inline hooks or implement PR B.
- Never release plugin code/provider before callbacks, physical removal, safe-thread plugin cleanup, and static destruction finish. No busy loops on the game thread and no V8 shutdown on a removal worker.
- Native/JS/human evidence is independent. Missing required evidence is pending; observed failures and malformed evidence cannot be erased by later collection.
- Every passing run identifies the actual s2script binary, host manifest, fixtures, server build and map. A controller checkout SHA alone is not binary identity.
- Keep twelve case names. Added subchecks and record fields must agree in controller, both fixtures, offline data and documentation.
- Keep PR #221 draft and PR B blocked until required live evidence passes. Do not merge or release.
- Workers own isolated worktrees. The coordinator integrates commits sequentially and owns cross-task protocol changes and CI wiring. No worker pushes or dispatches more agents.
- Meaningful regressions exercise production logic and show the reviewed baseline failure. Human hearing/visibility is never fabricated by an agent.

## Dynamic execution and review

Run Tasks 1, 2 and 3 independently; Task 4 may proceed alongside them. Each owns a separate worktree, eliminating shared-index conflicts. Task 5 consumes their commits and resolves protocol joins. An available agent slot moves to task review as soon as an implementation is ready; a rejected change returns to its implementer. Escalate architecture questions or repeatedly failing fixes to a stronger model instead of repeating an unchanged assignment.

| Task | Initial owner/model | Dependencies | Owned files |
| --- | --- | --- | --- |
| 1 | Astra, high | none | host patch, host lifetime tests/scripts; necessary shim lifecycle integration only |
| 2 | Sol, high | none; interfaces joined in 5 | Python controller/tests, shell runner |
| 3 | Sol, high | none; interfaces joined in 5 | native/JS fixtures, observer tests, fixture data and README |
| 4 | coordinator | none | corrected-host build recipe/test, CI wiring, build docs |
| 5 | coordinator plus scoped workers | 1–4 | artifact identity handshake, lifecycle persistence, cross-task compatibility |
| 6 | fresh Astra reviewer, high | 5 | read-only final review; implementation owner handles findings |

### Task 1: Safe forced unload and reentrant completion

**Files:** `patches/metamod-source/0001-khook-owned-provider-retirement.patch`, `shim/tests/khook_host_lifetime_test.cpp`, `scripts/test-khook-host-lifetime.sh`; necessary shim shutdown integration may be proposed to coordinator.
**Interfaces:** Keep IKHook API; provider retirement must expose completion only after backend locks and plugin callbacks are finished. Actual host `_Unload`/`UnloadAll` must preserve a plugin that returns Busy/Pending and retry cleanup on an appropriate host/game lifecycle boundary.

- [ ] Reproduce forced `_Unload(..., true)` bypass and the pinned backend's lock-held completion using actual patched production lifecycle/provider code.
- [ ] Ensure a false plugin Unload result cannot destroy the plugin in forced/process-exit paths; implement bounded/deferred progress without losing its API object or running V8 cleanup on a worker. Validate idle and in-callback requests.
- [ ] Move completion delivery outside backend deletion/insertion locks; address synchronous pending-insert cancellation too. Preserve one backend removal and one callback per subscriber, including nested removal and static destruction.
- [ ] Test delayed completion, actual forced host path, busy core, eventual cleanup exactly once, callback reentry and destruction order with ASan/UBSan. Do not let a mock omit the lock that triggered the bug.
- [ ] Commit and report exact commands/results, native platform limitations and proposed integration needs.

### Task 2: Persistent, strict acceptance evidence

**Files:** `scripts/khook_acceptance.py`, `scripts/test-khook-acceptance.py`, `scripts/test-khook-live.sh`.
**Interfaces:** Keep prepare/collect/judge and twelve cases. Propose any producer handshake to Task 3/coordinator before changing its commands. Use an explicit artifact input/discovery protocol; absence is pending and mismatch invalid, never a false pass.

- [ ] Add failing tests: failure then pass across collects; duplicate fail/pass within one response; malformed JSON followed by valid records; repeated raw outputs; missing binary identity; wrong artifact; human outcomes absent/null and contradictory pass flags.
- [ ] Preserve append-only raw history and irreversible errors/failures for each run. Allow legitimate pending-to-observed transitions and identical replay, while rejecting contradictory terminal results. Offline and live inputs must use the same validation and aggregation rules.
- [ ] Require validated artifact/run identity and expose a usable CLI path for it. Do not infer installed artifacts from controller HEAD. Bind human observations to the same artifact identity and require real expected/actual outcomes.
- [ ] Run Python tests and shell self-test; add regression evidence for both failure retention and legitimate multistage completion. Commit with protocol/documentation notes for integration.

### Task 3: Executable native and JS acceptance stages

**Files:** `tools/khook-probe/plugin.cpp`, `acceptance_observer.h`, `CMakeLists.txt`, `README.md`, `testdata/*`; `examples/khook-acceptance/*`; `shim/tests/khook_acceptance_observer_test.cpp`; new focused fixture harness if needed.
**Interfaces:** Preserve controller case names; propose explicit added subchecks/identity fields before integration. Test production fixture logic (TypeScript VM/mock engine boundary or shared native observation helpers), not rewritten toy state machines.

- [ ] Fix the observed TypeScript syntax error and verify normal typecheck/build.
- [ ] Make final unsubscribe wait for a witnessed Touch invocation after both removals. Require exact PRE/POST/original counts, including first POST during self-unsubscribe; emit actual counts even on failure. Add negative tests for duplicates, omitted calls, stale phases and suppressed original.
- [ ] Observe voice effective arguments after policy processing and real original execution, including explicit Recall and both plugin orders. A PRE counter is not an original witness.
- [ ] Require real ClientCommand coverage independently of the registered ConCommand path. Follow the spec's route-validation requirement; never relabel DispatchConCommand counters. Both Continue/Handled paths and negative controls need a valid engine boundary.
- [ ] Replace cvar-existence plugin presence with an actual lifecycle/generation witness that notices unload. Keep visibility restoration separate from entity deletion until human capture completes; make restore and cleanup commands documented and executable.
- [ ] Preserve the same run through native as well as JS unload/reload via external controller state. A JS instance counter must not pass native unload. If native lifecycle instrumentation needs another owner, send a precise interface request.
- [ ] Update fixture data and README, run real-code host regressions, typecheck/build and observer tests. Report real-server/human evidence separately from implemented workflows.

### Task 4: Reproducible corrected-host build and validation environment

**Files:** `scripts/build-metamod-pinned.sh`, a focused build preparation regression, `scripts/ci-native.sh`, build documentation and native workflow path filters when necessary.
**Interfaces:** Keep `build/metamod-pinned/{tree,metamod-build.json}` and source/patch digest recipe. Build identity is an input to Task 5.

- [ ] Reproduce configuration against the isolated source: the pinned version script opens `.git/HEAD` after the recipe deletes it.
- [ ] Prepare a clean exact-source build with correct versioning metadata or explicitly disabled auto-versioning. Never inherit the parent repository's git identity or modify vendored checkout state. Invalidate stale success manifests on a failed rebuild.
- [ ] Execute a behavioral preparation/configuration regression, including failure propagation. Run actual corrected-host AMBuild and artifact verification on Linux/sniper where available; wire repeatable build validation through the native script.
- [ ] Record available macOS tests versus Linux CI/build tests; no local arm64 output may count as a deployable host.

### Task 5: Integration, identity, and full gates

**Files:** cross-task interfaces above, native/JS CI entry scripts, `licenses/licenses.txt`, operator docs, this plan's completion evidence.

- [ ] Review each task diff for spec compliance and correctness, integrate sequentially, and resolve controller/producer schemas. Every required workflow must have a complete prepare/observe/restore/collect path.
- [ ] Bind run identity to verified installed binaries/manifests and fixture revisions; test stale/wrong runtime rejection plus real pending-to-pass collection across lifecycle stages.
- [ ] Regenerate deterministic notices for the final patch digest. Run focused tests then `make ci` on supported Linux, corrected-host/shim/probe optimized builds and artifact verification.
- [ ] Run live suite A on an available authorized CS2 host, including both plugin orders, forced/callback unload and process shutdown; retain required human voice/transmit/recipient observations as pending until actually observed.
- [ ] Commit and push to the existing PR branch without overwriting new upstream work. Inspect CI for that exact head and fix failures until supported checks pass.

### Task 6: Final review and status

- [ ] Fresh reviewer compares the final integrated diff to this plan and the remediation spec, prioritizing lifetime and trustworthy evidence.
- [ ] Resolve concrete findings, rerun affected tests and obtain scoped re-review.
- [ ] Update PR description with actual implementing commits, passing gates and explicit remaining live/human evidence. Keep draft status until all acceptance conditions pass.

## Completion record

Execution is in progress. The implementation ledger records task assignments, decisions, commits, reviews and actual evidence. No live acceptance is claimed by this plan.
