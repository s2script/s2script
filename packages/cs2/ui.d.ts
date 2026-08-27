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

/** Default descriptor for the shipped s2script_hud.xml + workshop addon 3790153369. */
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
