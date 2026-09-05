---
"@s2script/cs2": patch
---

ui: drop the paint caches when the layout entity does

`setText` / `setClass` / `show` / `hide` all suppress a redundant engine call by
comparing against a cached last-known value. Those caches describe state that
lives on the layout ENTITY — and `resetForMap` dropped the entity reference
while leaving every cache intact.

The next map creates a new entity with every panel at its markup default and no
input capture, against caches that still claim everything is already set. The
result is a partial paint: any value unchanged since the previous map is
suppressed and never re-sent, so a sheet draws its rows and not its buttons, or
its title and not its rows — intermittently, depending on what happened to
differ.

The surviving cursor lease is worse than cosmetic. `releaseCursor` sees a
non-empty lease set, concludes something still wants the pointer, and never
disables a capture the new entity never had — leaving a player holding a cursor
that no click can clear and no close can release.

`resetForMap` now clears `lastValue`, `cursorLeases`, `visiblePanels`,
`meterClass` and `disabled` on every hud. Click handlers are deliberately NOT
cleared: they are registered once per button id at claim time and belong to the
plugin, not to the entity.
