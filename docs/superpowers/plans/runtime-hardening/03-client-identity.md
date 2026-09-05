# Slice 3: Bind Client to a connection lifetime

**Status:** Implemented and locally verified; independent review and final Linux/live acceptance pending.
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
- [ ] Wire the new plugin test into scripts/ci-js.sh. Run core, SDK, plugin typecheck, and live
  disconnect/reconnect tests; confirm normal connect, disconnect, and map-change behavior.

## Evidence required before completion

- [x] Record the regression test and its failure on the parent implementation.
- [x] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.

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

## Remaining acceptance

Independent review, a final Linux native gate/shim build, and live CS2 disconnect/reconnect and
map-transition checks are pending on this commit. Live evidence must cover actual engine hook
ordering, late-load occupancy including existing clients, deferred disconnect identity, and
same-account replacement isolation. Exact in-game signon recovery for late-loaded clients is
not claimed: bootstrap intentionally reports connected until an observed phase hook advances it.
The controller owns final evidence and descendant restacking after review; this work does not
restack, push, or mark the slice complete.

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
