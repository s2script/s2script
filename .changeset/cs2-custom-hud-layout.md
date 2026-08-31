---
"@s2script/cs2": minor
---

Reshape the CS2 HUD API around `custom_hud_layout`.

`ui` was a kitchen-sink namespace that did not match the engine object: a `custom_hud_layout` entity, driven per-player. The public face is now `CustomHudLayout.create(spec)` returning a `HudLayout`, painted through `layout.forSlot(slot)` so drive calls do not re-thread the slot. Clicks hand back that same player view. Unchanged values are not re-sent. `hudkit` is the shared modal/toast/badge pool (formerly `ui.components()`).

`ui` / `hud()` / `components()` remain as deprecated aliases of the same objects.
