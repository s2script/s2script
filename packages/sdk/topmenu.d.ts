/** @s2script/topmenu — the extensible admin/top menu registry. NO runtime code (injected at load). */
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
export interface TopMenuSnapshot {
  categories: string[];
  items: { id: string; category: string; name: string; flags: number }[];
}
export declare const TopMenu: {
  /**
   * Register a category (idempotent; order = first-registration order).
   *
   * @deprecated moved to ctx.topmenu.addCategory (L1 lifecycle v2) — removed after the port fan-out
   */
  addCategory(name: string): void;
  /**
   * Register/replace an item under a category (auto-creates the category).
   *
   * @deprecated moved to ctx.topmenu.addItem (L1 lifecycle v2) — removed after the port fan-out
   */
  addItem(category: string, item: TopMenuItem): void;
  /** All categories + item metadata (no handlers) — the adminmenu renderer reads this. */
  snapshot(): TopMenuSnapshot;
  /** Fire an item's onSelect (dispatched post-frame to the owner's context). */
  select(id: string, slot: number): void;
};
