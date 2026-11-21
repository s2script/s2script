---
"@s2script/cs2": minor
---

`GameHud.rail()` now covers CS2's stock team counter and round timer by default: a solid bar across the full width of the top edge, with the score blocks, pips, clock and sub line inside it. `rail.set(audience, { cover: false })` keeps the floating look per player. Needs the game-kit workshop addon republished (stylesheet change).
