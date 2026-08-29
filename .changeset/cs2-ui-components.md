---
"@s2script/cs2": minor
---

Add `ctx.ui.components()` — a shared, pooled Panorama component library.

`ctx.ui.hud()` drives panel ids that some `.xml` declares, so using it directly
means authoring and publishing your own workshop layout before you can draw a
row. `components()` is the library over that primitive: plugins describe data
(rows, titles, handlers) and never touch an id. Paging, selection, per-player
state and the reveal are handled for them.

This also keeps plugins inside an engine limit. `CCSCustomHudLayout` interns
every panel id, class name and dialog variable the server references into three
networked vectors, each capped at 1024, and those vectors belong to the ENTITY —
every plugin shares them. Private per-plugin layouts consume that budget
multiplicatively and fail late, when the Nth plugin loads. A shared pool is
interned once and reused, so cost tracks what is on screen rather than plugin
count. `Components.budget()` reports the three counts separately, because they
are three separate 1024s and a combined total is wrong in both directions.

Also in this change:

- `ctx.ui.createLayout()` / `components().ensure()` spawn the layout entity
  once a client is active — from player-join, a game event, a command, or any
  other post-ready callback. `hud()` / `components()` also spawn at that point.
  OnMapStart is still too early (the world is not up). A drive never creates
  the entity as a side-effect of paint.
- `Modal.cursor()`, `select()` and the `detail()` callback all speak ABSOLUTE
  indices, matching `onPick`. They previously mixed absolute and page-relative,
  so "act on the selection" operated on the wrong row on every page but the
  first — silently, and only once a list was long enough to page.
