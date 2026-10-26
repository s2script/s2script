---
"@s2script/cs2": minor
---

`pawn.eyePosition` in `@s2script/cs2`: where the player's camera sits, the origin plus the networked
view offset (SourceMod's `GetClientEyePosition`). It follows crouching. `pawn.aimTrace` now starts
from it instead of a fixed 64 units above the origin.
