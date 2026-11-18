---
"@s2script/cs2": minor
---

The `GameHud.weapon` card marks the item in hand inside a shared slot row and shows weapon icons. Each row now holds up to four items (the knife and the Zeus, the grenades), drawn as the game's own equipment icon with the one in hand at full strength and the rest dimmed; a row past four ends in "+N". `auto` marks the item by its entity, orders knife before Zeus and grenades in buy order, shows the active weapon's icon beside its name, and takes `options.icon(defIndex, weapon)` to swap an icon. In `set`, a row is still a string, or `{ items, active }` to mark one item, and an item can be `{ text, icon }`; `spec.icon` sets the header icon. The layout's slot ids changed (`gk_wpn_sN_t` is now `gk_wpn_sN_i0..i3`), so the game-kit workshop addon must be republished together with the runtime.
