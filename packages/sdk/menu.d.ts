/** @s2script/menu — interactive player menus (chat + center backends). NO runtime code (injected at load). */

/** The display backend a {@link Menu} renders through — the value of {@link Menu.style}. */
export declare const enum MenuStyle {
  /** Chat-log backend: numbered lines printed to chat, picked by typing the number. */
  Chat = "chat",
  /** CS2 center-screen HTML backend: W/S move the cursor, E selects; re-painted each tick. */
  Center = "center",
}
/** Why a menu closed without a selection — the {@link MenuCancelEvent.reason}. */
export declare const enum MenuCancelReason {
  /** The player chose the Exit control (or `close`/`cancel` was called). */
  Exit = 0,
  /** The `display` seconds elapsed with no selection. */
  Timeout = 1,
  /** The player left the server. */
  Disconnect = 2,
  /** A newer menu was shown to the same slot, superseding this one. */
  NewMenu = 3,
}

/** Payload for {@link Menu.onSelect} — the item a player picked. */
export interface MenuSelectEvent {
  /** 0-based player slot that made the selection. */
  readonly slot: number;
  /** 0-based index of the chosen item within the menu's item list. */
  readonly item: number;
  /** The `info` string the item was added with — the machine key, returned to your handler. */
  readonly info: string;
  /** The visible label the item was added with. */
  readonly display: string;
}
/** Payload for {@link Menu.onCancel} — a menu that closed without a selection. */
export interface MenuCancelEvent {
  /** 0-based player slot whose menu closed. */
  readonly slot: number;
  /** Why it closed (see {@link MenuCancelReason}). */
  readonly reason: MenuCancelReason;
}

/**
 * A slot-based interactive menu. Add items, wire {@link Menu.onSelect}/{@link Menu.onCancel},
 * then {@link Menu.display} to a player slot. The model paginates automatically (7 items per chat
 * page) and picks a renderer by {@link Menu.style}, falling back to Chat when the style's renderer
 * is unregistered. Only one menu is active per slot; displaying a new one supersedes the old
 * ({@link MenuCancelReason.NewMenu}).
 * @example
 * import { Menu, MenuStyle } from "@s2script/sdk/menu";
 * const m = new Menu("s2script Menu Demo");
 * m.style = MenuStyle.Center;
 * m.freezePlayer = true;                 // freeze movement while the WASD menu is open (nav still works)
 * m.addItem("hp", "Heal to 100");
 * m.addItem("noclip", "Toggle Noclip");
 * m.onSelect(e => { console.log(`picked ${e.info} (slot ${e.slot})`); });
 * m.display(slot, 30);
 */
export declare class Menu {
  constructor(title?: string);
  /** Heading shown above the item list. */
  title: string;
  /** Which registered renderer to use. Falls back to Chat if the style's renderer is unregistered. */
  style: MenuStyle;
  /** Append an auto Exit control (default true). */
  exitButton: boolean;
  /** When true, a renderer that supports it (the CS2 center renderer) freezes the player's movement
   *  while the menu is open and restores it on close — buttons still register, so WASD nav still works.
   *  The chat renderer ignores it. Default false (movement allowed, the normal behavior). */
  freezePlayer: boolean;
  /** (info, display) — `info` is returned to onSelect; `display` is shown. */
  addItem(info: string, display: string, opts?: { disabled?: boolean }): void;
  /** Register the callback fired when a player picks an item. Replaces any previous handler. */
  onSelect(handler: (e: MenuSelectEvent) => void): void;
  /** Register the callback fired when the menu closes without a selection (see {@link MenuCancelReason}). Replaces any previous handler. */
  onCancel(handler: (e: MenuCancelEvent) => void): void;
  /** Show to a 0-based slot for `seconds` (0 = until selection/cancel/disconnect). */
  display(slot: number, seconds?: number): void;
  /** Close an open menu for `slot` early. */
  close(slot: number): void;
  /** Register a display backend under a MenuStyle value (used by the CS2 center renderer). */
  static registerRenderer(name: string, renderer: MenuRenderer): void;
}

/** A live display of one menu to one slot, passed to a {@link MenuRenderer}. Owns the page/cursor state and exposes the input verbs a renderer drives. */
export interface MenuSession {
  /** 0-based player slot this session is displayed to. */
  readonly slot: number;
  /** The resolved snapshot the renderer paints: heading, per-line state, and 0-based page position. */
  view(): { title: string; lines: MenuLine[]; page: number; pageCount: number; exit: boolean };
  /** Chat idiom: apply a number-key press against the current view (1..7 select, 8=Back, 9=Next, 0=Exit). */
  pickNumber(n: number): void;
  /** Center idiom: move the cursor up over the current page's nav targets (items + Back/Next/Exit), wrapping. */
  moveUp(): void;
  /** Center idiom: move the cursor down over the current page's nav targets, wrapping. */
  moveDown(): void;
  /** Center idiom: activate the current cursor target (select an item, paginate, or exit). */
  confirm(): void;
  /** Close this session with an {@link MenuCancelReason.Exit}. */
  cancel(): void;
}
/** One rendered line of a {@link MenuSession} view. */
export interface MenuLine {
  /** The line's visible text. */
  text: string;
  /** The chat number key ("1".."0") that picks this line, or null for an unselectable/disabled line. */
  key: string | null;
  /** True if this line is a selectable item. */
  selectable: boolean;
  /** True if the center cursor is currently on this line (the renderer highlights it). */
  cursor?: boolean;
  /** For a control line, which control it is: `"back"`, `"next"`, or `"exit"` (absent on item lines). */
  control?: string;
  /** For an item line, its 0-based index into the menu's item list (absent on control lines). */
  index?: number;
}
/** A display backend for {@link Menu} — registered via {@link Menu.registerRenderer}, driven with a {@link MenuSession}. */
export interface MenuRenderer {
  /** Show the session for the first time (the menu just opened). */
  open(session: MenuSession): void;
  /** Re-paint the session after a page/cursor change. */
  update(session: MenuSession): void;
  /** Tear down the display for `slot` (the menu closed). */
  close(slot: number): void;
}
