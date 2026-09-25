---
"@s2script/sdk": patch
---

`SDKHook` `OnTakeDamage` / `OnTakeDamagePost` now run on the selected game package's damage
function (CS2: the trusted `takeDamageOld` binding through the `legacy.damage.v1` adapter) instead
of a core damage detour. The `SDKHook` signature and the `DamageInfo` fields are unchanged; the
view is now borrowed and throws "expired borrowed view" when used after the synchronous callback
(including after an `await`), and a non-finite `damage` write is refused. `Handled`/`Stop` still
block by zeroing damage after every handler ran, and the engine function always runs. Without a
game package that provides damage, `SDKHook` for these types returns `false`. The damage signature
moved from core gamedata to the CS2 package gamedata, so a repair belongs in `gamedata/cs2/custom/`.
Requires the matching runtime.
