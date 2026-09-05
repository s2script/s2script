# Slice 3: Bind Client to a connection lifetime

**Status:** Complete for automated acceptance; human-client limitations are recorded below.
**Branch:** `core/hardening-03-client-identity`
**Parent / PR base:** `core/hardening-02-channels`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/client.rs, core/js/prelude.js, packages/sdk/clients.d.ts,
core/src/cookies.rs, plugins/clientprefs/src/plugin.ts; inspect shim/src/s2script_mm.cpp's
client lifecycle bookkeeping and games/cs2/js/pawn.js consumers.
**Tests:** client.rs, cookies.rs, v8host.rs; create plugins/clientprefs/src/plugin.test.mjs.

**Boundary:** A connection token belongs to the host's client-liveness books, not SteamID alone.
Preserve the departing identity through its disconnect callback, then invalidate it. Account
for clients already present at runtime initialization and engine map-transition semantics.

- [x] Add tests where A disconnects, B occupies the same slot, and a saved Client attempts
  kick/chat/command/voice operations. None may affect B; isValid must be false for the old handle.
- [x] Add tests for the same SteamID reconnecting, late cookie query completion, a cached event
  queued before slot reuse but dispatched afterward, and disconnect-handler identity access.
- [x] Mint a host connection generation on a new connection. Capture it when constructing Client
  and gate getters and actions against it. Document stale-access return values consistently
  with the existing API's safe-access conventions.
- [x] Carry slot plus connection generation through the pending cookie notification queue and
  recheck at dispatch. Drop stale loads before cache mutation, not only before notification.
- [x] Inspect generated/manual Player wrappers and retained Client uses; update affected callers
  and classify SDK/host API compatibility together. Do not hand-edit generated files.
- [x] Wire the new plugin test into scripts/ci-js.sh. Run core, SDK, plugin typecheck, and live
  disconnect/reconnect tests; confirm normal connect, disconnect, and map-change behavior.

## Evidence required before completion

- [x] Record the regression test and its failure on the parent implementation.
- [x] Record implementation commits and passing focused checks.
- [x] Record applicable full-gate and live-server results, with environment limitations stated.
- [x] Review the diff against the parent and restack descendants using recorded old tips.
- [x] Set status to complete only when this slice's required gates pass.

## Implementation and compatibility

This branch's implementation commit adds a separate host `LiveTable` for connection generations,
plus versioned lifecycle exports and owned deferred disconnect snapshots. The engine-ops table
is unchanged. Old immediate lifecycle exports remain supported; unsafe slot-only replay drops
with a named compatibility warning. A map-start notification preserves connection generations;
real reconnects mint a new generation even for the same SteamID. Late-load bootstrap uses the
engine's unsigned userid sentinel (`65535`), with conservative connected signon state.

`Client` captures an immutable slot and private generation. Native checks fence getters/actions,
including voice, after argument coercion. Disconnect callbacks receive a read-only, synchronous
identity snapshot and an already-invalid handle. The manual `Player` wrapper gates its controller
ref, generated schema setters, and retained navigation views without editing generated files.
Delayed kicks, menu/vote cleanup, and Tab input retain or check connection ownership.

Online cookie caches and cached notifications carry the originating connection. Disconnect
detaches dirty rows into one reset-owned retired-save seam; clientprefs serializes account work
so a replacement load waits for those writes. Stale loads cannot commit, and pending loads
preserve newly authored dirty values. **This slice does not provide bounded retention, retry,
revision/ack ownership, or persistence durability.** Slice 5 upgrades this exact retired-save
seam; it must not introduce a competing outbox.

Compatibility is a bugfix with unchanged public signatures and host apiVersion 2. The SDK and
CS2 package have patch changesets. Clientprefs is an ignored runtime plugin in the repository's
changeset policy; its source and rebuilt archive participate in the same runtime release.
There is no public Client language getter on this branch; the raw language native is guarded
when given a connection token, and the existing immediate translation lookup remains slot-based.

## Local verification (macOS arm64, 2026-09-04)

- Core: **672 passed, 0 failed**, using the documented scratch linker wrapper that removes
  Linux-only `-Wl,-z,nodelete`. This is host test evidence, not a Linux or CS2 binary gate.
- Full `scripts/ci-js.sh`: **passed**, including 566 SDK tests, plugin/example typechecks,
  generated-file freshness, core JS lint, 69 UI component tests, 3 clientprefs race tests,
  and the Docker-dependent gate script. Local mode skips `npm ci` as the script specifies.
- `scripts/build-base-plugins.sh`: **passed**, including clientprefs.
- `scripts/test-defer-queue.sh`: **passed** with ASan/UBSan; copies disconnect payload strings
  and exact 64-bit tokens into the real deferred queue.
- `scripts/test-client-bootstrap.sh`: **passed**; empty/bot/human/unauthenticated slots,
  unsigned sentinel, bounded scan, conservative signon, and repeat ensure preservation.
  The new gate is wired into `scripts/ci-native.sh`.
- Core boundary and `git diff --check`: **passed**. Engine-ops generation reports 126 unchanged
  operations. Existing sentinel/order shell parsers fail on macOS BSD sed despite unchanged
  declarations; final Linux execution of both scripts remains required.

Scratch logs under `.superpowers/sdd/2026-09-04-runtime-hardening/task-3-*.log` retain the prior
red evidence for the stale Client, Player controller reuse and clientprefs query races. Additional
self-review regressions failed before fixing old-disconnect HUD cleanup and Player numeric
coercion. Tests also cover valid-action controls, stale voice/getter defaults, deferred snapshot
scope, per-subscriber lifecycle/cookie fences, shared engine string scratch, and pending-kick
coercion preserving the replacement's record.

## Acceptance scope

Automated core/SDK regressions and the isolated live bot gate cover connection identity,
same-slot replacement, same-SteamID mocked reconnects, stale delayed work, disconnect snapshots,
existing-client discovery, and engine map reconciliation. Human map continuity, authenticated
same-account reconnects, and visual chat/console receipt were not available to automate. They are
not inferred from bot behavior. Bootstrap intentionally reports only an observed phase.

## Review round 1

Fix commit: `04913049c9344ef7a716e43c8ce3c7422123302e`. Addresses the five findings in the
independent review of `3d96aca`:

- Register `CLIENT_CONNECTIONS` with the process singletons after their registration list resets,
  rather than from the owner-store registration path. The coverage assertion requires the entry;
  a shutdown/re-init regression proves empty books and a still-monotonic allocator.
- Replace the partial Player setter facade with an explicit argument plan for every shipped
  EntityRef method. Scalar values, paths/vectors, strings, keyvalue getters and input actor refs
  finish access/coercion before the connection check. Shared wrapper functions avoid per-lookup
  allocation of the complete method inventory; retained reads/actions use their documented stale
  returns. A new unplanned EntityRef method fails closed with a named error. No generated file
  changed.
- Bind HUD cache buckets and retained slot views to their originating Client. Replacement access
  clears the previous cache/disabled/cursor/view state before using it; disconnect cleanup compares
  the departing generation and cannot clear a replacement bucket. Retained views fail closed.
- Make game-package fixtures explicitly seed connected slots. Unseeded construction is invalid,
  enumeration is empty, and fromSlot returns null. Connect/retire/replace controls never reuse a
  token. Existing tests now seed only the client population they model.
- Bind admin-menu sheet/freeze records and RTV voters to Clients. Admin-menu restoration keeps
  the original pawn and connection; stale voters are removed before threshold decisions. Old
  disconnect callbacks cannot delete the replacement's state. The plugin regressions run in CI.

Round 1 local gates passed: **673 core tests**, full **`ci-js.sh`** (**582 SDK tests**, 69 UI
component tests, 3 clientprefs tests, and 2 admin-menu/RTV tests), plugin/example typechecks,
base-plugin archive builds, core boundary, and `git diff --check`.

Exact logs are retained in `.superpowers/sdd/2026-09-04-runtime-hardening/`:
`task-3-round1-core.log`, `task-3-round1-ci-js.log`, `task-3-round1-build-plugins.log`, and the
focused `task-3-round1-*.log` files. Red evidence covers reset leakage, the optimistic fixture,
HUD same-value suppression, inherited ref operations, and plugin ownership. The final ref test
was also run against the pre-fix Player source without changing production files: 13 failures
(inventory plus 12 unsafe inherited paths) and one passing spawn control. The current facade
passes all 14 tests against the real EntityRef prelude, including the complete method inventory.

The controller reports that the pre-review `3d96aca` Linux native gate and release build passed,
but those binaries were not installed. **That does not validate the round 1 fix.** Independent
re-review and final Linux/live checks remain pending on `0491304` and its evidence commit.
Cookie persistence/admission remains the single Task 5 seam described above.

## Live map regression correction

The live bot map gate found that a map can remove a client without delivering a disconnect hook.
After `changelevel`, slot 10 retained a generation and tracked full signon even though the engine
reported an empty name and unsigned absent user ID 65535. The earlier map-preservation regression
did not model this divergence between engine occupancy and tracked lifecycle state.

The shim now reconciles all 64 slots in its **POST StartupServer** hook, before map-start JS.
It reads the typed engine user-ID sentinel directly. Occupied slots keep their generation,
connection policy, and observed signon phase; only an unknown phase is raised to connected.
Absent slots clear shim signon/mute/hearability state and call the existing
generation-checked core end entry, retiring voice rules and cookie sessions. No synthetic disconnect
event is emitted with an already-lost identity. Deferred map replay does not repeat reconciliation.

Local evidence: the old occupancy-only scan fails the new phantom-token assertion; the corrected
engine-free C++ test passes absent-slot retirement, user ID 0, and stable survivor identity. Its
original phase-reset expectation was corrected in the follow-up below. A core test exercises the
exact end/ensure entry points and verifies dirty-cookie retirement,
stale cached-event rejection, survivor cookie/voice preservation, and safe late retirement after
slot reuse. **674 core tests**, **52 focused SDK/Player/HUD tests**, the client-bootstrap and defer
queue C++ tests, core boundary, and `git diff --check` pass. Exact logs are
`task-3-round2-{map-red,map-green,core-targeted,core,sdk,defer,boundary-green}.log` in the scratch
directory used above. An initial boundary command used a nonexistent script name; the correct
`check-core-boundary.sh` passed, with both outputs retained.

Independent review, Linux shim/native validation and a repeated live map gate remain required on
this correction. Human clients are unavailable in the live fixture: the non-sentinel survivor test
proves the reconciliation policy, but the bot trace does **not** prove human engine hook continuity
through a map transition. Task 5's single persistence seam and durability limits are unchanged.

## Review round 3 — retain observed survivor phase

Review of `b66058a` found that unconditional signon reset could strand an active survivor at
connected when `StartupServer` repeats without later active callbacks. Immediate kicks require
signon >= 4, and both transmit paths require full signon. Reconciliation now uses the same
conditional connected floor as late-load bootstrap, preserving every already-observed phase.

The C++ regression fails against the downgrade and passes after correction. It reconciles twice
without intervening lifecycle hooks, checking stable full/spawn phases, tokens, mute and hearability
state; an occupied previously unknown slot is initialized once, and absent slots still retire.
Logs: `task-3-round3-phase-{red,green}.log` and `task-3-round3-client-core.log` in shared scratch.
The focused client core tests pass; the prior 674-test core suite remains the unchanged-core
baseline. Independent re-review and final Linux/live gates remain pending on this follow-up.

## Final automated acceptance (2026-09-05)

- Independent review: all five initial findings and both map-boundary findings closed. Final
  focused review approved spec and quality at `db2cf0c0d8d45f9911d4473b673203e67a2d3918`.
- Final core: **674 tests passed**. Full Linux `ci-native.sh` passed at map fix `b66058a`,
  including shim build, engine symbols, and sanitizer selftests. The final phase-only fix
  `db2cf0c` passed the C++ reconciliation suite, 19 focused core tests, release shim rebuild,
  and symbol validation against the installed game. Core production code was unchanged by
  that last fix. Earlier full JS gate passed 582 SDK tests, 69 UI, 3 clientprefs and 2 plugin
  ownership tests; subsequent map fixes changed no JS.
- Installed only on Nebula's isolated `s2script-cs2-hardening`, RCON 27016. Core SHA256:
  `843b9d2b85eb871bacf1464b9b5448e61a3ad733c0cad9ad7663885658e1b758`;
  shim SHA256: `0f0c601590168707d7b44e7207f23ffe8478299d50eced60ebd31967ee6fe822`.
- Repeated 16 controlled bot cycles, then actual same-slot replacement at slot 6/userid 262:
  stale command/fakeCommand returned false, voice unchanged, replacement remained valid;
  fresh voice toggled/restored and fakeCommand succeeded. Earlier fresh kick proof and
  synchronous nonempty disconnect snapshots passed. Bot `command()` can return true for
  engine acceptance; this does not prove rendered console delivery.
- `changelevel de_inferno` reproduced the original failure before correction: an absent slot
  retained a valid handle with userid 65535. With the final fix, map-start logged
  `live=0 retained=[6:false:-1]`; then the two newly connected bots were the only live clients.
  All 14 base plugins plus two disposable probes remained running afterward.
- Descendants were restacked from recorded exact bases after initial review. Final map commits
  are being carried through the remaining in-progress stack; final integration/soak belongs
  to the completed stack gate, not this slice's isolated acceptance.

Detailed logs are retained in the controller's plan scratch `client-live/evidence-round1/`
and `/tmp/s2script-hardening-slice3-round3-*.log`. No production server was modified.
