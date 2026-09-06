# UI API Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make multi-plugin UI interactions reliable and easier to author, then reduce redundant rendering work with measured evidence.

**Architecture:** Preserve `CustomHudLayout` as the low-level driver and `hudkit` as the component layer. Put cross-plugin ownership in a generic host registry; keep connection-bound view state, snapshot dispatch, and rendering in the CS2 package. Deliver additive APIs in individually testable slices.

**Tech Stack:** Rust/V8 host, ES5-compatible injected JavaScript, TypeScript declarations, node:test, existing npm gates, Docker CS2 on Nebula.

**Spec:** [UI API design](../specs/2026-09-05-ui-api-hardening-design.md). Read it before this plan.

**Status:** Local implementation and final evidence are verified at source commit `102ff64267a74b010a83171477edd7e374425ebe`. Tasks 0–6 and Task 7's local fixtures, gates, churn, and reviews are complete. Owned-server staging, human acceptance, and final publication remain root-owned and pending.

## Global Constraints

- Core is engine-generic; CS2 layout names, input policy, and rendering semantics remain in the game package.
- Cross-plugin state belongs in the host, never a plugin context's `globalThis`.
- The ledger is the teardown authority; cleanup cannot depend on plugin callbacks running.
- No raw pointer, engine event view, or cross-plugin JS object may survive a callback.
- Existing public methods keep their return types, timing, and documented disabled-row behavior. New semantics use additive methods/options; incompatible changes require a separately declared major version.
- No new runtime dependency or workshop markup change is required for the initial implementation.
- No production deployment, merge, or automatic restart of `s2script-hudlab` is part of this plan.
- Offline tests and bots do not prove human rendering, click delivery, focus, or spectator privacy.

## Coordination, models, and Git

Root owns the baseline, API contract, integration, server, and final report. Use at most three workers beside root. The assignments below use models exposed by this session; they are a proposed allocation, not a measured cost/speed guarantee. Recheck availability before dispatch.

- **GPT-5.6 Sol, high:** default implementation worker for snapshot dispatch, lifetimes, results, and repaint scheduling.
- **GPT-6 Astra, high:** host arbitration/focus worker and independent reviewer for ownership, lifecycle, and reentrancy.
- **GPT-5.6 Luna, medium:** bounded documentation, compile-time examples, and fixture updates after contracts are frozen. Escalate semantic failures to Sol rather than repeatedly retrying.
- **Root:** specification review and conflict resolution. A different worker reviews implementation quality; an implementer never approves their own slice.

Use a fresh assignment per slice with only the spec, task, baseline SHA, relevant files, and predecessor report. Do not fork the entire historical chat into each worker. After one failed review, send the exact reproducer; after a second failure or a host-boundary uncertainty, escalate to Astra. Avoid simultaneous duplicate broad reviews and repeated full gates without new changes.

One worker per worktree. Root alone changes branch bases or integrates commits. No shared-checkout editing of `components.js`, `ui.js`, or `packages/cs2/ui.d.ts` by concurrent agents. Independent test-design/documentation work can overlap implementation; source slices touching these files integrate sequentially.

Proposed PR order, all branches using `codex/`:

1. `codex/ui-01-click-snapshots`
2. `codex/ui-02-component-lifetimes`
3. `codex/ui-03-results-readiness`
4. `codex/ui-04-focus-ownership`
5. `codex/ui-05-coalesced-refresh`
6. `codex/ui-06-owned-surfaces-events`
7. `codex/ui-07-acceptance`

Each PR targets its predecessor, contains its implementation/types/tests/docs together, and passes the relevant gates. Per the user's explicit instruction, continue the existing stack: UI slice 1 targets `refactor/hardening-10-host-modules` (#195), after reconciling all ten original slices with `main`. Preserve original PR numbers. Publication of the continued draft stack is authorized; merging is not.

## Task 0: Reconcile and verify the baseline

**Owner:** root; independent Astra review of conflict resolutions.

**Files:** existing hardening branches; `core/src/v8host.rs` and extracted host modules, `games/cs2/js/ui.js`, `components.js`, `menuhud.js`, `voterail.js`, `hudinput.js`, `core/src/shared_entity_switch.rs`, `core/src/ui_pool.rs`, and their tests.

**Consumes:** current `origin/main` and the published #186–#195 stack. **Produces:** one recorded integrated SHA containing both upstream UI fixes and hardening lifetimes, with clean tests.

- [x] Fetch and inspect current heads; create a recoverable Git bundle before any restack. Preserve unrelated work and record original PR bases/SHAs.
- [x] Reconcile the existing stack using the repository's normal Git workflow. Do not choose an entire file from one side of a conflict: preserve shared capture ownership, pool arbitration, entity-cache invalidation, disconnect painting cleanup, and stale-client fencing together.
- [x] Run the focused UI gate below, full JS gate, and native gate because host files change. Capture exact failures and fix conflict-induced regressions before any UI implementation assignment.
- [x] Record the integrated SHA and new worktree paths in the execution ledger. Treat the MAM server-side mount/map crash as an independent unresolved incident, not proof of a UI API defect or a fixed baseline.

Commands, from repository root:

```bash
bash scripts/check-components-test.sh
node --test games/cs2/js/ui.test.js
node --test packages/sdk/test/cs2-ui.test.mjs
bash scripts/ci-js.sh
bash scripts/ci-native.sh
```

Use the existing Linux Docker build environment for native checks when needed; Docker is available on the Mac. Follow `CLAUDE.md` for deployable sniper binaries. Do not infer that host-linked binaries are server-loadable.

## Task 1: Route clicks through submitted row snapshots

**Worker:** Sol/high. **Depends on:** Task 0.

**Modify:** `games/cs2/js/components.js`, `packages/cs2/ui.d.ts`, `games/cs2/js/components.test.js`, `games/cs2/js/hudkit-prelude.test.js`.

**Interfaces:** add `readonly id?: string` to `Row`; preserve the existing `onPick(slot, index, row, view)` signature. Internal modal/dashboard state owns `paintedRows`, `paintedOffset`, `paintedTabId`, `footerFns`, and `interactive`. Snapshot records contain copies of declared row fields. Later tasks must invalidate these records before deferred/lifecycle cleanup.

- [x] Add this regression using the existing `mount()` fixture in `components.test.js`, then run it and confirm the pre-fix failure is wrong identity, not a fixture error:

```js
test("a row click uses the submitted row after provider reorder", () => {
  const { ui, clickHandlers } = mount();
  let rows = [{ id: "a", a: "A" }, { id: "b", a: "B" }];
  const picked = [];
  ui.modal({ title: "Pick", rows: () => rows,
    onPick: (_slot, _index, row) => picked.push(row.id) }).open(1);
  rows = [rows[1], rows[0]];
  clickHandlers["s2_m0_r0"](1);
  assert.deepStrictEqual(picked, ["a"]);
});
```

- [x] Add dashboard reorder, in-place row mutation, two-player pages, duplicate supplied IDs, hidden/out-of-range row, and failed-paint regressions. Preserve the existing modal cosmetic-disabled test and dashboard disabled rejection.
- [x] Compute one candidate page/action snapshot before painting, including dashboard tab IDs and actions. On complete success commit it; on any drive error disable interactive dispatch until a successful repaint. Keep the prior data snapshot for recovery, but do not leave it clickable over partially changed visuals. Copy footer action arrays rather than modifying the committed array during paint. A reentrant paint must not let an older outer candidate overwrite a newer committed generation.
- [x] Change row dispatch to read committed records. Pass the stored absolute index; do not use it to re-index `rowsFor(slot)`. Document stable-ID domain revalidation and the missing client render-revision limitation.
- [x] Run the focused UI gate; have an independent reviewer test mutation/reentrancy. Commit as `fix(cs2): bind UI clicks to submitted row snapshots` with a package changeset where required.

## Task 2: Bind component views and callbacks to lifetimes

**Worker:** Sol/high. **Depends on:** Task 1; Astra reviews lifecycle paths.

**Modify:** `games/cs2/js/components.js`, `games/cs2/js/ui.js`, `packages/cs2/ui.d.ts`, `games/cs2/js/hudkit-prelude.test.js`, `packages/sdk/test/cs2-ui.test.mjs`, `games/cs2/js/components.test.js`.

**Interfaces:** every retained view captures the actual `Client` returned by `Clients.fromSlot(slot)` and a component epoch; validate with its host-backed `isValid()` and existing private identity comparison. Never reproduce identity from SteamID or slot alone. Expose `isValid(): boolean` on retained component views. Legacy void methods no-op when stale; throwing open methods fail; existing `tryOpen` returns failure. New structured adapters arrive in Task 3.

- [x] Extend the real-prelude fixture to reconnect the same slot with the same SteamID but a new client generation. Retain old modal/dashboard/badge/MOTD/kit views, invoke them, and assert zero replacement-client engine writes or capture changes.
- [x] Add delayed hide/fade regressions: show, close, reopen, then fire the first session's timer. Assert the newer component remains visible. Add release/reclaim by another plugin and plugin-reload cases.
- [x] Wrap retained view operations with client/component checks. Keep slot-first APIs adopting the current client. Capture epochs in every timer and next-frame closure; invalidate on forget, release, map/entity replacement, and unload.
- [x] Assert stale action dispatch returns before looking up current domain data. Cleanup must not release the replacement client's capture lease or overwrite its paint cache.
- [x] Run focused UI and existing client-lifetime tests, independently review, and commit as `fix(cs2): bind component handles to client lifetimes`.

## Task 3: Add consistent results and honest readiness

**Worker:** Sol/high; Luna handles compile-time examples after type approval. **Depends on:** Task 2.

**Modify:** `packages/cs2/ui.d.ts`, `games/cs2/js/ui.js`, `games/cs2/js/components.js`, `packages/sdk/test/cs2-ui.test.mjs`, `games/cs2/js/components.test.js`.
**Create:** `docs/UI_API.md` with compatibility and migration examples; `packages/sdk/test/fixtures/ui-api-contract/` using the existing SDK compile-fixture conventions.

**Public contract:**

```ts
type UiErrorCode = "NotReady" | "StaleClient" | "Released" |
  "Unavailable" | "PoolExhausted" | "Busy" | "InvalidArgument" | "PaintFailed";
type UiResult<T> = { readonly ok: true; readonly value: T } |
  { readonly ok: false; readonly error: {
    readonly code: UiErrorCode; readonly message: string } };
type UiStatus = { readonly server: "ready" | "not-ready" | "unavailable";
  readonly clientContent: "unknown"; readonly reason: string | null };
```

Add `status(): UiStatus` on `HudLayout`; `tryShow(...): UiResult<void>` on layout/player and badge views; `tryOpenResult(...): UiResult<View>` on modal/dashboard owners and views; `tryRefresh(slot: number): UiResult<void>` on owners and `tryRefresh(): UiResult<void>` on bound views. Structured owner refresh requires a slot so mixed bulk outcomes cannot be hidden. Add `HudKit.tryModal(spec): UiResult<Modal>` and `tryBadge(spec): UiResult<Badge>` so pool exhaustion is reachable through structured APIs without changing legacy factory timing or null returns. Preserve existing `ModalOpenResult` and `HudResult` exactly. Task 6 uses these result types.

- [x] Write fault-injection cases for no active client, missing binding, exhausted pool, stale view, released component, and failure halfway through painting. Assert code plus no leaked capture/claim.
- [x] Add compile fixtures that accept the new discriminated union and compile existing API examples unchanged; deliberately reject invalid error codes and wrong value/view types.
- [x] Introduce internal structured results at failure sources, adapting outward to legacy strings/throws. Do not parse error text. Route dashboard failures through the same cleanup discipline as modal failures.
- [x] Implement `status()` using existing entity/client/binding facts. Test that active server plus unknown client addon remains `clientContent: "unknown"`. Do not treat a MAM cvar as a render acknowledgement.
- [x] Run focused UI gate, SDK tests, plugin typecheck; document concrete retry behavior and commit as `feat(cs2): add structured UI results and readiness`.

## Task 4: Add opt-in focus ownership across plugins

**Worker:** Astra/high; independent Sol reproduces host tests. **Depends on:** Task 3.

**Create:** `core/src/surface_leases.rs` for generic owner-checked arbitration.
**Modify:** `core/src/lib.rs`, the native-registration module selected after Task 0, `core/src/dispatch.rs`, `core/src/client.rs`, `core/src/entity_live.rs`, `games/cs2/js/ui.js`, `components.js`, `hudinput.js`, `packages/cs2/ui.d.ts`, and corresponding core/UI tests. Reuse existing owner-store/ledger integration and `shared_entity_switch.rs`; do not duplicate capture state.

**Interfaces:** public `UiFocusOptions = { mode: "exclusive"; priority?: number }` added as optional `focus` to high-level open options. Internal host operations are `reserve(surface, entityIdentity, slot, priority) -> token`, `activate(token) -> boolean`, `active(token) -> boolean`, and `release(token) -> boolean`. Reserved tokens cannot dispatch; paint successfully before activation, release on failure. The host derives plugin and client lifetime identity; JS cannot choose an owner. Tokens are opaque, monotonically unique, and owner-checked. `surface` is game-declared data. Invalid priority/identity returns a structured failure through the game adapter. Covered owners release their panel capture lease; restoring them requires successful next-frame reconciliation before interaction resumes. The host ledger never directly invokes a cross-context JS repaint. Freeze outgoing capture release before activating a replacement (including a replacement requesting cursor:false); use game-supplied binding data with the existing generic switch, not CS2 names in Rust.

- [x] Implement pure registry tests for A/B acquisition, priority/tie ordering, releasing covered A, restoring A after B closes, owner spoof rejection, same-slot reconnect, entity replacement, reentrant release, and unload without JS cleanup. Extend `pluginWorld()` in `hudkit-prelude.test.js`: a click that opens/transfers focus to B must not also trigger B during that delivery. Apply the existing dispatch-epoch fencing to newly active tokens.
- [x] Wire host natives/owner stores and ledger disposal. Add ABI registration checks wherever the reconciled host owns them. No CS2 class/panel names belong in this Rust module.
- [x] Integrate modal/dashboard/MOTD focus adapters: successful paint then activation, rollback on failure, suspended covered component state, active-token checks on click, and restoration on close/unload. Keep capture-only calls without focus options compatible.
- [x] Use the existing frame lifecycle to reconcile only active focus participants; prevent restored focus from painting a stale client/entity. Document that raw observers still observe and arbitrary low-level layouts are outside opt-in focus.
- [x] Run core tests, boundary/ABI checks, focused UI gate, and Linux native gate. Commit as `feat(cs2): arbitrate component focus across plugins`.

## Task 5: Coalesce explicit invalidations

**Worker:** Sol/high. **Depends on:** Task 4.

**Modify:** `games/cs2/js/components.js`, `packages/cs2/ui.d.ts`, `games/cs2/js/components.test.js`, `games/cs2/js/hudkit-prelude.test.js`.
**Create:** `docs/benchmarks/2026-09-ui-api/README.md` and a reproducible Node benchmark beside it.

**Interfaces:** add `invalidate(): void` and `lastUpdateResult(): UiResult<void> | null` on modal/dashboard views, and `invalidate(slot?: number): void` on owners. Keep `refresh()` synchronous. Internally use one dirty record per component/client lifetime, one scheduled drain per plugin context, and the Task 1 committed snapshot only after successful paint. Before any update the retained result is null; successful/failed initial open, synchronous refresh/tryRefresh, and deferred repaint all record completion. Pending invalidation retains the previous completed result until drain. A deferred failure logs one diagnostic on entry into failure, rearms that diagnostic only after success, and does not auto-retry.

- [x] Extend the fixture's frame queue with a deterministic drain. Assert 100 invalidations before one frame evaluate rows once; synchronous refresh still updates before return. Verify close/release/reconnect before drain causes zero writes.
- [x] Test invalidation during a provider callback schedules the next frame, never recursively drains; throwing providers cannot starve other dirty views. Assert a deferred paint failure is visible through `lastUpdateResult`, disables interaction, and does not log repeatedly or automatically reschedule. Test focus suspension/restoration and entity replacement.
- [x] Build all page counts/details/footer plans from one provider result per repaint. Eliminate the additional `rowsFor()` call in footer pagination. Remove scheduled records on lifecycle cleanup.
- [x] Benchmark fixed data for 1/8/32 players, 1/2 visible modals, 10/100/1000 rows, and 1/10/100 invalidations. Record provider calls, attempted engine calls, actual writes, median/p95 elapsed time, queue depth, and cleanup-to-zero. Run five alternating baseline/candidate pairs on the same runtime/hardware.
- [x] Report regressions as well as savings. Require one provider evaluation per dirty view/drain and bounded pending records; make no FPS/network-byte claims from stub tests. Run focused UI and SDK gates; commit as `perf(cs2): coalesce explicit component invalidations`.

## Task 6: Owned shared surfaces and disposable typed events

**Worker:** Sol/high; Luna owns examples/types review in a separate worktree. **Depends on:** Tasks 3–5.

**Modify:** `games/cs2/js/ui.js`, `components.js`, `packages/cs2/ui.d.ts`, `core/src/surface_leases.rs`, `games/cs2/js/hudkit-prelude.test.js`, `packages/sdk/test/cs2-ui.test.mjs`, the Task 3 compile fixture, and `docs/UI_API.md`.

**Interfaces:** add `UiSubscription { dispose(): void }` and `HudLayout.subscribeClick(buttonId, handler): UiSubscription`. Infer literal button IDs from generic `CustomHudSpec`/`HudLayout`; dynamic string-array descriptors retain string IDs. Keep the old `onClick` duplicate-handler semantics.

For `HudKitPlayer`, add `tryOwnToast(spec)`, `tryOwnBanner(spec)`, `tryOwnCallout(spec)`, and `tryOwnMotd(spec)`, each returning `UiResult<UiSurfaceHandle>`; add `HudKit.tryOwnDashboard(spec): UiResult<OwnedDashboard>`. `UiSurfaceHandle` exposes `isValid()` and `dispose()`; `OwnedDashboard` implements the existing dashboard operations plus `dispose()`. Claim only on actual presentation when a player is known. Dashboard construction failures are immediate; per-player contention is reported by its `tryOpenResult`. Busy policy is reject; timers/cleanup are owner- and generation-checked. Legacy active users count as occupancy and cannot be overwritten by an explicit claim; legacy calls while explicitly owned fail through their existing error/throw adapter.

Toast ownership uses its existing four physical lanes per player, in lowest-free-lane order; Busy means all four are occupied, not one. Timer identity includes client, lane, and owner token. Fix cross-player timer generation interference along this path. Pooled badges already have host-owned claims and retain that API; no redundant `tryOwnBadge` is introduced.

- [x] Add two-context tests: A owns a surface, B receives `Busy`, A unloads, B succeeds, then A's stale timer/disposer has no effect. Cover an already active legacy surface and a legacy write during explicit ownership.
- [x] Add subscription tests for idempotent dispose, dispose during callback, subscribe during dispatch (first runs on the next event), plugin unload, and unrelated-owner preservation. Freeze the dispatch subscriber list before invoking callbacks.
- [x] Add compile fixtures accepting declared button IDs and rejecting misspelled literals; verify dynamic descriptors and old plugins still build.
- [x] Reuse Task 4's generic registry, record legacy occupancy in the host as well, and ensure singleton timer replacement cannot hide another owner's presentation. Document low-level global-ID collision limits and unchanged raw-observer semantics.
- [x] Preserve fixed markup IDs, lowest-first pooled allocation, intern caps, lazy context-bound hudkit initialization, and load-window hook registration. Install the shared engine hook through the existing lifecycle path; late subscriptions only modify the established route table. Document that per-slot HUD state is not proven confidential against spectator viewing.
- [x] Run focused UI, core ownership tests, SDK/typecheck gates; commit as `feat(cs2): add owned UI surfaces and disposable click subscriptions`.

## Task 7: Integrated acceptance and publication-ready evidence

**Worker:** Sol/high for fixture; root alone drives Nebula; Astra final review. **Depends on:** all prior tasks.

**Create:** `examples/ui-multiplugin-a/`, `examples/ui-multiplugin-b/` (using the existing SDK scaffold conventions), and `docs/superpowers/plans/ui-api-hardening/acceptance.md`.
**Modify:** `docs/UI_API.md`, appropriate package changesets, and gate registration only where new fixtures/tests require it.

- [x] Build two genuinely separate plugin contexts. A shows a keyed list, pooled badge, and owned banner; B shows a competing focus-enabled modal. Add commands to reorder A's provider without repaint, repaint, close/release B, and print owner/click/provider counters. Avoid commands with destructive gameplay effects.
- [x] Run complete JS/native gates on the integrated SHA. Record commands, exit codes, source hashes, benchmark baseline/candidate, and reviewer findings; do not reuse old hardening test totals as evidence for new code.
- [ ] Stage only to the owned `s2script-cs2-hardening` environment using sniper binaries and a rollback backup. Preserve production and coordinate restarts if a human is connected. Carry forward the separate MAM incident; an addon-loading failure blocks client acceptance rather than counting as an API pass.
- [ ] With a human: verify A click identity after data reorder, B receives focus, covered A cannot act, closing B restores A, cursor releases after the last owner closes, and two components do not overwrite each other. Exercise disconnect/reconnect into the same slot, plugin reload, and pooled-panel reclaim. Record visible results alongside command counters.
- [x] Repeat automated open/refresh/release/reload churn for 1,000 cycles and verify owner, subscription, pending-render, and cursor-lease counts return to baseline. Distinguish observed RSS from a proven memory bound.
- [x] Obtain independent spec and code reviews, fix actionable findings, and run only affected checks again. Publish the authorized continuation as draft PRs with compatibility, validation, and remaining human/protocol limitations. Do not merge without a subsequent user instruction.

## Worker handoff template

```text
Implement Task N from docs/superpowers/plans/2026-09-05-ui-api-hardening.md.
Read the linked spec and CLAUDE.md. Work only in your assigned worktree and files.
Your baseline SHA and branch are supplied with this assignment. Do not rebase,
push, merge, alter other agents' files, or use the live server.
Start with the named behavior regression, confirm its failure, implement the
contract, and run the task's focused checks. Commit only this task's files.
Return: commit SHA, public contract changes, tests/commands and outcomes,
performance evidence where relevant, limitations, and any unresolved finding.
```

## Completion checklist

- [x] Both baseline histories preserved and verified.
- [ ] Six improvement areas covered by Tasks 1–6; real two-plugin acceptance covered locally by Task 7. The combined live-acceptance item remains unchecked until owned-server staging and the human protocol are complete.
- [x] Compatibility adapters, docs, and changesets accompany every public API addition.
- [x] No claim of client acknowledgement, global click suppression, or human acceptance from a server-only test.
- [x] Subagent reports identify exact commits and evidence; root has no unreviewed integration delta.
