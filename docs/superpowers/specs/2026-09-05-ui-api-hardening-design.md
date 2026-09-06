# UI API correctness, ownership, and update design

Status: implementation authorized as a continuation of the existing PR stack on September 5, 2026.

## Goal and baseline

Make multiple plugins' interactive UI predictable across data changes, competing input, reconnects, and reloads, while reducing redundant repaint work. Extend the existing `CustomHudLayout` and `hudkit` APIs instead of replacing them with a new framework.

The inspected upstream baseline is `9a36af4834a19e94d0485cdaa6ece543d6fbc8ab`. It contains shared panel allocation (#175), entity-bound paint caches (#176), per-player footer actions (#177), and shared cursor ownership (#182). The published hardening stack ends at `481d196631404e0d0a874d0a1136dd7c1628b6d3` and contributes connection-lifetime protection. These histories must be reconciled and tested before implementing this design. Neither branch alone is the intended baseline.

## Constraints

- Core is engine-generic; CS2 layout names, input policy, and rendering semantics remain in the game package.
- Cross-plugin state belongs in the host, never a plugin context's `globalThis`.
- The ledger is the teardown authority; cleanup cannot depend on plugin callbacks running.
- No raw pointer, engine event view, or cross-plugin JS object may survive a callback.
- Existing public methods keep their return types, timing, and documented disabled-row behavior. New semantics use additive methods/options; incompatible changes require a separately declared major version.
- No new runtime dependency or workshop markup change is required for the initial implementation.
- No production deployment, merge, or automatic restart of `s2script-hudlab` is part of this plan.
- Offline tests and bots do not prove human rendering, click delivery, focus, or spectator privacy.

## Decisions

### 1. Click identity

Capture a value snapshot of each modal/dashboard's successfully submitted page: row values, absolute indices, stable IDs where supplied, tab identity, and footer actions. Dispatch clicks against that snapshot, never a newly reordered row provider. Add optional `Row.id: string`; dashboard rows already carry IDs. Reject duplicate supplied IDs within a rendered collection. Copy row fields so mutation of the source objects cannot silently change the action mapping.

Domain actions still revalidate current permissions, entity/client identity, and item availability by stable ID immediately before acting. The API cannot make a deleted player or expired purchase valid. Legacy index callbacks receive the index from the submitted snapshot, and documentation tells authors not to use it to index a newly fetched collection.

A failed or partial paint invalidates interactive dispatch for that view until a complete successful repaint. It must not commit a new action table over partially updated visuals. Successful engine submission is not client acknowledgement: the engine click payload currently exposes a player and global button ID, without a render revision. This design cannot distinguish an old network-delayed click after the same panel is repainted/reused. End-to-end render revision transport is a separate protocol project if live testing requires it; do not claim this limitation is solved by a server-only counter.

### 2. Component lifetimes

All retained high-level views capture the same host-validated client identity as `HudPlayer`, plus component lifetime identity. Modal, dashboard, badge, MOTD, and `HudKitPlayer` operations must fail or no-op consistently when that identity is stale. Slot-first APIs deliberately adopt the current occupant. Timer/next-frame callbacks capture both client identity and component generation, preventing an old fade/close from changing a reopened component.

### 3. Results and readiness

Add `UiResult<T>` with stable error codes and a human message. Preserve existing `HudResult`, throwing `open`, and `ModalOpenResult` as compatibility adapters. Add structured `tryShow`, `tryOpenResult`, and `tryRefresh` methods where needed. Classify failures at their source; never derive error codes by parsing English messages.

Add structured `tryModal`/`tryBadge` factories for pool exhaustion; legacy factories still eagerly claim and return null on exhaustion. Structured owner refresh requires one slot, while legacy bulk refresh remains available.

Expose server readiness separately from client content availability. `status()` can report world/entity/binding readiness; it cannot claim an addon rendered on a client. Do not add unbounded automatic retries or pending opens. A caller can explicitly retry a named transient failure.

### 4. Focus and shared surfaces

Shared cursor capture answers whether input capture remains enabled. Focus additionally determines which participating interactive component may act. Add opt-in focus options to high-level opens: `{ mode: "exclusive", priority?: number }`. Higher priority wins; latest successful acquisition wins ties. Default priority is zero. Closing, releasing, disconnecting, or unloading the winner restores the next live owner. Existing calls without this option retain their current behavior.

Use one engine-generic host registry for owner-checked, connection-bound surface leases and their active token. Modal/dashboard/MOTD code translates those leases into CS2 presentation and capture. Covered exclusive components are hidden/suspended without destroying their state and must not dispatch clicks. Focus acquisition and paint failure must roll back without stranding capture. Raw click observation remains observer-only; no promise to suppress map handlers or arbitrary custom layouts.

Ownership transfer is fenced by the current dispatch epoch: a click that opens or transfers focus to another component cannot also activate that component during the same delivery. Focus priority does not imply engine z-order control; hide suspended roots and restore them through the existing presentation layer.

Use two-phase reservation/activation: a reserved token cannot dispatch, successful paint permits activation, and failure releases the reservation. Covered components release their panel capture leases before the replacement activates, including replacements with `cursor:false`. Restoration reconciles through the game package on a later frame and remains noninteractive until painting succeeds; host teardown never invokes arbitrary JS repaint callbacks.

For singleton banner, toast, callout, dashboard, and MOTD surfaces, offer an explicit owned API. Initial policy is `reject` when busy, with structured `Busy` and idempotent disposal; no unbounded queue. Existing last-writer behavior remains available for compatibility and is documented as uncoordinated. An owned API rejects acquisition if an active legacy user occupies that surface, and legacy calls must not evict an explicit lease. Arbitrary custom markup/global button-ID collisions remain a documented low-level responsibility.

When an explicitly owned surface rejects a legacy call, preserve that method's established error convention: return a reason where it already returns `HudResult`, fail where it already throws on open, and no-op where it returns void. The additive structured counterpart reports `Busy`. No legacy void call becomes throwing as a side effect of this work.

Toast is a four-lane pool per player, not a single surface: ownership preserves all four lanes and lowest-free allocation, with timer generations scoped to client/lane/token. Badges already use owned pool claims; reuse them rather than adding another ownership API.

### 5. Updates

Keep synchronous `refresh()` behavior. Add `invalidate()` to modal/dashboard views and owners, coalescing repeated requests to at most one pending repaint per component/client. Drain at a documented next-frame boundary, using the existing lifecycle-managed scheduling mechanism. Evaluate a dynamic row source once per repaint and derive page count, details, and footer planning from that snapshot. A repaint requested during repaint belongs to the next frame, preventing recursive drains. Close, release, unload, reconnect, and entity replacement discard stale work. There is at most one queued record per live dirty component/client pair; queue depth must return to zero after cleanup.

Deferred errors must be observable. Retain the last completed `UiResult<void>` on each live view via `lastUpdateResult()`; it is null before any update, and a failed deferred update also emits one diagnostic on transition into failure. There is no automatic retry loop. A failed update invalidates interaction as specified above.

Initial open, synchronous refresh, and deferred repaint all record completed results. Pending invalidation does not overwrite the previous result. Repeated failures emit no new diagnostic until a success rearms reporting.

### 6. Events and authoring

Add `subscribeClick(buttonId, handler)` returning an idempotent disposable subscription. Existing `onClick` stays a compatibility API. Parameterize layout types so literal `buttons` declarations constrain typed subscriptions while dynamic descriptors retain a string fallback. Removing a subscription never removes another owner's handler; dispatch uses a stable subscriber snapshot for reentrant unsubscribe/register operations. The host hook stays lifecycle-bound. A raw global observer is not an exclusive component owner.

## Alternatives considered

- A new declarative/React-style renderer would multiply migration, runtime, and workshop requirements. It is outside this scope.
- Changing all existing methods to structured results and deferred refresh would simplify the final API but break consumers. Additive methods preserve compatibility and allow migration with evidence.
- Implementing focus only in JS would be smaller, but plugin contexts cannot arbitrate each other's state. Host ownership is required.

## Acceptance

Deterministic tests must cover row mutation/reorder, failed paint, stale views, competing plugin contexts, failed focus acquisition, lifecycle cleanup, update coalescing, and compile-time API compatibility. The final test server must run two independent UI plugins and prove focus restoration and click ownership with a human client. Reconnect, plugin reload, and release/reclaim checks are mandatory. Record exact source/binary/addon hashes and keep the existing MultiAddonManager startup/map crash open until reproduced and fixed independently.
