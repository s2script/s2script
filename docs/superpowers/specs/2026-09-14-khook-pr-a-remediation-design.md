# KHook PR A remediation — design spec

**Status:** Stock-host requirements confirmed; implementation and live acceptance are incomplete.
**Decision:** [Stock Metamod and resident s2script](2026-09-15-khook-stock-host-decision.md).
**Execution:** [Current dynamic work packages](../plans/2026-09-15-khook-pr-a-review-fixes.md).
**PR:** [#221](https://github.com/s2script/s2script/pull/221), based on the documentation branch in [#220](https://github.com/s2script/s2script/pull/220).

## Outcome and scope

Complete the KHook cutover on unmodified upstream Metamod. The native s2script shim
stays loaded during gameplay and manages JavaScript `.s2sp` load, unload and hot
reload internally. Metamod has no responsibility for those JavaScript lifecycles.
Native shim updates require a server restart; native hot unload/reload and native
library unmapping are not acceptance requirements.

This replaces earlier remediation requirements for a host patch, private runtime
build, patch-set identity and native reload driver. Preserve the existing JS API,
HookResult semantics, generic core/game boundary and PR A/B/C migration split. Do
not migrate damage/inline hooks or precache as part of this remediation. PR B stays
blocked until the amended PR A gate passes. No merge or release is authorized.

## Constraints

- Use stock Metamod and bundled KHook through their public API. No dependency
  patches, writes into private host ownership structures or second hook engine.
- The source compatibility baseline remains Metamod
  `7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33` and nested KHook
  `1e200e4cc8e0badcb7cf941525268d6977f6a4e6`. Do not silently change gitlinks.
- Linux x86_64, PLAPI 18 and GLIBC <= 2.31 for deployed binaries. macOS harnesses
  are useful unit evidence, not real server or native trampoline validation.
- `PLUGIN_SAVEVARS` runs before registering hooks. Registration failures remain
  named and observable. Keep PRE/POST, entity filtering and peer precedence.
- The native shim/provider can retain shared hooks while individual `.s2sp`
  subscriptions come and go. Removing a routing filter is not physical unhooking.
- Script-owned subscriptions and resources use the resource ledger and must remain
  safe even when outgoing cleanup code is missing or fails. `createEntity` creates
  game-world-owned entities, which follow the SDK's explicit cleanup contract; do
  not add automatic entity ownership as part of this migration.

## Lifetime responsibilities

### JavaScript plugins

During reload, stop outgoing callbacks, retire that plugin's subscriptions and
owned resources, and activate the replacement through the existing core lifecycle.
The native shim and Metamod provider remain resident. Keep shared native hooks
available to peers and replacement JavaScript subscriptions. Preserve both phase
removal orders, self-unsubscribe, entity deletion/serial reuse and map transitions.

Do not call whole-shim retirement because the last JS subscriber leaves a hook.
Checked binding state remains useful for failed installation and safe ownership;
remove machinery that exists solely to unload/reload the native shim.

### Native teardown and process shutdown

Ordinary native unload during gameplay is unsupported and must reject without
starting whole-shim retirement. Do not leave a running server with a partially
retired s2script instance. Safe server shutdown and partial-install/degraded-load
ownership remain required. The existing core-init failure path returns true and
stays loaded for diagnosis; test that actual path rather than inventing a failed
load return. These responsibilities are separate from JavaScript hot reload.
Rejecting ordinary native unload cannot be used as
proof of safe process shutdown: stock Metamod forces plugin removal before shutting
KHook down, and its `Unload` callback has no force/shutdown argument.

The current async false/retry coordinator does not establish terminal safety.
Determine a real engine lifecycle boundary where callbacks and intercepted
originals have returned, engine resources remain valid, and the host provider is
still alive. Only then may synchronous physical cleanup be appropriate. A callback
counter returning to zero does not account for an original executing between PRE
and POST. Never synchronously remove a hook whose original is on the same stack.

The SDK exposes `ISource2ServerConfig::PreShutdown/Shutdown`, but source headers
alone do not prove the required CS2 order. A staged cleanup using an earlier engine
shutdown phase is a candidate to validate, not an approved implementation shortcut.
Do not add speculative shutdown hooks or claim an untested ordering is guaranteed.
Capture ordering and quiescence on a live server before selecting that path.
Record the actual CS2 child-process outcome as well as the container status. The
Nebula `e6b89d9` run reached `Source2Shutdown` after RCON `quit`, then the child
segfaulted while its wrapper returned zero. A zero wrapper exit is therefore not
shutdown acceptance. Its saved registers, matching loader disassembly, scanned
host frames and source audit support null-handle cleanup of the failed probe
record. The probe had not loaded. This is separate from the source-level
hook-retirement gaps; diagnose any further fault before choosing a lifecycle change.

Retain typed helpers and callback contexts until physical removal is complete.
Finalize helper bookkeeping while the provider lives. Completed Virtual reverse
IDs must not trigger redundant backend calls at static destruction. Preserve
cleanup for live IDs. Do not use receipt publication as proof that the receipt
callback has returned. No native library release requirement justifies an upstream
patch in this scope. Unsupported busy/forced paths must remain explicit; idle
process shutdown still needs actual acceptance evidence.

## Review corrections to preserve

| Finding | Required behavior |
| --- | --- |
| F1: early context/core destruction | Correct script resource ownership and native shutdown lifetime on stock host; no native reload requirement |
| F2: FireEvent POST dereferences consumed event | Correlate through caller-owned invocation state; never read the consumed event in POST |
| F3: arbitrary binary accepted as verified host | Validate actual artifacts and provenance before installation and before skipping an existing install |
| F4: acceptance stages permanently pending | Provide executable state transitions and explicit ingestion of human observations |
| F5: probe counters substitute for s2script delivery | Join independent native observations with actual JS delivery for the same run and subject |

For FireEvent, use a caller-owned nested invocation scope around the controlled
engine call. Count PRE/POST, original skip and real listener delivery without
reading freed event memory. Distinguish KHook automatic-original suppression from
an explicit CallOriginal followed by Supersede. Cover consuming and nested
originals with the actual observer code under sanitizers.

ClientCommand proof must traverse the real client engine path. An unregistered
controlled command may exercise ClientCommand; registered DispatchConCommand
traffic alone cannot satisfy it. Continue requires JS once and original once;
Handled requires JS once and original zero. Use a separate original witness,
negative controls and both peer load orders. Preserve the shim's fallback listener
dispatch without double-delivering the registered command path.

## Stock host delivery and identity

Operators may install an official stock Metamod release. An isolated, unmodified
source build remains an optional reproducible validation tool. No s2script custom
Metamod build is required. Remove the patch series, application step, patch-set
checksum and altered-host notices together with the private-host requirement.

Use manifest schema 2 with `schema`, `plapi`, `target`, `glibc_max`, `provenance`
and `artifacts` (relative `path` plus SHA-256). Provenance is either:

- `unmodified-source` with the checked full Metamod and nested KHook source SHAs;
- `official-release` with the official AlliedModders HTTPS archive URL and an
  independently expected archive SHA-256, checked before extraction.

Do not describe an operator-supplied checksum as an upstream signature. Source
builds derive PLAPI from checked headers. The default official release is
`2.0.0.1467`, whose upstream tag resolves to the checked Metamod pin, with its
published asset SHA-256. This fixed default derives PLAPI from that checked source;
arbitrary release overrides require explicit operator-confirmed compatibility.
Actual runtime load/handshake validation remains required. Archive hashes and ELF structure alone do not establish the ABI. Keep
source identity and stock-release provenance distinct. Safe extraction must reject path
escapes and unsafe links. Require valid x86_64 shared ELF objects, complete loader
layout, appropriate GLIBC requirements and matching file hashes. Empty, malformed,
truncated or arbitrary files fail. Manifest creation belongs to verified preparation
or successful build, not the installer accepting whatever candidate it sees.
The probe build must also reject unexpected unresolved relocations. Its SDK
support definitions must be linked into the probe itself; the live `e6b89d9` load
exposed a missing `MurmurHash2LowerCase` definition despite successful ELF checks.
The explicit host-provided `g_pMemAlloc` exception still needs real process loading.
Controlled native calls must remain observable under optimization: a direct call
to a target defined in the same translation unit can let the compiler disregard
runtime detour effects. Verdict evidence must expose every checked count and
return value as valid JSON. Independent Function observers must not overwrite
signature bytes before the plugin under test resolves its descriptors.
The probe must use public level-lifecycle notifications to invalidate world-owned
entity references and prohibit entity lookup/removal after level shutdown.
Track ownership separately for targets created after a map change. World lifetime
must not gate cleanup of the engine-lifetime command registry; stock Metamod still
uses that registry during plugin removal. Keep the controlled game-event listener
scoped to its synchronous invocation, with cleanup on every exit.
This test-fixture rule does not establish a terminal cleanup boundary for the shim.

Preserve running-server refusal, destination-byte verification before skipping,
staging before replacement and rollback on failure. Failed builds invalidate stale
success receipts. Source preparation must not inherit the parent repository's Git
identity or mutate vendored source. Update BUILDING, INSTALL, CI and generated
notices consistently.

Acceptance binds a run to actual installed shim/core/probe/fixture artifacts,
verified host manifest digest, source/build identity, server build and initial map.
A controller checkout SHA cannot stand in for binary identity. Missing identity is
pending; supplied malformed or mismatched identity fails. Freeze the initial map
identity while recording later map transitions separately.

Generate test bundle provenance only while freshly building all four artifacts
from an unchanged clean source revision. The active JS fixture must report an
embedded build revision/token that matches its archive's bundle record; copying a
probe cvar or hashing an arbitrary installed archive does not establish this.
Native runtime witnesses must identify actual mapped module device/inode values,
including Metamod's loader and game module, and reject replaced or ambiguous
files. The generator compares installed files and both runtime witnesses before
and after hashing, retains captured evidence, and emits the existing unsigned
operator receipt only after successful validation. This tooling is specific to
the acceptance fixtures and requires no production SDK/core or upstream changes.

## Acceptance evidence

Keep the twelve existing case names. Replace the `native_unload_reload` subcheck
under `entity_slot_reuse_map_teardown` with `script_hot_reload`. Remove required
native probe/shim unload and native resume stages from the runbook.

The script reload proof uses one persistent native target and one run/artifact
identity across an actual `.s2sp` reload. Explicitly arm the check, observe the old
JS generation, retain native target identity, then observe replacement PRE/POST
callbacks and one independent original invocation. Require a changed JS generation,
exact new callback counts, zero old callbacks and removal of the old marker entity.
The dedicated test subscriptions are left to owner-ledger cleanup; do not manually
unsubscribe them in OnPluginEnd. The game-world marker separately proves explicit
OnPluginEnd entity cleanup, not automatic entity-ledger disposal. Do not infer reload from a reset counter.

A JS-only resume restores run binding without replaying completed filter/phase
checks. Negative controls cover unchanged generation, stale/duplicate/missing
callbacks, missing original and an old resource that survives teardown.

Retain other required observations:

- Final unsubscribe needs a real native invocation after both phase removals,
  exact PRE/POST/original counts and self-unsubscribe POST behavior.
- Entity deletion, serial reuse, map changes and resource teardown remain distinct
  stages with native acknowledgements bound to run, entity and stage.
- Voice combines effective engine arguments, stored listen state and original
  execution with human allow/deny/restore observations.
- Transmit uses a visible networked entity in both clients' PVS; keep it alive
  until deny/restore visibility observations are captured.
- Recipient masks require real client receipt/nonreceipt for a strict subset and
  an all-suppressed case with valid event payloads and independent outgoing proof.

The controller preserves append-only raw history. Observed failures and malformed
records remain failures; identical replay is idempotent and pending may progress.
Offline and live judging share validation. Supplied identity must be validated even
when no raw response exists. Validate bounded command-safe run IDs before any RCON
or filesystem side effect. Human evidence needs explicit expected and actual
outcomes, matching run/artifact identity and evidence references; never invent it.

## Completion

Implementations, unit/CI validation and live acceptance are separate states. Keep
PR #221 draft until required stock-host builds, full applicable gates, both live
plugin orders, real script reload and process shutdown pass. Missing authorized
server access or real-client evidence stays pending with the next executable step.
No native hot-unload gate or host patch may be reintroduced to resolve that pending
status. The current plan records owners, dependencies, regressions and reviews.
