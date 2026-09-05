---
"@s2script/cs2": patch
---

ui: key the paint caches to the layout entity's identity, and make disconnect teardown unconditional

`setText` / `setClass` / `show` / `hide` suppress redundant engine calls by
comparing against a cached last-known value. Those caches describe state that
lives on the layout ENTITY — and they used to outlive it. After a map change
the new entity has every panel at its markup default and no input capture, but
the caches still claimed everything was set: any value unchanged since the
previous map was suppressed and never re-sent (a partial paint — a sheet draws
its rows but not its buttons, intermittently by construction), and a surviving
cursor lease made `releaseCursor` refuse to disable a capture the new entity
never had — a player holding a pointer no click can clear.

The caches are now bound to the entity's host id: every resolve passes through
an identity gate that drops all of them (`lastValue`, `cursorLeases`,
`visiblePanels`, `meterClass`, `disabled`) the first time a DIFFERENT entity is
resolved. That covers a map change and an entity silently replaced mid-map
alike, without depending on any lifecycle notification having fired.
`resetForMap` also clears them eagerly so stale cursor leases die immediately
rather than at the next paint. Click handlers are deliberately not cleared:
they are registered once per button id and belong to the plugin, not the
entity.

Disconnect teardown (`forget(slot)`) is now authoritative and engine-first:
input capture is forced off unconditionally — never gated on the lease books,
which menuhud's `discardSession` or an earlier forget may already have emptied
— and every class the books say the slot holds (visible panels, meter width,
disabled buttons) is painted back to markup default so the slot's next
occupant starts clean.
