# Observable HUD modal open failures

## Why

Modal open currently publishes `isOpen` before painting and ignores HUD drive errors. A missing layout entity, unresolved descriptor, or rejected engine invocation can therefore return a successful-looking view while displaying nothing. Menu presentation must fall back to chat in those cases.

## Contract

- Add `ModalOpenResult`: `{ ok: true, view: ModalView } | { ok: false, error: string }`.
- Add `tryOpen` to both Modal and its bound ModalView. Paint candidate state and show the root before publishing logical open state.
- Preserve successful `open` callers: return the same bound-view shape. Throw a descriptive error on failure.
- Failed `tryOpen` leaves the modal logically closed. Hiding partially painted state is best effort: if the engine also rejects cleanup, the API cannot guarantee that already-applied engine state was removed.
- Cache HUD class/text writes only after successful invocation, so a later retry sends previously rejected values.
- When a Menu HUD open fails, use the existing chat renderer and release the claim if no other HUD viewer uses it. Do not arm input on the failed path.

## Native boundary

No new engine powers or Rust native are needed. Existing declared void calls return `undefined` on success and `null` on rejection. The game HUD wrapper converts rejection to a named error; a resolved descriptor's `available` status does not override an invocation failure. Shared cursor acquisition and cleanup are owned by the preceding capture slice.

## Validation

Exercise the shipped UI/component/menu preludes with a simulated engine: world not ready, unresolved descriptor, native invocation rejection, closed state and no capture after failed paint, retry without poisoned caches, successful legacy return shape, and menu fallback with idle and shared claims. Run the full JS gate; record unavailable environment gates without implying a live-server proof.
