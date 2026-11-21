---
"@s2script/cs2": minor
---

`GameHud.target()` claims a crosshair target label: a name and an optional sub line centred just under the crosshair, per player, for "who you are looking at". `set(audience, { name, sub?, tone? })` is safe to call every tick (only a change is sent), and the label fades in and out in about 0.1 s so a sweeping aim does not blink. It collapses under the scoreboard and buy menu. Needs the game-kit workshop addon republished (new `gk_target` panel).
