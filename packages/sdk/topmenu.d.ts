/** @s2script/topmenu — the extensible admin/top menu registry. NO runtime code (injected at load). */

/** A single registered top-menu entry (an admin command surfaced in the adminmenu). */
export interface TopMenuItem {
  /** Plugin-namespaced unique id, e.g. "playercommands:slap". A duplicate id replaces. */
  id: string;
  /** Shown label. */
  name: string;
  /** Required ADMFLAG bit mask (adminmenu hides items the caller lacks flags for). */
  flags: number;
  /** Runs in THIS plugin's context when an admin selects the item; receives the admin's 0-based slot. */
  onSelect: (adminSlot: number) => void;
}
/** A handler-free copy of the whole registry — what a menu renderer reads to build the display. */
export interface TopMenuSnapshot {
  /** All category names, in registration order. */
  categories: string[];
  /** Every registered item's metadata (no `onSelect`), tagged with its category. */
  items: { id: string; category: string; name: string; flags: number }[];
}
/**
 * The shared admin/top-menu registry — renderers read {@link TopMenu.snapshot} and dispatch picks
 * through {@link TopMenu.select}; items are contributed by plugins via their own ctx.topmenu.
 * @example
 * import { TopMenu } from "@s2script/sdk/topmenu";
 * const snap = TopMenu.snapshot();
 * for (const cat of snap.categories) renderCategory(cat);
 * // when the admin picks an item id, dispatch it back to its owner plugin:
 * m.onSelect(e => { TopMenu.select(e.info, slot); });
 */
export declare const TopMenu: {
  /** All categories + item metadata (no handlers) — the adminmenu renderer reads this. */
  snapshot(): TopMenuSnapshot;
  /** Fire an item's onSelect (dispatched post-frame to the owner's context). */
  select(id: string, slot: number): void;
};
