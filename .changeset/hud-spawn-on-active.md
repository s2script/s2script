---
"@s2script/cs2": minor
---

Spawn custom HUD layouts when a client becomes active.

`hud()` / `components()` / `createLayout()` register the descriptor at load and create the layout entity on `SIGNON_ACTIVE`, so player-join and game events can drive panels. `OnMapStart` still only resets. A drive never creates the entity as a side-effect of paint.
