/**
 * ctx.ui — lifecycle-bound custom HUD types. Runtime lives in games/cs2/js/ui.js (injected).
 * Import types and the default descriptor from `@s2script/cs2`; drive panels via `ctx.ui.hud()`.
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

/** What a layout offers — the contract between Panorama markup and server-side drive calls. */
export interface LayoutDescriptor {
  /** Decimal workshop addon ids clients must mount (via MultiAddonManager). */
  readonly addons: readonly string[];
  /** Source `.xml` path under `panorama/layout/custom_game/`. */
  readonly resource: string;
  /** Class that hides a panel when applied. */
  readonly hideClass: string;
  /** panelId -> dialog variable name for {@link Hud.setText}. */
  readonly text: Readonly<Record<string, string>>;
  /** Button ids delivered by {@link Hud.onClick} / {@link CtxUi.onCustomHudClicked}. */
  readonly buttons: readonly string[];
  /** Meter name -> fill panel id (width driven via s2-w0..s2-w10 classes). */
  readonly meters: Readonly<Record<string, string>>;
  /** Named pools of fixed slots (e.g. list rows). Each slot carries its own var list. */
  readonly slots?: Readonly<Record<string, readonly LayoutSlot[]>>;
}

/**
 * Default descriptor for the shipped `s2script_hud.xml` in workshop addon 3790153369.
 *
 * **This targets a PROBE, not a HUD.** `s2script_hud.xml` carries literal text and has no dialog
 * variable bindings at all, so a panel driven through this descriptor permanently reads
 * "S2SCRIPT PROBE OK" and no call can change it. That is deliberate: literal text is what let us
 * prove the render pipeline end to end when nothing else could be trusted, and standalone-renders
 * and driveable-by-a-plugin cannot both be true of one layout.
 *
 * Use it to answer "is my addon mounted and rendering at all?" — and nothing else. For real UI,
 * use {@link CtxUi.components}, which drives the generic pool in `s2script_lib.xml` and hands out
 * panels through a claim so two plugins cannot collide on the same one.
 */
export declare const DEFAULT_HUD_DESCRIPTOR: LayoutDescriptor;

/** Block-scoped view of one custom HUD click. Valid only during the handler. */
export interface OnCustomHudClickedView {
  /** The clicking controller, books-gated. Null when unresolved. */
  readonly player: EntityRef | null;
  /** Globally scoped button id from the layout markup. */
  readonly buttonId: string;
}

/** Typed custom HUD bound to one layout descriptor and its owned entity. */
export interface Hud {
  readonly layout: LayoutDescriptor;
  show(slot: number, panelId: string, opts?: { cursor?: boolean }): HudResult;
  hide(slot: number, panelId: string): HudResult;
  cursor(slot: number, on: boolean): HudResult;
  /** Set text where panel id equals the dialog variable name. */
  set(slot: number, id: string, value: string | number): HudResult;
  /** Set text using the descriptor's explicit text map. */
  setText(slot: number, panelId: string, value: string): HudResult;
  setClass(slot: number, panelId: string, className: string, on: boolean): HudResult;
  /** Set a meter 0..100 (quantized to 10% CSS steps s2-w0..s2-w10). */
  setMeter(slot: number, meterName: string, percent: number): HudResult;
  capacity(poolName: string): number;
  setPool(slot: number, poolName: string, entries: readonly (readonly string[])[]): HudResult;
  onClick(buttonId: string, handler: (slot: number) => void): void;
  setDisabled(slot: number, buttonId: string, disabled: boolean): HudResult;
  forget(slot: number): void;
}

export interface CtxUi {
  /**
   * The component library — pooled generic panels driven with data, not ids. Prefer this over
   * {@link CtxUi.hud} unless you ship your own workshop layout.
   */
  components(descriptor?: LayoutDescriptor): Components;
  /**
   * Spawn the layout entity for `descriptor`. Returns null on success, or a reason.
   *
   * Call from player-join (`ctx.clients.onActive`), a game event, a command, or any other callback
   * after a client is active. `hud()` / {@link CtxUi.components} also spawn at that point, so
   * this is only needed to force a spawn before the first of those calls. OnMapStart is still
   * too early — wait for an active client. Idempotent.
   */
  createLayout(descriptor?: LayoutDescriptor): HudResult;

  /** Default shipped descriptor, or pass a custom {@link LayoutDescriptor}. */
  hud(descriptor?: LayoutDescriptor): Hud;
  /**
   * Raw click observer — player + globally scoped button id only. Always observes; cannot suppress
   * the engine/map handler. Prefer {@link Hud.onClick} for typed routing.
   */
  onCustomHudClicked(handler: (view: OnCustomHudClickedView) => HookResultValue | void): void;
}

declare module "@s2script/sdk/plugin" {
  interface PluginContext {
    readonly ui: CtxUi;
  }
}

// ── component library ─────────────────────────────────────────────────────────────────────────
// `ctx.ui.hud()` is the primitive: it drives ids some .xml declares, so using it directly means
// authoring and publishing your own workshop layout. `ctx.ui.components()` is the library — a
// shared pool of generic panels every plugin drives with DATA instead of ids.
//
// Prefer it. Beyond the ergonomics, the engine caps how many distinct panel ids, class names and
// dialog variables the server may reference, and those vectors live on the ENTITY — so private
// per-plugin layouts all compete for one budget and fail late, when the Nth plugin loads. The
// shared pool is interned once and reused, so cost tracks what is on screen, not plugin count.

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
  readonly onClick: (slot: number) => void;
}

export interface ModalSpec {
  readonly title: string | ((slot: number) => string);
  /** Defaults to a page indicator when the list pages. */
  readonly subtitle?: string | ((slot: number) => string);
  /** Full list; the library pages it. Called per repaint, so it may read live state. */
  readonly rows: readonly Row[] | ((slot: number) => readonly Row[]);
  readonly onPick?: (slot: number, index: number, row: Row) => void;
  /**
   * Up to 4 detail lines for the selected row. The LAST line renders in a clamped, fixed-height
   * box — put attacker-controlled text there, and escape it before it reaches this call.
   */
  readonly detail?: (slot: number, row: Row | undefined, cursor: number) => readonly string[];
  // `cursor` is the ABSOLUTE index into the full row list, matching onPick and Modal.cursor().
  /** Up to 5; Prev/Next claim the trailing two automatically when the list pages. */
  readonly buttons?: readonly FooterButton[];
  readonly pageSize?: number;
  /** Sheet width. Default `md` (560px). */
  readonly width?: "sm" | "md" | "lg" | "xl";
}

export interface Modal {
  open(slot: number): void;
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
  /** Return the pooled panel so another plugin may claim it. */
  release(): void;
}

export interface BadgeSpec {
  readonly corner?: "tl" | "tr" | "bl" | "br";
  readonly title?: string;
  readonly accent?: "accent" | "good" | "warn" | "bad";
}

/** A persistent corner element — the thing chat cannot be, because chat scrolls away. */
export interface Badge {
  show(slot: number, data?: { title?: string; text?: string }): void;
  hide(slot: number): void;
  release(): void;
}

export interface ToastSpec {
  readonly title?: string;
  readonly message?: string;
  readonly variant?: Variant;
  /** 0 keeps it up until something replaces it. Default 6. */
  readonly holdSeconds?: number;
}

export interface Components {
  readonly descriptor: LayoutDescriptor;
  /**
   * Spawn the pool's layout entity. Same timing as {@link CtxUi.createLayout}.
   * `components()` also spawns once a client is active.
   */
  ensure(): HudResult;
  /** The underlying primitive, for anything the library does not cover. */
  readonly hud: Hud;
  /** Claim a pooled modal. Null when all are in use. */
  modal(spec: ModalSpec): Modal | null;
  /** Claim a pooled corner badge. Null when all are in use. */
  badge(spec?: BadgeSpec): Badge | null;
  toast(slot: number, spec: ToastSpec): HudResult;
  /** Hide every pooled panel for one player. */
  hideAll(slot: number): void;
  forget(slot: number): void;
  /** Names interned so far against the engine cap, for logging. */
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
