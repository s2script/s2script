# Shared HUD capture ownership

## Why

All plugin contexts drive the same layout entity, but ui.js previously counted cursor leases inside each context. Closing one plugin's modal could disable another plugin's capture. Modal cursor toggles also used the manual wildcard lease instead of the root lease acquired by show. JavaScript cleanup could not guarantee capture release on unload.

## Contract

Capture is a host-owned boolean switch per registered game descriptor, entity index and host identity, and player slot. Holders are plugin owner plus token. The first holder enables capture; only the last holder disables it. Panel tokens and the manual cursor token are distinct. Ordinary forget removes only the caller's holders; host disconnect clears all holders before callback delivery. Map reset and entity invalidation drop dead identities without dereferencing them.

The internal native accepts only game-registered entity bindings with no receiver-via hop and void(int,bool) shape. Core knows no game class, descriptor name, or JavaScript callback. Invocation resolves identity through entity_live and preserves the existing hook bypass contract. No registry borrow crosses an engine call. Same-key nested transitions return a retryable named error.

Failed enable records no holder. Failed final disable records an ownerless pending release, retried by frame drain. A new holder cancels pending release without toggling the still-enabled engine state. Retry snapshots recheck holder emptiness. Normal errors return a named string; successful operations return null. Missing native support has no context-local or raw-engine fallback.

Owner-store sweep performs cleanup without JavaScript. A scoped dispatch deferral guard returns the existing deferred sentinel for subscribed notifications; the shim replays against current subscription books after cleanup. Synchronous pre-hooks retain their existing skip policy. Lifecycle changes during an outbound transition supersede its pending claim.

Game presenters release their own root tokens. Modal/MOTD/dashboard close no longer clears the manual token; vote rail uses its own root. Show rolls back visibility on failed acquire. Hide attempts release even if painting fails.

## Validation and limits

Use actual V8 plugin contexts to prove shared ownership, unload, failed enable/disable recovery, pending-release reacquisition, disconnect recycling, dead entity/map reset, binding rejection, and nested/deferred delivery. Game integration tests prove root/manual independence and missing-host failure. The engine boundary is mocked: this slice does not assert live-client rendering or change a server.
