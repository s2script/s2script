# CS2 UI API

This page describes the additive UI reliability APIs in `@s2script/cs2`.
The structured methods make failure categories machine-readable while existing methods
keep their current return values, timing, and throwing behavior.

## Structured results

```ts
type UiErrorCode = "NotReady" | "StaleClient" | "Released" |
  "Unavailable" | "PoolExhausted" | "Busy" | "InvalidArgument" | "PaintFailed";

type UiResult<T> = { readonly ok: true; readonly value: T } |
  { readonly ok: false; readonly error: {
    readonly code: UiErrorCode; readonly message: string } };
```

Check `ok` before using `value` or `error`:

```ts
const result = hudkit.tryModal(spec);
if (!result.ok) {
  if (result.error.code === "PoolExhausted") {
    // Release a modal you no longer need, or retry after a later lifecycle event.
    return;
  }
  return;
}
const modal = result.value;
```

Error codes are stable classifications. The human-readable `message` is for logs and
diagnostics; callers should branch on `error.code`, never parse the message. A failed
operation does not leave a capture claim or a partially committed action table behind.
Retry only when the surrounding condition has changed (for example, after the HUD is
ready, a client reconnects, or a pooled handle is released). These methods do not queue
an unbounded pending open or silently retry forever.

## Readiness

`HudLayout.status()` reports server-side facts separately from what is known about a
client's addon content:

```ts
type UiStatus = {
  readonly server: "ready" | "not-ready" | "unavailable";
  readonly clientContent: "unknown";
  readonly reason: string | null;
};
```

`server: "ready"` means the world entity and its server binding are available for
drives. It does not mean that a particular client mounted the workshop addon or
rendered the latest paint. `clientContent` is intentionally always `"unknown"` in
this API. A MAM cvar, successful engine submission, or server-side counter is not a
render acknowledgement. If the status is not ready, wait for the normal client/map
lifecycle and explicitly retry the operation.

## Additive factories and operation methods

The following methods are structured counterparts to existing APIs:

```ts
interface HudLayout {
  status(): UiStatus;
  tryShow(slot: number, panelId: string, opts?: { cursor?: boolean }): UiResult<void>;
}

interface HudPlayer {
  tryShow(panelId: string, opts?: { cursor?: boolean }): UiResult<void>;
}

interface Modal {
  tryOpenResult(slot: number, opts?: { cursor?: boolean; focus?: UiFocusOptions }): UiResult<ModalView>;
  tryRefresh(slot: number): UiResult<void>;
}

interface ModalView {
  tryOpenResult(opts?: { cursor?: boolean; focus?: UiFocusOptions }): UiResult<ModalView>;
  tryRefresh(): UiResult<void>;
}

interface Dashboard {
  tryOpenResult(slot: number, opts?: { tab?: string; cursor?: boolean; focus?: UiFocusOptions }): UiResult<DashboardView>;
  tryRefresh(slot: number): UiResult<void>;
}

interface DashboardView {
  tryOpenResult(opts?: { tab?: string; cursor?: boolean; focus?: UiFocusOptions }): UiResult<DashboardView>;
  tryRefresh(): UiResult<void>;
}

interface BadgeView {
  tryShow(data?: { title?: string; text?: string }): UiResult<void>;
}

interface HudKit {
  tryModal(spec: ModalSpec): UiResult<Modal>;
  tryBadge(spec?: BadgeSpec): UiResult<Badge>;
}
```

Owner refresh methods require a `slot`, so a mixed bulk refresh cannot hide which
player failed. Bound views already identify their player and therefore use
`tryRefresh()` with no argument. `tryShow` is available on layout/player and badge
views. `tryOpenResult` is available on modal/dashboard owners and their views.

`HudKit.tryModal` and `HudKit.tryBadge` expose pool exhaustion as `PoolExhausted`.
The legacy `modal()` and `badge()` factories retain eager claim timing and return
`null` when their pool is exhausted.

## Compatibility

Existing APIs remain valid and retain their established conventions:

- `HudResult` remains `string | null`; `null` still means the drive succeeded.
- `ModalOpenResult` remains its existing `{ ok: true, view } | { ok: false, error: string }`
  shape. `tryOpen()` keeps its existing name and behavior.
- `open()` keeps its synchronous return and throwing behavior on failure.
- Legacy `refresh()` remains synchronous and keeps its existing bulk-owner or bound-view
  shape. Use `tryRefresh()` when the caller needs a structured result.
- Low-level layout/player `show()` keeps its `HudResult` return. `Badge.show()` keeps
  returning a `BadgeView`, and `BadgeView.show()` remains void. Use `tryShow()` where
  the corresponding structured result is available and the caller needs a stable
  error code.
- Modal disabled rows remain cosmetic: `onPick` still fires, so authors decide whether
  an unavailable action should be explained or rejected by their domain logic.
  Dashboard disabled rows retain their existing disabled-action rejection behavior.

The additive APIs are intended for incremental migration. A plugin can continue using
legacy calls while adopting structured factories and operation results at boundaries
where it needs reliable retry or telemetry.

## Rows, snapshots, and action validity

Interactive modal and dashboard paints commit one page snapshot only after the complete
paint succeeds. A click is resolved against the successfully submitted snapshot, so a
provider reorder or in-place mutation after paint cannot make the callback receive a
different row. The snapshot includes declared row fields, absolute index, tab identity,
and footer actions. The `Row` type's optional stable identifier is:

```ts
interface Row {
  readonly id?: string;
}
```

Supply unique IDs within each rendered collection when the domain has them. Duplicate
supplied IDs are rejected for that collection. IDs make it possible to revalidate the
current domain immediately before an action; they do not authorize an action by
themselves. Recheck permissions, player/entity identity, and item availability by ID
before changing state. An absolute index from `onPick` refers to the submitted
snapshot, so do not use it to index a freshly fetched or reordered collection.

If painting fails or stops partway through, interaction is disabled until a complete
successful repaint. The prior data may be retained for recovery, but it is not left
clickable over partially changed visuals. Successful server submission still is not a
client render acknowledgement: the current click payload has no render revision, so
this API cannot identify an old delayed click after a panel is repainted and reused.

## Retained views and client generations

Modal, dashboard, badge, MOTD, and other retained component views are bound to the
client generation that created them. A reconnect in the same slot creates a new client
generation; the old view does not become a handle to the replacement client. Operations
on a stale or released view return `StaleClient` or `Released` through structured
methods, while legacy void methods keep their no-op compatibility behavior and legacy
throwing opens keep their failure behavior. Slot-first owner methods deliberately adopt
the current occupant.

An ordinary close/reopen of the same component for the same client remains reusable.
Presentation epochs invalidate framework-owned timers and frame jobs from an earlier
presentation, preventing an old close/fade or repaint callback from changing the
reopened component. Forget, release, reconnect, and replacement also invalidate the
component's retained work. User-managed asynchronous code remains the author's
responsibility. Treat retained handles as disposable references and check their result
before continuing an action.


## Exclusive component focus

Modal and dashboard opens accept `focus`, and `MotdSpec` accepts the same option:

```ts
import { hudkit, type UiFocusOptions } from "@s2script/cs2";

const focus: UiFocusOptions = { mode: "exclusive", priority: 10 };
const modal = hudkit.modal({ title: "Confirm", rows: [] });
const result = modal?.tryOpenResult(slot, { focus, cursor: true });
hudkit.motd(slot, { title: "Rules", focus });
```

All three components participate in one host-owned focus stack per layout entity and
client connection, including across plugins. Higher priority wins. The latest successful
reservation wins equal-priority ties. Priority defaults to `0` and must be an integer in
the inclusive signed-int32 range `-2147483648` through `2147483647`; fractions, nonfinite
numbers, strings, and out-of-range values fail with `InvalidArgument` before painting.
Reopening and explicitly replacing a dashboard spec make fresh reservations. Ordinary
refresh and restoration retain their reservation order.

Reservation happens before providers, engine writes, or capture. A covered open succeeds
as a logical open and retains desired navigation and cursor state, but does not evaluate
providers, paint, capture input, or dispatch component actions. Taking focus hides the
outgoing root and releases that panel's capture before the incoming component paints,
even when the incoming open requests `cursor: false`. Independent manual capture holders
remain independent.

Closing, forgetting, releasing, disconnecting, or unloading a winner retires its exact
reservation. A surviving contender waits for a later host frame, then clears its paint
cache, evaluates fresh providers, completely repaints, restores its desired capture, and
activates. Restoration also clears the cache when the component never observed that it
was covered. Focus does not promise engine z-order control or client rendering
acknowledgement; `clientContent` remains `"unknown"`.

Component actions check host focus immediately before dispatch. A newly activated
component cannot consume the input delivery that opened it, including nested delivery.
A failed initial paint leaves no open focused presentation; a failed refresh or
restoration disables actions and releases focus. Desired state may remain available for
an explicit `refresh()`/`tryRefresh()` retry. Failure does not automatically retry each
frame. Modal `setCursor` remembers changes while covered and applies them only when the
component owns focus again.

Use modal/dashboard `tryOpenResult` to inspect reservation errors, including `Busy`,
without parsing text. MOTD retains its existing return shape: a failed focused open logs
a diagnostic and returns an invalid no-op `MotdHandle`. It does not expose a new structured
MOTD-open method.

Calls without `focus` retain their existing behavior. Raw `CustomHudLayout.onClicked`
observers still receive events, and raw `HudInput` state/arming remains unchanged. Focus
is not global click suppression: legacy menus, low-level layouts, manual drives, and
broad legacy helpers such as `hideAll` remain outside this opt-in arbitration. Cooperating
plugins must opt in and use their component handles for focused presentation cleanup.
