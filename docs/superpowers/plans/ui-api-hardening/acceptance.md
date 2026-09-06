# UI API hardening acceptance evidence

Status: local Task 7 acceptance preparation restacked onto Task 6 dashboard checkpoint
`b9986daa6c4ed7a8efc9a2fbe3a5e680b8da73e7`, which remains under independent review. The evidence
below was collected at code/test checkpoint `1641daceeba0159daca2732222208b372542f98f`; the root
integrator still owns the final whole-stack gates and live protocol.

## Evidence boundaries

The local JavaScript churn runs the real `games/cs2/js/ui.js` and
`games/cs2/js/components.js` in separate Node VM contexts. Engine calls and the native ownership
boundary are deterministic host stubs. Internal JavaScript queue and subscription counts are
therefore direct runtime observations; pool-owner, focus, owned-surface, and capture counts in this
test are host-stub observations. They are not native-host evidence or client render
acknowledgements.

Native ownership/capture churn is separate preparation from commit
`e6cb17485f1ad42aaaf40e3796da5565bee4543d`. Its real host fixture previously passed the focused
1,000-cycle test (1/1) and the complete `surface_leases::tests` module (34/34). Those native tests
were not rerun in this worktree, as requested. The final integrated native gate remains pending.

No new UI artifact has been deployed to a server. No human has joined for this continuation. Live
rendering, click delivery, focus coverage/restoration, cursor release, reconnect into the same
slot, reload, pool reclaim, and spectator behavior are pending. Earlier runtime soak results are
historical evidence for their recorded runtime and are not new UI acceptance evidence.

## Local commands and results

Run from the repository root unless a command changes directory:

```sh
npm ci
(cd packages/sdk && npm run build)
node packages/sdk/dist/cli.js build examples/ui-multiplugin-a --packages-dir packages
node packages/sdk/dist/cli.js build examples/ui-multiplugin-b --packages-dir packages
node --test --test-name-pattern='1000 VM reload cycles' games/cs2/js/hudkit-prelude.test.js
bash scripts/check-components-test.sh
node --test packages/sdk/test/cs2-ui.test.mjs
bash scripts/check-core-js-lint.sh
bash scripts/check-plugins-typecheck.sh
```

Each command exited 0 on this worktree. The focused churn command passed 1/1;
`scripts/check-components-test.sh` passed 203/203 across the component, input, menu, vote,
prelude, and low-level UI suites. The SDK UI contract suite passed 38/38, the core/game JavaScript
lint gate passed, and `scripts/check-plugins-typecheck.sh` passed every plugin and example,
including both fixtures.
The two fixture builds emitted:

```text
examples/ui-multiplugin-a/dist/_demo_ui-multiplugin-a.s2sp
examples/ui-multiplugin-b/dist/_demo_ui-multiplugin-b.s2sp
```

Artifact metadata from the fresh fixture build:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `_demo_ui-multiplugin-a.s2sp` | 3,775 | `05e257338bac0ce66ed580da8ec8116adb39c7cbb40c09781adca5db48ab228f` |
| `_demo_ui-multiplugin-b.s2sp` | 2,395 | `32813730b14a32517be62fe3795e6215379ab0e841d5737365e0608d22a7cbbb` |

The repository's existing `examples/*/` wildcard registers both fixtures with
`scripts/check-plugins-typecheck.sh`; no additional gate list or workspace-lock entry is needed.
The pre-restack 177/193 provisional result has been superseded: Task 6's final dashboard source and
fixture corrections are present here, and the complete gate now passes 203/203.

## Automated 1,000-cycle VM churn

The regression `1000 VM reload cycles drain real component work and disposable click routes`
performs exactly 1,000 cycles. Each cycle creates a fresh plugin VM after unloading the previous
one and exercises:

- modal pool acquisition, focus-aware open, synchronous refresh, deferred invalidation/drain, a
  second queued invalidation, click delivery, and explicit release;
- an explicitly owned banner and idempotent disposal;
- a disposable click subscription and its actual closure-owned route storage;
- same-slot client-generation replacement followed by stale view calls;
- plugin unload/reload followed by calls through the old view, banner, and subscription handles.

The internal JavaScript measurements return to their exact baseline on every cycle:

| Measurement | Active/queued | After disposal/release | Fresh VM after reload |
| --- | ---: | ---: | ---: |
| Actual component dirty queue | 1 | 0 | 0 |
| Actual subscription route keys | 1 | 0 | 0 |
| Actual retained subscription entries | 1 | 0 | 0 |

`HudLayout._subscriptionStats()` is an internal, read-only test seam omitted from the TypeScript
contract. It exposes only route and entry counts from the real closure-owned `subscribers` table.
The stable-dispatch regression separately proves that disposal during an event updates retained
storage for the next event without changing that event's frozen callback snapshot.

The VM harness also records its simulated host boundary separately:

| Host-stub measurement | Active modal + banner | After disposal/release |
| --- | ---: | ---: |
| Panel pool claims | 1 | 0 |
| Focus leases | 1 | 0 |
| Owned-surface leases | 1 | 0 |
| Capture routes | 1 | 0 |
| Capture holders | 1 | 0 |

Totals are 4,000 provider evaluations, 1,000 modal actions, and 1,000 subscription actions. The
four provider evaluations per cycle are initial open, synchronous refresh, deferred repaint, and
the selected-row detail repaint. After client-generation replacement and again after plugin
unload, stale handles cause zero additional provider evaluations, actions, or engine writes. A
final aggregate assertion checks all 1,001 retained VM contexts (the initial context plus 1,000
reload contexts) and finds zero subscription routes, subscription entries, and dirty records in
every context; all host-stub measurements are also zero.

## Actual fixture command runbook

The fixtures expose only the commands listed here. There is no `sm_ui_churn`, `sm_ui_metrics`,
`sm_ui_reload`, reconnect helper, or synthetic click command. Reload and reconnect use the normal
operator/server and real-client workflows.

Commands accept an optional zero-based active client slot unless shown without one. When the slot
is omitted, an in-game caller's slot is used.

Fixture A (`@demo/ui-multiplugin-a`, focus priority 10):

| Command | Actual behavior |
| --- | --- |
| `sm_ui_a_open [slot]` | Opens the keyed modal, shows the pooled badge, and acquires the owned banner. |
| `sm_ui_a_reorder` | Rotates and mutates source records without provider evaluation or repaint. |
| `sm_ui_a_refresh [slot]` | Repaints synchronously through `tryRefresh`. |
| `sm_ui_a_invalidate [slot]` | Queues a coalesced repaint and reports the immediate provider-call delta. |
| `sm_ui_a_banner_busy [slot]` | Attempts a second owned banner; the live primary handle should make this `Busy`. |
| `sm_ui_a_close [slot]` | Closes the modal, hides the badge, and disposes the banner handle for the slot. |
| `sm_ui_a_release` | Closes all views, disposes all banners, and releases modal and badge pool claims. |
| `sm_ui_a_status [slot]` | Prints source/provider/click/claim/view/handle state as JSON. |

Fixture B (`@demo/ui-multiplugin-b`, focus priority 20):

| Command | Actual behavior |
| --- | --- |
| `sm_ui_b_open [slot]` | Opens B's higher-priority exclusive modal. |
| `sm_ui_b_refresh [slot]` | Repaints synchronously through `tryRefresh`. |
| `sm_ui_b_invalidate [slot]` | Queues a coalesced repaint and reports the immediate provider-call delta. |
| `sm_ui_b_close [slot]` | Closes B while retaining its modal pool claim, allowing A to restore later. |
| `sm_ui_b_release` | Closes every B view and releases B's modal pool claim. |
| `sm_ui_b_status [slot]` | Prints provider/click/claim/view/last-update state as JSON. |

For a human slot `S`, the pending live sequence is:

1. Run `sm_ui_a_open S`, record `sm_ui_a_status S`, then run `sm_ui_a_reorder`. Confirm the visible
   order does not repaint. Click the first visible A row and record `sm_ui_a_status S`; its last
   click must retain the painted stable ID while reporting the changed source ID at that index.
2. Run `sm_ui_a_refresh S`; confirm the visible rows now match the new source snapshot. Run
   `sm_ui_a_invalidate S`, wait one frame, and record `sm_ui_a_status S`.
3. With A open, run `sm_ui_b_open S`. Confirm B is visible and actionable while covered A cannot
   act. Record both status commands. Run `sm_ui_b_close S`, wait one frame, confirm A restores and
   can perform exactly one action, then record both statuses again.
4. Run `sm_ui_a_banner_busy S` and require `Busy`. Run `sm_ui_a_close S` and
   `sm_ui_b_release`; confirm capture has ended and ordinary mouse/game controls work.
5. Reopen A, unload/reload fixture A through the operator's normal plugin workflow, and let B
   acquire the surface. Confirm old A state produces no visible write or action. Repeat with B
   owning focus, then unload/reload B and confirm A restores.
6. Disconnect and reconnect the same account until `status` shows the same slot `S`, the same Steam
   ID, and a different user ID. Old views must remain inert; fresh `sm_ui_a_open S` and
   `sm_ui_b_open S` must work. Record `status`, `meta list`, and both fixture status commands.
7. Finish with `sm_ui_a_release` and `sm_ui_b_release`. Record both status commands and verify the
   final human-visible cursor/control state.

## Pending final evidence

The post-restack Task 7 checkpoint still needs the root-owned full JS and Linux/native gates, sniper artifact
build and hashes, independent final review, owned-server deployment with rollback, and the human
protocol above. The separate MultiAddonManager startup/map crash remains an independent incident;
failure to load the fixtures blocks live acceptance and does not count as a UI API pass or failure.
Publication and PR navigation remain root-owned, and no merge is authorized by this document.
