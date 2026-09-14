# KHook PR A remediation — design spec

**Status:** Ready for implementation handoff; no fixes or live acceptance are claimed by this document.
**Date:** 2026-09-14.
**Implementation plan:** [Remediation work packages](../plans/2026-09-14-khook-pr-a-remediation.md).
**Code under review:** [PR #221](https://github.com/s2script/s2script/pull/221), branch `cursor/khook-sourcehook-cutover-8628`, commit `57a329b7814c6e1d50f32cc804741ddd14b730bd`.
**Approved planning baseline:** [PR #220](https://github.com/s2script/s2script/pull/220), commit `14c7dec930389d2aca2d8db445c7fe568589c5d8`.
**Parent contracts:** [Migration design](2026-09-14-khook-migration-design.md) and [migration plan](../plans/2026-09-14-khook-migration.md), especially T2–T8.

## 1. Outcome and precedence

Finish PR A's checked-hook lifetime, verified host installation and executable acceptance. Preserve the three-slice migration: this work belongs in PR A; PR B remains blocked until PR A's full acceptance passes. The destination remains shared KHook installation/lifetime with the existing JS API.

This spec amends the parent documents where the review disproved an assumption: retaining the shared library alone does not preserve the pinned host's `IKHook` provider, and the initial probe is not a completed T8 implementation. It does not reopen the entire migration or move PR B's inline conversion into A.

Implement the remediation on the PR A branch after bringing in these documentation files. Use the user's dynamic workflow; task ownership and evidence travel with each worker assignment. Do not reinterpret this handoff as authorization to implement PR B, merge PRs, or release binaries.

## 2. Findings and evidence baseline

| ID | Finding at the reviewed commit | Required correction |
|----|--------------------------------|---------------------|
| F1 / P1 | `S2ScriptPlugin::Unload` removes this-filters and destroys core without checked physical retirement. The host destroys `CPlugin::m_khook` while callbacks/removal still use it. | Correct host provider ownership and removal tracking; integrate nonblocking shim retirement and safe retry. |
| F2 / P1 | Probe `Hook_FireEventPost` calls `EventNameIs(ev)` after the original consumed `ev`. | Observe the targeted call without dereferencing a consumed event; cover nested/consuming originals. |
| F3 / P2 | `s2_metamod_stage_is_pin_ok` accepts any file whose bytes lack the old SourceHook marker. | Require build provenance and binary validation before replacement; never manufacture verified identity from arbitrary input. |
| F4 / P2 | Five required records always emit `pending`: phase removal, entity/map/teardown, voice, transmit, recipient mask. | Implement the automated state transitions and explicit human observation ingestion. |
| F5 / P2 | `frame_client_command_hooks` passes from the probe's own counters and Supersede votes. | Require s2script delivery plus independent original/skipped observations; a missing or broken s2script callback must fail. |

Review checks: GitHub native/JS CI at `57a329b7` passed; the binding tests passed ASan/UBSan against the exact pinned header, the installer harness passed 51 checks, and the runner self-test passed. These results did not exercise real host lifetime or establish live acceptance. An isolated installer reproduction installed a zero-byte `.so`, wrote a PLAPI 18 identity and then skipped it as verified. Preserve that failure as a regression test.

Source anchors at the reviewed dependency pins:

- Metamod `7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33`: `core/metamod_plugins.cpp` (`CPlugin` destructor and `Unloader`), `core/metamod_plugins.h` (`m_khook`), `core/metamod.cpp` (`GetDetourInterface`), `core/metamod_khook.h` (registration/removal bookkeeping).
- KHook `1e200e4cc8e0badcb7cf941525268d6977f6a4e6`: `include/khook.hpp` (`Virtual` PRE/POST/removal callbacks call `GetContext` before filtering), `src/detour.cpp` (`RemoveHook` returns without completion for an already absent async id).
- `KHookImpl::SetupHook` does not register inline ids in the host ownership set, while `SetupVirtualHook` does. `RemoveHook` never erases that set. Repair these together; the probe uses both kinds.
- `FireEvent` consumes its event. The SDK's `FreeEvent` contract and [CounterStrikeSharp's public event wrapper](https://github.com/roflmuffin/CounterStrikeSharp/blob/main/managed/CounterStrikeSharp.API/Modules/Events/GameEvent.cs) agree on this lifetime.

## 3. Scope and invariants

1. Linux x86_64 and PLAPI 18. Server-loadable binaries use the sniper toolchain and require GLIBC <= 2.31.
2. Keep the original Metamod and KHook gitlinks as the source baseline. Carry reviewed host fixes as a reproducible patch set, built in an isolated source tree. Record the patch-set digest alongside both full SHAs. An upstream replacement is permitted only after the same regressions pass and all identity/docs references are updated; do not invent or silently select a newer pin.
3. No second hook engine, plugin-facing KHook API or changed JS `HookResult` semantics. No damage/inline migration or precache conversion in this remediation.
4. `PLUGIN_SAVEVARS` remains first in `Load`. No dummy engine-pointer probes, busy-waiting on the game thread, or early destruction of callback context.
5. The corrected host is now part of PR A's operator requirement. Unpatched PLAPI 18 alone is insufficient. Installer, build recipe, notices, upgrade and rollback docs must agree.
6. Every required case has an executable observation path and a documented outcome. Missing human evidence remains pending; it is never synthesized from logs or an arbitrary success flag.
7. The native probe and acceptance plugin remain test-only. New native/host tests run from `scripts/ci-native.sh`; do not add a parallel gate only in workflow YAML.

## 4. F1 — host and shim lifetime

### 4.1 Chosen host delivery

Add `patches/metamod-source/series` and `0001-khook-owned-provider-retirement.patch`. The patch covers provider ownership, per-id removal completion, and host unloader integration. Use an isolated copy of the exact source, apply the patch with `git apply --check`, then build. Do not leave uncommitted edits in the vendored submodule as the deliverable or require an unpublished submodule SHA/fork.

New `scripts/build-metamod-pinned.sh` builds the corrected runtime in the sniper environment. It produces the full Metamod tree and a build manifest under `build/metamod-pinned/`. Host tests compile the patched production provider/unloader code with an injected backend, not a separately reimplemented model. Extract a small internal unloader header in the patch if necessary to make the real ownership code testable.

### 4.2 Provider ownership contract

Allocate the provider at a stable address with shared ownership. `CPlugin` owns it while loaded; `GetDetourInterface` returns that same object's raw `IKHook*`. On removal, transfer/retain shared ownership in the host unloader until all of the following finish:

- in-flight detour PRE/original/POST execution;
- typed helper removal callbacks and registered completion callbacks;
- plugin static destructors and `dlclose`.

Retain the provider through `dlclose`, not just until immediately before it. Never run `dlclose`, foreign callbacks or backend removal while holding the provider's bookkeeping mutex. Do not retain dead providers/libraries forever as the fix.

This matters during normal `meta unload`: the command executes inside an intercepted `DispatchConCommand` original after our PRE's Observe guard has ended. A callback-depth test alone does not cover that interval. Physical removal completion and host provider ownership are both necessary.

### 4.3 Per-id host retirement

Track every accepted Function and Virtual id. Each has registered/removing/completed state and completion subscribers. One physical removal operation is allowed per id:

- The first removal marks the record removing and calls the low-level backend once.
- Further requests for a removing id join its completion subscribers; they do not issue another low-level removal.
- A completed/absent id completes an idempotent removal request without calling the pinned backend's absent-id async path.
- Keep removing records visible to the unloader until the backend and plugin callbacks have finished. Erasing an id merely when removal is requested lets the unloader release the library too early.
- Notify every subscriber exactly once, outside locks. Completion may happen synchronously for a queued insertion that is cancelled, so retain context before calling the backend.
- Stop accepting new registrations during host retirement. Valid rejected calls return `INVALID_HOOK` with the existing caller-side named failure.

The host unloader joins this provider-owned retirement, rather than copying the old stale `m_hooks` set and removing those ids a second time. Cover explicit removal followed by unload, unload without explicit removal, pending insertion cancellation, both hook kinds, reentrant completion, and a retained provider used by static destruction.

### 4.4 Checked bindings and shim lifecycle

Integrate a shutdown state with `Running -> Retiring -> Ready`. Preserve the existing API return conventions: SDKHooks add nonzero success; declarative install 0 success/-1 failure.

Before any destructive action, reject a reentrant unload while s2script callbacks or the core isolate are active. Add a read-only internal `s2script_core_can_shutdown()` check backed by the core's actual host-borrow/dispatch state; a panic swallowed inside `core_shutdown` is not success. Account for direct ConCommand/event-listener entry as well as checked KHook callbacks.

For a safe initial request, stop new registrations and JS dispatch, remove filters, and call `BeginRemove` once for every owned interface and SDKHooks binding. Retain bindings, the core, entity/event listeners and teardown resources while completions are pending. `S2SdkhooksVpUnload` must retire all fourteen binding objects, including objects with no remaining subscriber rows. Repeated calls are idempotent.

If retirement is pending, return false with a precise retry message. Do not spin, sleep in the callback, or report successful unload. A later external `meta unload` retry can drain the completed bookkeeping and finish core/listener/command cleanup once. The fixture runner owns bounded retries; removal must not depend on a GameFrame hook that has already been removed. While resident and retiring, report that state and fail new work by name.

Callbacks already accepted by KHook must still acquire the real provider safely; a shim early-return gate only controls dispatch into JS. Call sites must honor failed/retiring Observe guards. Use shared active-invocation accounting for shutdown checks; TLS depth alone is not a cross-thread drain.

The host's forced-unload/process-exit paths must preserve these ownership requirements. In particular, a forced caller must not convert a retirement-pending response into early destruction, and `UnloadAll` must not repeatedly spin on a pending plugin. Test these paths with delayed completion and document their deferred completion behavior.

Also close the checked Function retargeting hole without doing PR B: while a Function binding owns a live/removing id, configuring a different address returns a named failure without implicitly removing that id. Same-address Configure remains idempotent. A completed old binding may be replaced by a new object. Test rejected retarget -> normal retirement so ownership cannot get stuck on an id the base helper silently removed.

### 4.5 F1 acceptance

The actual patched host/provider test must prove: provider alive during original/POST/removal/destructors; provider and library eventually released; exactly-once completions; no stale-id unload hang; no premature core shutdown; retry progresses without game-thread blocking. Use ASan/UBSan and deterministic delayed completion. Live: unload/reload the probe and s2script with a peer still loaded, ordinary command-origin unload, unload requested during callback, and idle process shutdown. A resident pending unload is not a completed unload.

## 5. F2 — safe FireEvent observation

Observe fixture-generated events through an invocation scope that surrounds the entire call to `events->FireEvent`, owned by the caller in `RunSuiteA`. The scope carries a run/case token, nesting link, PRE/POST counts, skipped state and listener delivery. It is alive until FireEvent returns, and restores the enclosing scope on every exit.

Never dereference the incoming `IGameEvent*` in the probe's POST callback. For the controlled probe call, PRE/POST can correlate by the pointer value saved by the caller without reading its memory; also track nested invocations so another event cannot satisfy the outer case. The engine listener may read its event during its valid listener callback. For nonfixture events, do not inspect a possibly consumed pointer merely because our PRE ran: a preceding peer may already have called the original. Do not subscribe this observer to unrelated game events to invent coverage.

Prove engine delivery with the listener and the targeted invocation's skip state. `WasOriginalFunctionSkipped` describes KHook's automatic original: it does not count an explicit original call inside another PRE. For Handled/recipient-mask paths, use engine-side delivery/message observations to account for s2script's explicit original call and record its automatic-skip separately. Do not classify intentional CallOriginal+Supersede as zero engine calls.

Regression: an injected original deletes the event before POST; the observer passes ASan without reading it. Include nested target/non-target events, skipped original, one delivery, and duplicate delivery. The same tracking code must be used by the live probe.

## 6. F3 — trustworthy build identity and installation

### 6.1 Build manifest

The isolated build recipe emits `build/metamod-pinned/metamod-build.json` only after successful build/verification. Its versioned schema is:

```json
{
  "schema": 1,
  "plapi": 18,
  "metamod_commit": "7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33",
  "khook_commit": "1e200e4cc8e0badcb7cf941525268d6977f6a4e6",
  "patchset_sha256": "64 lowercase hexadecimal characters computed from the ordered series and patch bytes",
  "target": "linux-x86_64",
  "glibc_max": "2.31",
  "artifacts": [
    {"path": "bin/linuxsteamrt64/metamod.2.cs2.so", "sha256": "computed binary SHA-256"}
  ]
}
```

The example describes fields; implementations serialize real digests, never the explanatory strings above. Include the other required loader/runtime files in `artifacts`. Record toolchain/build commands in a companion build log. Read PLAPI from the checked source; do not derive it from a filename or write a requested version into the manifest without checking the build inputs.

`patchset_sha256` is SHA-256 over each series entry's UTF-8 relative pathname, one NUL, its exact bytes, and one NUL, in series order. Ignore only blank/comment lines in `series`; reject duplicate entries and paths escaping the patch directory.

### 6.2 Verification and transaction

The default source is this corrected pinned build. A staged tree needs the independently supplied build manifest from that build, with the expected source SHAs/patch digest, required file list, matching hashes, ELF64 little-endian x86_64 shared-object format and GLIBC requirements <= 2.31. Missing, empty, malformed, wrong-architecture, truncated or mismatched artifacts fail before replacement.

A checksum computed from an arbitrary candidate and then written to an identity sidecar is not provenance. The build manifest is an input to verification, not invented by the installer. These checks address accidental stale/corrupt/wrong builds; they do not claim that an unsigned operator-supplied manifest provides adversarial supply-chain attestation.

Remove the string-heuristic success paths and implicit trust in `S2_METAMOD_PINNED_TREE`. An arbitrary PLAPI 18 mmsdrop does not contain the required lifetime fix. Disable automatic latest-drop acceptance; an alternative build requires an independently verified compatible manifest and the same host regression/live evidence.

Stage under the destination's parent filesystem. Verify the whole candidate before stopping/replacing the destination. Preserve the previous tree and VDF. Check each copy/rename/write error explicitly, including identity writing; roll back failures after the first rename. Refuse to swap a running server. Only mark the installed tree verified after the transaction succeeds; revalidate bytes against the build manifest on the skip path. Keep the old runtime and Metamod pair available for operator rollback.

Regression cases include zero-byte candidate, plain text containing `GetDetourInterface`, valid wrong-source ELF, missing/wrong patch digest, stale sidecar, altered byte/hash, wrong architecture, truncated ELF, incomplete loader tree, failed copy/rename/identity write, already-running server, repeat verified installation and successful repaired build. Existing stub-file tests may test transaction mechanics through injected verification, but must not claim to validate an actual binary.

## 7. F4/F5 — acceptance that can finish and can detect failure

### 7.1 Run identity and records

Keep suite A's twelve top-level case names. Give each run a unique `run_id` and record s2script commit/build hash, host manifest digest, fixture revision, server build, map and start time. Do not reset a prepared run every time `report` is called. Persist the controller's state/evidence outside the plugins so map changes and unload/reload do not erase the proof.

Add `scripts/khook_acceptance.py` as the single case registry, evidence validator and judge; `scripts/test-khook-live.sh` remains the entry point. Native and JS output uses JSON serialization/escaping and agrees with this record shape:

```text
schema=1; suite=A; run_id; case; producer=native|js|human;
result=pass|fail|pending; expected; actual; evidence (nonempty for pass/fail)
```

One case may require multiple named subchecks from different producers. The judge aggregates those required observations; it does not overwrite a native pending/failure record with a JS success string. Require an envelope for each of the twelve top-level cases; a missing envelope is invalid, preserving the existing runner's completeness check. Reject wrong-run/wrong-revision records, duplicate producer-subchecks, malformed records, contradictory status fields and unsupported case names. Any observed failed subcheck fails the case. A present case with required evidence still waiting remains pending. Exit 0 only for complete pass, 1 for failed/invalid evidence, 2 for incomplete evidence.

`--from-file` uses this exact judge. Reports must include expected and observed values, rather than treating `result=pass` as sufficient. Historical records cannot pass a new revision. Host parser self-tests use valid and deliberately corrupted records and run in native CI.

### 7.2 Automated stateful cases

- **Entity selection:** hook only A; independently spawn valid A and B; assert A=1, B=0 and both engine actions succeeded. Test filtering failure with B delivered and registration failure with neither delivered. Clean up both entities; repeated prepare cannot leak or reuse counters.
- **Phase removal:** drive a known schedulable entity or controlled native invocation that exercises the actual SDKHooks adapter. Run PRE-remove/POST-survives, POST-remove/PRE-survives, self-unsubscribe inside PRE, and final unsubscribe. Check exact callback and original counts after each step. A Dummy Virtual test is supporting evidence, not a substitute for the SDKHooks adapter.
- **Deletion/reuse/map/teardown:** save entity identity, remove it, force or boundedly seek slot reuse, and prove the old subscription never fires for the new serial. If reuse cannot be achieved, keep that subcheck pending. Persist the run while changing map and unloading/reloading the JS plugin; verify old callbacks gone and new subscriptions deliver once. Native s2script/probe unload evidence also belongs here. No cached pre-map counter can satisfy a post-map assertion.

### 7.3 Human-assisted cases

The runner has distinct prepare/collect/judge phases and documented client actions. Add `--observations FILE` for structured human observations bound to the same run, identities, case/subcheck, actors and evidence paths. Human observations supplement required native/JS assertions, never override them. The implementation agent must not author passing human observations for actions it has not observed.

- **Voice:** use three clients so one listener is allowed and another denied for a single speaking sender; then remove the policy and verify the formerly denied listener hears the sender. Native observation confirms the effective listen bit after processing and one original execution; human evidence confirms allowed/denied/unmuted audio. Stats alone do not pass.
- **Transmit:** use an actually networked visible entity in both clients' PVS, not `logic_relay`. Allow client A, deny B, then restore visibility. Require first-fire layout validation, native per-recipient filtering observations and both clients' visibility observations. Avoid an entity without a networked renderable when judging visibility.
- **FireEvent recipients:** target a strict subset of at least two real clients, plus an all-suppressed scenario. Use an observable game event with valid payload. Record server/original delivery, the actual outgoing recipient decisions and client receipt/nonreceipt; client console/listener instrumentation is acceptable. Distinguish the no-suppression case from CallOriginal+Supersede. Sending to every human is not a mask test.

Describe how client evidence is captured and retained in the probe README. Missing clients or missing observations is pending with the exact next action; completed human checks can produce a final result without source edits.

### 7.4 Independent frame, lifecycle and command proof

For `frame_client_command_hooks`, require both native engine observation and s2script's JS callback delivery for the same run. Join client identity/generation, not just unrelated monotonically increasing counts.

The probe observes command behavior with Ignore; it must not Supersede on s2script's behalf. Use separate command tokens for continue and handled. A test-only engine command callback owned by the probe counts original delivery; the JS client-command hook chooses the requested result. Remove the fixture's competing JS command registration for this command name; only the probe owns its engine callback. A real client issues the token (RCON-only delivery cannot stand in for ClientCommand). PRE/POST observation records the target token and actual skip state.

First verify that the chosen real-client command actually traverses ClientCommand and the proposed original witness. A registered ConCommand can take a different engine path. If that occurs, use a valid engine command and an independent witness at its validated original boundary through KHook; record the observed route in the runbook. Never label DispatchConCommand-only delivery as ClientCommand evidence.

Expected: Continue -> JS once, probe PRE/POST once, engine callback once, skipped=false. Handled -> JS once, probe PRE/POST once, engine callback zero, skipped=true. Frame/public and connection/public joins are also mandatory. Negative controls disable the JS handler, reverse its decision, and omit the acceptance plugin: each must fail the appropriate subcheck rather than inherit a probe-generated pass. The controller records observed plugin/handler absence as a failed prerequisite; an observation not yet collected remains pending. Keep the separate peer-precedence tests as their own case, and run both plugin load orders.

## 8. Completion and handoff

All five findings need a reviewed code change and a regression that fails the reviewed baseline for the stated reason. The final gate is `make ci`, sniper builds of corrected host/shim/probe, and suite A on the same recorded build with both plugin load orders and the required human observations. Finish the operator docs and notices with that artifact identity. The full migration plan's additional PR A requirements remain in force.

The documentation authoring, implementation completion, host/CI validation and live acceptance are separate states. Keep PR #221 draft and PR B blocked while required evidence is pending. Do not describe this specification or a green parser self-test as a completed live gate.
