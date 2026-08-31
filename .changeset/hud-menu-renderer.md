---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Paint CS2 menus as hudkit center sheets.

`Menu.activation` (`immediate` | `tab`, default immediate) is generic. On CS2 both `MenuStyle.Center` and `MenuStyle.Chat` use one host-lifetime hudkit modal; Tab intercept lives on `HudInput`. Exhausted modal pool keeps the existing Chat renderer.
