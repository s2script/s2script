# CS2 UI API

This page describes the additive UI reliability APIs planned for `@s2script/cs2`.
The structured methods make failure categories machine-readable while existing methods
keep their current return values, timing, and throwing behavior. The Task 3 integration
worker must verify the implementation against this contract before it is considered
available.

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
  tryRefresh(slot: number): UiResult<void>;
}

interface HudPlayer {
  tryShow(panelId: string, opts?: { cursor?: boolean }): UiResult<void>;
  tryRefresh(): UiResult<void>;
}

interface Modal {
  tryOpenResult(slot: number, opts?: { cursor?: boolean }): UiResult<ModalView>;
  tryRefresh(slot: number): UiResult<void>;
}

interface ModalView {
  tryOpenResult(opts?: { cursor?: boolean }): UiResult<ModalView>;
  tryRefresh(): UiResult<void>;
}

interface Dashboard {
  tryOpenResult(slot: number, opts?: { tab?: string; cursor?: boolean }): UiResult<DashboardView>;
  tryRefresh(slot: number): UiResult<void>;
}

interface DashboardView {
  tryOpenResult(opts?: { tab?: string; cursor?: boolean }): UiResult<DashboardView>;
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
- Legacy `show()` keeps its `HudResult` return. Use `tryShow()` for code that branches
  on a stable error code.
- Existing disabled-row behavior is unchanged: `disabled` is cosmetic and `onPick`
  still fires. Authors decide whether an unavailable action should be explained or
  rejected by their domain logic.

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

Delayed callbacks associated with an old view (including timers and frame work) are
invalidated when the component is forgotten, released, reconnected, or replaced. This
prevents a previous session's close/fade or repaint from changing a reopened component
or writing to a replacement client. Treat retained handles as disposable references and
check their result before continuing an action.

