---
"@s2script/cs2": patch
---

`GameHud.weapon.auto` reads each player once every 4 frames (about 16 Hz) instead of every frame, spread across the frames by slot, and rebuilds the slot list only when the inventory or the item in hand changes. A full server spends about a sixth of the time it did on the card and makes about a third of the garbage. The API is unchanged; an ammo count or a switch can show up to 3 frames later than before.
