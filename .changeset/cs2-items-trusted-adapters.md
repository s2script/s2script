---
"@s2script/cs2": minor
---

Remove the `ctx.items` plugin-context augmentation. Pickup gates are the `items` module export
(`import { items } from "@s2script/cs2"`) with an unchanged `onCanAcquire` / `onCanAcquirePost`
view; the interface is now `ItemsApi`, and `CtxItems` remains as a deprecated type alias.

Pickup gates and custom-HUD clicks now run on the engine-function service through the CS2
package's `legacy.acquire.v1` and `legacy.hud-click.v1` adapters instead of legacy gamedata hooks.
The vote fold, POST observation, HUD click timing and button-id copy are unchanged. The
`onCanAcquire` and `onCustomHudClicked` hook descriptors are gone from the CS2 gamedata; their
signatures are bound by name, so `gamedata/cs2/custom/` signature repairs still apply. Requires the
matching runtime.
