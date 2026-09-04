# TopMenu dashboard tabs — design spec

**Status:** implemented this slice.
**Date:** 2026-09-04.
**Scope:** `sm_admin` / `sm_menu` become one tabbed dashboard. Each plugin that
contributes TopMenu items declares its own tab and options. Engine-generic
registry stays in core; CS2 paint is `hudkit.dashboard()` over `s2_dash`.

## Why

The hub was a SourceMod-style category list you drill into. The intended
surface is the tabbed dashboard from the unpublished `s2script_dash` workshop
layout: one sheet, selectable tabs, each tab a plugin's own options.

## Locked decisions

1. **One tab per plugin that declares one.** `topmenu.addTab({ id, title })`
   then `topmenu.addItem(tabId, item)`. Shared SM category names still work
   (`addCategory("Player Commands")` is `{ id, title }` with both equal).
2. **Registry stays core / engine-generic.** Snapshot grows `tabs: [{id,title}]`.
   `categories` remains tab ids in insertion order. No CS2 types in core.
3. **Paint is `hudkit.dashboard()` over `s2_dash`.** One dedicated panel family
   on `s2script_lib.xml`, not a third `s2_mN` sheet and not a modal-pool slot.
   Menu remains for pickers (`pickPlayer`, change-map). Republish workshop
   addon 3790153369 after compiling the new XML/CSS.
4. **Selecting an option closes the hub, then `TopMenu.select`.** The item's
   `onSelect` may open a Menu. Freeze + cursor while the hub is open (user
   asked). Disconnect / activate drop freeze without restoring a stale moveType.
5. **Empty tabs are omitted.** A tab with no flag-visible items on this sheet
   is not painted. An empty hub still replies and does not open a blank sheet.
6. **`sheets` stay on the item.** Admin vs `!menu` filtering is unchanged.

## Authoring

```ts
import { topmenu, ADMFLAG } from "@s2script/sdk";

topmenu.addTab({ id: "playercommands", title: "Players" });
topmenu.addItem("playercommands", {
  id: "playercommands:slap",
  name: "Slap",
  flags: ADMFLAG.SLAY,
  onSelect: (slot) => { /* pick + slap */ },
});
```

## Non-goals

- Per-plugin custom dashboard chrome (widgets, meters, live stats).
- Auto-creating a tab from the plugin id when `addTab` is omitted — items
  still land on the category / tab id passed to `addItem`.
- Shipping a second layout (`s2script_dash.xml`). The hub lives on the shared
  lib so it does not spend a second intern budget.
