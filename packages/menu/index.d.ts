/** @s2script/menu — interactive player menus (chat + center backends). NO runtime code (injected at load). */
export declare const enum MenuStyle { Chat = "chat", Center = "center" }
export declare const enum MenuCancelReason { Exit = 0, Timeout = 1, Disconnect = 2, NewMenu = 3 }

export interface MenuSelectEvent { readonly slot: number; readonly item: number; readonly info: string; readonly display: string; }
export interface MenuCancelEvent { readonly slot: number; readonly reason: MenuCancelReason; }

export declare class Menu {
  constructor(title?: string);
  title: string;
  /** Which registered renderer to use. Falls back to Chat if the style's renderer is unregistered. */
  style: MenuStyle;
  /** Append an auto Exit control (default true). */
  exitButton: boolean;
  /** (info, display) — `info` is returned to onSelect; `display` is shown. */
  addItem(info: string, display: string, opts?: { disabled?: boolean }): void;
  onSelect(handler: (e: MenuSelectEvent) => void): void;
  onCancel(handler: (e: MenuCancelEvent) => void): void;
  /** Show to a 0-based slot for `seconds` (0 = until selection/cancel/disconnect). */
  display(slot: number, seconds?: number): void;
  /** Close an open menu for `slot` early. */
  close(slot: number): void;
  /** Register a display backend under a MenuStyle value (used by the CS2 center renderer). */
  static registerRenderer(name: string, renderer: MenuRenderer): void;
}

export interface MenuSession {
  readonly slot: number;
  view(): { title: string; lines: MenuLine[]; page: number; pageCount: number; exit: boolean };
  pickNumber(n: number): void;
  moveUp(): void; moveDown(): void; confirm(): void; cancel(): void;
}
export interface MenuLine { text: string; key: string | null; selectable: boolean; cursor?: boolean; control?: string; index?: number; }
export interface MenuRenderer { open(session: MenuSession): void; update(session: MenuSession): void; close(slot: number): void; }
