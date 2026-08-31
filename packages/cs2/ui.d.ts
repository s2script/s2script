/**
 * `custom_hud_layout` — CS2's server-driven Panorama entity (`CCSCustomHudLayout`).
 *
 * Runtime lives in games/cs2/js/ui.js (injected). Import `{ CustomHudLayout }` from
 * `@s2script/cs2`. The engine object is one layout entity; almost every drive is per-player
 * (`SetHasClassForPlayer`, `SetDialogVariableStringForPlayer`, …), so paint through
 * {@link HudLayout.forSlot}.
 *
 * This is not a general UI toolkit. Panels cannot be created at runtime — a layout XML declares a
 * fixed pool of ids, and the server can only toggle classes and set dialog-variable strings. For
 * menus/toasts/badges without shipping your own workshop addon, use {@link hudkit}.
 */
import type { HookResultValue } from "@s2script/sdk/events";
import type { EntityRef } from "@s2script/sdk/entity";

/** Result of a drive call: null on success, or a human-readable reason it did not happen. */
export type HudResult = string | null;

/** One pre-declared row/slot in a pooled collection. */
export interface LayoutSlot {
  readonly id: string;
  readonly vars: readonly string[];
}

/**
 * What a `custom_hud_layout` entity will load and how the server may drive it.
 *
 * `resource` and `addons` are required. Everything else has a default: `hideClass` is `"s2-hide"`,
 * `text`/`buttons`/`meters` are empty. {@link HudLayout.set} always uses id == dialog variable
 * name; {@link HudLayout.setText} uses `text` when a panel's variable differs.
 */
export interface CustomHudSpec {
  /** Decimal workshop addon ids clients must mount (via MultiAddonManager). */
  readonly addons: readonly string[];
  /** Source `.xml` path under `panorama/layout/custom_game/`. */
  readonly resource: string;
  /** Class that hides a panel when applied. Default `"s2-hide"`. */
  readonly hideClass?: string;
  /** panelId -> dialog variable name for {@link HudLayout.setText}. Default: none. */
  readonly text?: Readonly<Record<string, string>>;
  /** Button ids delivered by {@link HudLayout.onClick} / {@link CustomHudLayoutNs.onClicked}. */
  readonly buttons?: readonly string[];
  /** Meter name -> fill panel id (width driven via s2-w0..s2-w10 classes). */
  readonly meters?: Readonly<Record<string, string>>;
  /** Named pools of fixed slots (e.g. list rows). Each slot carries its own var list. */
  readonly slots?: Readonly<Record<string, readonly LayoutSlot[]>>;
}

/**
 * @deprecated Use {@link CustomHudSpec}. Same shape; `hideClass`/`text`/`buttons`/`meters` were
 * required on this older name and are optional on {@link CustomHudSpec}.
 */
export type LayoutDescriptor = CustomHudSpec & {
  readonly hideClass: string;
  readonly text: Readonly<Record<string, string>>;
  readonly buttons: readonly string[];
  readonly meters: Readonly<Record<string, string>>;
};

/**
 * Spec for the shipped `s2script_hud.xml` probe in workshop addon 3790153369.
 *
 * **This targets a PROBE, not a driveable HUD.** The XML carries literal text and has no dialog
 * variable bindings, so a panel driven through it permanently reads "S2SCRIPT PROBE OK". Use
 * {@link CustomHudLayout.probe} to answer "is my addon mounted and rendering at all?" — and
 * nothing else. For real UI, {@link hudkit} or {@link CustomHudLayout.create} with your own layout.
 */
export declare const PROBE_LAYOUT: LayoutDescriptor;

/** @deprecated Use {@link PROBE_LAYOUT}. */
export declare const DEFAULT_HUD_DESCRIPTOR: LayoutDescriptor;

/** Block-scoped view of one engine CustomHudClicked delivery. Valid only during the handler. */
export interface CustomHudClickedView {
  /** The clicking controller, books-gated. Null when unresolved. */
  readonly player: EntityRef | null;
  /** 0-based player slot, or -1 when {@link CustomHudClickedView.player} did not resolve. */
  readonly slot: number;
  /** Globally scoped button id from the layout markup. */
  readonly buttonId: string;
}

/** @deprecated Use {@link CustomHudClickedView}. */
export type OnCustomHudClickedView = CustomHudClickedView;

/**
 * One player's state on a {@link CustomHudLayout}. The engine's `ForPlayer` calls — classes,
 * dialog variables, input capture — are all per-slot, which is why this object exists.
 *
 * Unchanged values are not re-sent (the networked intern tables are not free).
 */
export interface HudPlayer {
  /** 0-based player slot this view drives. */
  readonly slot: number;
  show(panelId: string, opts?: { cursor?: boolean }): HudResult;
  hide(panelId: string): HudResult;
  cursor(on: boolean): HudResult;
  /** Set text where panel id equals the dialog variable name. Also accepts a field map. */
  set(id: string, value: string | number): HudResult;
  set(fields: Readonly<Record<string, string | number>>): HudResult;
  /** Set text using the spec's `text` map; falls back to id == variable name. */
  setText(panelId: string, value: string): HudResult;
  setText(fields: Readonly<Record<string, string>>): HudResult;
  setClass(panelId: string, className: string, on: boolean): HudResult;
  /** Set a meter 0..100 (quantized to 10% CSS steps s2-w0..s2-w10). */
  setMeter(meterName: string, percent: number): HudResult;
  setPool(poolName: string, entries: readonly (readonly string[])[]): HudResult;
  setDisabled(buttonId: string, disabled: boolean): HudResult;
  forget(): void;
}

/**
 * A plugin-owned `custom_hud_layout` entity bound to one layout XML.
 *
 * Created with {@link CustomHudLayout.create}. The entity is spawned on the first
 * `SIGNON_ACTIVE` client (never from OnMapStart — that segfaults). Drive it through
 * {@link HudLayout.forSlot}; slot-first methods stay for bulk loops.
 *
 * @example
 * import { CustomHudLayout } from "@s2script/cs2";
 * const layout = CustomHudLayout.create({
 *   addons: ["YOUR_ADDON_ID"],
 *   resource: "panorama/layout/custom_game/your_hud.xml",
 *   buttons: ["ok_btn"],
 * });
 * layout.onClick("ok_btn", (p) => { p.hide("dialog"); p.cursor(false); });
 * const p = layout.forSlot(slot);
 * p.set({ title: "Hello", ok_label: "OK" });
 * p.show("dialog", { cursor: true });
 */
export interface HudLayout {
  readonly spec: CustomHudSpec;
  /** @deprecated Use {@link HudLayout.spec}. */
  readonly layout: CustomHudSpec;
  /** Per-player view of this layout (cached per slot). */
  forSlot(slot: number): HudPlayer;
  /**
   * Spawn the layout entity if it is not already in the world. Same timing as
   * {@link CustomHudLayout.create}: after a client is `SIGNON_ACTIVE`. Idempotent.
   */
  ensure(): HudResult;
  show(slot: number, panelId: string, opts?: { cursor?: boolean }): HudResult;
  hide(slot: number, panelId: string): HudResult;
  cursor(slot: number, on: boolean): HudResult;
  set(slot: number, id: string, value: string | number): HudResult;
  set(slot: number, fields: Readonly<Record<string, string | number>>): HudResult;
  setText(slot: number, panelId: string, value: string): HudResult;
  setText(slot: number, fields: Readonly<Record<string, string>>): HudResult;
  setClass(slot: number, panelId: string, className: string, on: boolean): HudResult;
  setMeter(slot: number, meterName: string, percent: number): HudResult;
  capacity(poolName: string): number;
  setPool(slot: number, poolName: string, entries: readonly (readonly string[])[]): HudResult;
  /**
   * Route a layout XML button id (from {@link CustomHudSpec.buttons}) to a handler.
   * There is no widget object — `buttonId` is a string the markup already declared.
   * The handler receives the clicking player's {@link HudPlayer}.
   */
  onClick(buttonId: string, handler: (player: HudPlayer) => void): void;
  setDisabled(slot: number, buttonId: string, disabled: boolean): HudResult;
  forget(slot: number): void;
}

/** Load-window `custom_hud_layout` API. Throws after settle. */
export interface CustomHudLayoutNs {
  /**
   * Bind (and spawn, once a client is active) a `custom_hud_layout` for `spec`.
   * Same layout resource is reused if you call this twice.
   */
  create(spec: CustomHudSpec): HudLayout;
  /**
   * The shipped probe layout. Renders literal "S2SCRIPT PROBE OK" and is not driveable.
   * Use it to confirm the workshop addon is mounted.
   */
  probe(): HudLayout;
  /**
   * Raw click observer — player + globally scoped button id. Always observes; cannot suppress
   * the engine/map handler. Prefer {@link HudLayout.onClick} for typed routing.
   */
  onClicked(handler: (view: CustomHudClickedView) => HookResultValue | void): void;
  /** Shared panel pool (modals / toasts / badges) over `s2script_lib.xml`. */
  readonly kit: HudKit;
  /** Shortcut for {@link HudKit.toast}. */
  toast(slot: number, spec: ToastSpec): HudResult;

  /** Spec used by {@link CustomHudLayoutNs.probe}. */
  readonly PROBE: LayoutDescriptor;

  /**
   * @deprecated Use {@link CustomHudLayoutNs.create}. `hud()` with no argument is {@link CustomHudLayoutNs.probe}.
   */
  hud(descriptor?: CustomHudSpec): HudLayout;
  /**
   * @deprecated Use {@link CustomHudLayoutNs.kit} or {@link hudkit}.
   */
  components(descriptor?: CustomHudSpec): HudKit;
  /**
   * @deprecated Call {@link HudLayout.ensure} on the layout {@link CustomHudLayoutNs.create} returned.
   */
  createLayout(descriptor?: CustomHudSpec): HudResult;
  /**
   * @deprecated Use {@link CustomHudLayoutNs.onClicked}.
   */
  onCustomHudClicked(handler: (view: CustomHudClickedView) => HookResultValue | void): void;
}

/** @deprecated Use {@link HudLayout}. */
export type Hud = HudLayout;

/** @deprecated Use {@link CustomHudLayoutNs}. Same object as {@link CustomHudLayout}. */
export type CtxUi = CustomHudLayoutNs;

declare module "@s2script/sdk/plugin" {
  interface PluginContext {
    /**
     * @deprecated Import {@link CustomHudLayout} from `@s2script/cs2`. Runtime is the same object.
     */
    readonly ui: CustomHudLayoutNs;
  }
}

/**
 * Load-window `custom_hud_layout` API (`import { CustomHudLayout } from "@s2script/cs2"`).
 * Throws after settle.
 */
export declare const CustomHudLayout: CustomHudLayoutNs;

/**
 * @deprecated Use {@link CustomHudLayout}. Same load-window object.
 */
export declare const ui: CustomHudLayoutNs;

// ── hudkit ────────────────────────────────────────────────────────────────────────────────────
// CustomHudLayout is the primitive: it drives ids some .xml declares, so using it directly means
// authoring and publishing your own workshop layout. `hudkit` is the library over that primitive —
// a shared pool of generic panels every plugin drives with DATA instead of ids.
//
// Prefer it when you do not ship a layout. Beyond the ergonomics, the engine caps how many
// distinct panel ids, class names and dialog variables the server may reference, and those
// vectors live on the ENTITY — so private per-plugin layouts all compete for one budget and fail
// late, when the Nth plugin loads. The shared pool is interned once and reused, so cost tracks
// what is on screen, not plugin count.

/** Toast / footer-button colouring. `ghost` is the low-emphasis default for footers. */
export type Variant = "primary" | "good" | "warn" | "bad" | "ghost";

/** One list row. Three columns: `a` is primary and flexes, `b` and `c` are right-aligned. */
export interface Row {
  readonly a: string;
  readonly b?: string;
  readonly c?: string;
  /** Greys the row. Cosmetic only — `onPick` still fires, so you can say WHY it is unavailable. */
  readonly disabled?: boolean;
}

export interface FooterButton {
  readonly text: string;
  readonly variant?: Variant;
  readonly onClick: (slot: number, view: ModalView) => void;
}

export interface ModalSpec {
  readonly title: string | ((slot: number) => string);
  /** Defaults to a page indicator when the list pages. */
  readonly subtitle?: string | ((slot: number) => string);
  /** Full list; the library pages it. Called per repaint, so it may read live state. */
  readonly rows: readonly Row[] | ((slot: number) => readonly Row[]);
  readonly onPick?: (slot: number, index: number, row: Row, view: ModalView) => void;
  /**
   * Up to 4 detail lines for the selected row. The LAST line renders in a clamped, fixed-height
   * box — put attacker-controlled text there, and escape it before it reaches this call.
   */
  readonly detail?: (slot: number, row: Row | undefined, cursor: number) => readonly string[];
  // `cursor` is the ABSOLUTE index into the full row list, matching onPick and Modal.cursor().
  /** Up to 5; Prev/Next claim the trailing two automatically when the list pages. */
  readonly buttons?: readonly FooterButton[] | ((slot: number) => readonly FooterButton[]);
  readonly pageSize?: number;
  /** Sheet width. Default `md` (560px). */
  readonly width?: "sm" | "md" | "lg" | "xl";
}

/** Per-player view of a claimed {@link Modal}. */
export interface ModalView {
  readonly slot: number;
  open(opts?: { cursor?: boolean }): ModalView;
  close(): void;
  isOpen(): boolean;
  refresh(): void;
  page(delta: number): void;
  select(index: number): void;
  cursor(): number;
  forget(): void;
}

export interface Modal {
  /** Open for `slot` and return the bound view. Default grabs the cursor. */
  open(slot: number, opts?: { cursor?: boolean }): ModalView;
  /** Grab or release the mouse without closing the sheet. */
  setCursor(slot: number, on: boolean): void;
  close(slot: number): void;
  isOpen(slot: number): boolean;
  /** Repaint from live data. Omit `slot` to repaint every player who has it open. */
  refresh(slot?: number): void;
  page(slot: number, delta: number): void;
  /** Select by ABSOLUTE index into the full row list; pages to it if needed. */
  select(slot: number, index: number): void;
  /**
   * ABSOLUTE index of the highlighted row — the same space {@link ModalSpec.onPick} reports in,
   * so "act on what is selected" stays correct on every page.
   */
  cursor(slot: number): number;
  forget(slot: number): void;
  forSlot(slot: number): ModalView;
  /** Return the pooled panel so another plugin may claim it. */
  release(): void;
}

export interface BadgeSpec {
  readonly corner?: "tl" | "tr" | "bl" | "br";
  readonly title?: string;
  readonly accent?: "accent" | "good" | "warn" | "bad";
}

/** Per-player view of a claimed {@link Badge}. */
export interface BadgeView {
  readonly slot: number;
  show(data?: { title?: string; text?: string }): void;
  hide(): void;
}

/** A persistent corner element — the thing chat cannot be, because chat scrolls away. */
export interface Badge {
  show(slot: number, data?: { title?: string; text?: string }): BadgeView;
  hide(slot: number): void;
  forSlot(slot: number): BadgeView;
  release(): void;
}

export interface ToastSpec {
  readonly title?: string;
  readonly message?: string;
  readonly variant?: Variant;
  /** 0 keeps it up until something replaces it. Default 6. */
  readonly holdSeconds?: number;
}

export interface HudKitPlayer {
  readonly slot: number;
  toast(spec: ToastSpec): HudResult;
  hideAll(): void;
  forget(): void;
}

export interface HudKit {
  readonly spec: CustomHudSpec;
  /** @deprecated Use {@link HudKit.spec}. */
  readonly descriptor: CustomHudSpec;
  /**
   * Spawn the pool's layout entity. Same timing as {@link CustomHudLayout.ensure}.
   * {@link hudkit} also spawns once a client is active.
   */
  ensure(): HudResult;
  /** The underlying `custom_hud_layout`, for anything the library does not cover. */
  readonly layout: HudLayout;
  /** @deprecated Use {@link HudKit.layout}. */
  readonly hud: HudLayout;
  /** Claim a pooled modal. Null when all are in use. */
  modal(spec: ModalSpec): Modal | null;
  /** Claim a pooled corner badge. Null when all are in use. */
  badge(spec?: BadgeSpec): Badge | null;
  toast(slot: number, spec: ToastSpec): HudResult;
  forSlot(slot: number): HudKitPlayer;
  /** Hide every pooled panel for one player. */
  hideAll(slot: number): void;
  forget(slot: number): void;
  /**
   * Names interned so far, per engine vector. Each has its OWN `cap` of 1024 (not one shared
   * 3072), and all are shared by every plugin on the HUD entity — past the cap a name is refused
   * and its value simply never arrives. `set` spends from two vectors at once, charging its panel
   * id and its dialog variable name to different ledgers.
   *
   * Interning is idempotent, so these climb only on a name's FIRST use; repainting is free.
   */
  budget(): {
    panelIds: number;
    classNames: number;
    variables: number;
    declared: number;
    warnAt: number;
    cap: number;
  };
}

/** @deprecated Use {@link HudKit}. */
export type Components = HudKit;

/**
 * Shared panel pool over `s2script_lib.xml`. Load-window; throws after settle.
 * Same object as {@link CustomHudLayoutNs.kit}.
 *
 * @example
 * import { hudkit } from "@s2script/cs2";
 * const menu = hudkit.modal({
 *   title: "Players",
 *   rows: [{ a: "Alice" }, { a: "Bob" }],
 *   buttons: [{ text: "Close", onClick: (_s, v) => v.close() }],
 * });
 * menu?.forSlot(slot).open();
 * hudkit.forSlot(slot).toast({ title: "Saved", message: "loadout written" });
 */
export declare const hudkit: HudKit;
