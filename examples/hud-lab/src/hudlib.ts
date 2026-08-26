/**
 * A typed component API over a custom_hud_layout.
 *
 * THE CONSTRAINT THAT SHAPES ALL OF THIS: panels cannot be created at runtime. There is no
 * appendChild, no templating, and no client-side scripting (Valve's point_script.d.ts says so
 * outright). A layout declares a FIXED POOL of slots and the server can only ever:
 *
 *     SetHasClassForPlayer(slot, panelId, className, status)   — variant / visibility
 *     SetDialogVariableStringForPlayer(slot, panelId, var, s)  — text
 *     SetInputCaptureEnabled(slot, bool)                       — clicks on/off
 *
 * So this is not a factory. It is a pool manager with a typed face. Three consequences drive the
 * design, and each is a thing a caller would otherwise get wrong:
 *
 *   1. EVERY COLLECTION HAS A HARD MAXIMUM baked into the markup. `rows`/`toasts` expose it as
 *      `capacity` and REFUSE overflow rather than silently dropping — a list that quietly shows the
 *      first 6 of 9 entries is a bug you find in production.
 *   2. A METER CANNOT TAKE A NUMBER. Width is a step class (`s2-w-0`..`s2-w-100` in 5% increments),
 *      so the rounding has to live here rather than in every call site.
 *   3. "DISABLED" IS PURELY VISUAL. `.s2-btn-disabled` greys a button and nothing more — the click
 *      still arrives. Enforcement has to be server-side, so `onClick` does it centrally.
 */
import type { EntityRef } from "@s2script/sdk/entity";
import { delay } from "@s2script/sdk/timers";
import { setPlayerInputCapture } from "./hudstate";
import * as tierb from "./tierb";
import { HudPanelClassStatus } from "./offsets";

/** Result of a drive call: null on success, or a human-readable reason it did not happen. */
export type HudResult = string | null;

/** A pooled collection of pre-declared slots — a table's rows, a toast stack. */
export interface SlotPool {
  /** Panel ids of each slot, in display order. `capacity` is `ids.length`. */
  ids: readonly string[];
  /** Dialog-variable names on each slot, parallel to the fields a caller sets. */
  vars: readonly string[];
}

/** What a layout offers. This is the seam a third-party layout plugs into. */
export interface LayoutDescriptor {
  /** Value for the entity's `layout` keyvalue. MUST use the `.xml` source extension — a client
   *  rejects `.vxml`/`.vxml_c` with "Layout xml is an invalid resource name". */
  resource: string;
  /** Class that hides a panel. Applied to hide, cleared to show. */
  hideClass: string;
  /** Simple text targets: panelId -> dialog variable name. */
  text: Readonly<Record<string, string>>;
  /** Button ids that can be clicked, i.e. what `OnCustomHudClicked` will report. */
  buttons: readonly string[];
  /** Pooled collections by name (e.g. "rows", "toasts"). */
  pools: Readonly<Record<string, SlotPool>>;
  /** Meter panels: the fill panel whose width class gets swapped. */
  meters: Readonly<Record<string, string>>;
}

/** Meter width steps exist at 5% increments only; anything else silently does nothing. */
const METER_STEP = 5;

/** `s2-w-0` … `s2-w-100`. Exported so a layout author can verify their CSS covers the same set. */
export function meterClassFor(percent: number): string {
  const clamped = Math.max(0, Math.min(100, percent));
  const stepped = Math.round(clamped / METER_STEP) * METER_STEP;
  return `s2-w-${stepped}`;
}

/**
 * A bound view of one layout, for one entity.
 *
 * Every method takes a player slot: this API is per-player throughout, because the underlying
 * engine calls are — and because a panel does not render for a player at all until that player has
 * per-player state on the layout.
 */
export class Hud {
  /** Buttons the caller has disabled, per slot. Enforced in `dispatchClick`, since the engine's
   *  "disabled" is a colour and nothing more. */
  private readonly disabled = new Map<number, Set<string>>();
  private readonly handlers = new Map<string, (slot: number) => void>();
  /** The theme class currently applied to the root, per slot. Cleared before the next is applied:
   *  classes accumulate, so two themes at once resolve to whichever the stylesheet orders last. */
  private readonly theme = new Map<number, string>();
  /** Which width class each meter currently carries, per slot — needed because applying a new step
   *  requires CLEARING the old one; classes accumulate otherwise and the widest wins. */
  private readonly meterClass = new Map<string, string>();

  constructor(
    private readonly calls: tierb.ResolvedCalls,
    private readonly entity: EntityRef,
    readonly layout: LayoutDescriptor,
  ) {}

  // ── visibility ────────────────────────────────────────────────────────────────────────────────

  /** Reveal a panel for one player, and give them a cursor if it has anything clickable. */
  show(slot: number, panelId: string, withCursor = false): HudResult {
    const err = this.setClass(slot, panelId, this.layout.hideClass, false);
    if (err || !withCursor) return err;
    return setPlayerInputCapture(this.entity, slot, true);
  }

  /** Hide a panel for one player. Does NOT drop input capture — another panel may still want it. */
  hide(slot: number, panelId: string): HudResult {
    return this.setClass(slot, panelId, this.layout.hideClass, true);
  }

  /** Cursor on/off. Players regain movement only once every layout has released capture. */
  cursor(slot: number, on: boolean): HudResult {
    return setPlayerInputCapture(this.entity, slot, on);
  }

  // ── content ───────────────────────────────────────────────────────────────────────────────────

  /**
   * Set text where the layout follows the `id == varName` convention — i.e. a Label with
   * `id="timer_value"` carrying `{s:timer_value}`. That convention removes the mapping layer
   * entirely, so this is the preferred form for layouts that adopt it.
   */
  set(slot: number, id: string, value: string | number): HudResult {
    return tierb.setDialogVariable(this.calls, this.entity, slot, id, id, String(value));
  }

  /** Set a declared text target by its logical name (for layouts with an explicit mapping). */
  setText(slot: number, panelId: string, value: string): HudResult {
    const varName = this.layout.text[panelId];
    if (!varName) return `panel "${panelId}" declares no text variable in this layout`;
    return tierb.setDialogVariable(this.calls, this.entity, slot, panelId, varName, value);
  }

  /** Add or remove a variant class (colour, size, state). */
  setClass(slot: number, panelId: string, className: string, on: boolean): HudResult {
    return tierb.setHasClass(
      this.calls, this.entity, slot, panelId, className,
      on ? HudPanelClassStatus.HasClass : HudPanelClassStatus.DoesNotHaveClass,
    );
  }

  // ── meters ────────────────────────────────────────────────────────────────────────────────────

  /**
   * Set a meter 0..100. Rounded to the nearest 5% step, because that is all the markup can express.
   *
   * The previous step class is cleared first: classes ACCUMULATE, so setting 20 then 80 without
   * clearing leaves both applied and the panel renders at whichever the stylesheet resolves last.
   */
  setMeter(slot: number, meterName: string, percent: number): HudResult {
    const fillId = this.layout.meters[meterName];
    if (!fillId) return `no meter "${meterName}" in this layout`;
    const next = meterClassFor(percent);
    const key = `${slot}:${fillId}`;
    const prev = this.meterClass.get(key);
    if (prev && prev !== next) {
      const err = this.setClass(slot, fillId, prev, false);
      if (err) return err;
    }
    const err = this.setClass(slot, fillId, next, true);
    if (!err) this.meterClass.set(key, next);
    return err;
  }

  // ── pools ─────────────────────────────────────────────────────────────────────────────────────

  /** How many slots a pool has. There is no way to grow this at runtime. */
  capacity(poolName: string): number {
    return this.layout.pools[poolName]?.ids.length ?? 0;
  }

  /**
   * Fill a pool and collapse the unused tail.
   *
   * REFUSES overflow rather than truncating. Silently showing the first N of a longer list is the
   * kind of bug that only surfaces with real data, so the caller is told to paginate instead.
   */
  setPool(slot: number, poolName: string, entries: readonly string[][]): HudResult {
    const pool = this.layout.pools[poolName];
    if (!pool) return `no pool "${poolName}" in this layout`;
    if (entries.length > pool.ids.length) {
      return `pool "${poolName}" holds ${pool.ids.length} slot(s); ${entries.length} given — ` +
        `the maximum is fixed in the layout markup and cannot grow at runtime, so paginate`;
    }
    for (let i = 0; i < pool.ids.length; i++) {
      const panelId = pool.ids[i];
      const row = entries[i];
      if (!row) {
        const err = this.setClass(slot, panelId, this.layout.hideClass, true);
        if (err) return err;
        continue;
      }
      const err = this.setClass(slot, panelId, this.layout.hideClass, false);
      if (err) return err;
      for (let f = 0; f < pool.vars.length && f < row.length; f++) {
        const e = tierb.setDialogVariable(this.calls, this.entity, slot, panelId, pool.vars[f], row[f]);
        if (e) return e;
      }
    }
    return null;
  }

  // ── animated show / hide ──────────────────────────────────────────────────────────────────────
  //
  // `visibility: collapse` CANNOT be transitioned, so a fade needs two phases: drop the collapse,
  // let a frame pass, then clear the fade class (and the reverse to hide). Doing that by hand at
  // every call site is how you end up with half-faded panels.
  //
  // THE GENERATION COUNTER IS THE POINT. Each slot's in-flight animation is tagged; a re-fire bumps
  // the tag and the older timer sees it changed and abandons. Without it, a fast second event on the
  // same slot yanks the first off screen mid-life — the classic kill-feed bug.

  /** Bumped per (slot, panel) on every animated transition; a stale timer compares and gives up. */
  private readonly generation = new Map<string, number>();

  private bump(slot: number, panelId: string): number {
    const key = `${slot}:${panelId}`;
    const next = (this.generation.get(key) ?? 0) + 1;
    this.generation.set(key, next);
    return next;
  }

  private isCurrent(slot: number, panelId: string, gen: number): boolean {
    return this.generation.get(`${slot}:${panelId}`) === gen;
  }

  /**
   * Reveal with a fade. `fadeClass` is the opacity-0 class the layout defines (e.g. `s2-feed-out`).
   * `settle` is the frame the browser needs between uncollapsing and animating; 0.03s is one tick
   * at 64 and matches the reference implementation.
   */
  async showAnimated(
    slot: number, panelId: string, fadeClass: string, settle = 0.03,
  ): Promise<HudResult> {
    const gen = this.bump(slot, panelId);
    let err = this.setClass(slot, panelId, fadeClass, true);
    if (err) return err;
    err = this.setClass(slot, panelId, this.layout.hideClass, false);
    if (err) return err;
    await delay(settle);
    if (!this.isCurrent(slot, panelId, gen)) return null;   // superseded — leave it to the newer call
    return this.setClass(slot, panelId, fadeClass, false);
  }

  /** Fade out, then collapse once the transition has run. `hold` must exceed the CSS duration. */
  async hideAnimated(
    slot: number, panelId: string, fadeClass: string, hold = 0.25,
  ): Promise<HudResult> {
    const gen = this.bump(slot, panelId);
    const err = this.setClass(slot, panelId, fadeClass, true);
    if (err) return err;
    await delay(hold);
    if (!this.isCurrent(slot, panelId, gen)) return null;
    return this.setClass(slot, panelId, this.layout.hideClass, true);
  }

  /**
   * Show, then auto-dismiss after `seconds`. The generation guard means a re-fire of the same slot
   * restarts the life rather than letting the first timer close the second message.
   */
  async flash(
    slot: number, panelId: string, fadeClass: string, seconds: number,
  ): Promise<HudResult> {
    const err = await this.showAnimated(slot, panelId, fadeClass);
    if (err) return err;
    const gen = this.generation.get(`${slot}:${panelId}`) ?? 0;
    await delay(seconds);
    if (!this.isCurrent(slot, panelId, gen)) return null;
    return this.hideAnimated(slot, panelId, fadeClass);
  }

  // ── theming ───────────────────────────────────────────────────────────────────────────────────

  /**
   * Switch the whole layout's palette for ONE player.
   *
   * `rootPanelId` must be the FIRST CHILD, not the document root: a root `<Panel>` may not carry an
   * `id` at all (confirmed — it is a hard resource-compile error), so the themed wrapper is the
   * outermost element that can have one.
   *
   * Viable because descendant selectors are confirmed working in-game — a class on the root
   * cascades to every component under it — and because SetHasClassForPlayer is per-player, so two
   * people can see different themes of the same layout simultaneously.
   *
   * The palettes themselves must be pre-baked in the stylesheet (there are no runtime CSS variables:
   * `var()` and `calc()` do not exist in Panorama; `@define` is compile-time only). So this SELECTS
   * from a finite set rather than composing one.
   */
  setTheme(slot: number, rootPanelId: string, themeClass: string | null): HudResult {
    const prev = this.theme.get(slot);
    if (prev && prev !== themeClass) {
      const err = this.setClass(slot, rootPanelId, prev, false);
      if (err) return err;
    }
    if (themeClass === null) { this.theme.delete(slot); return null; }
    const err = this.setClass(slot, rootPanelId, themeClass, true);
    if (!err) this.theme.set(slot, themeClass);
    return err;
  }

  // ── clicks ────────────────────────────────────────────────────────────────────────────────────

  /** Register a handler for one button id. Replaces any previous handler for that id. */
  onClick(buttonId: string, handler: (slot: number) => void): void {
    if (!this.layout.buttons.includes(buttonId)) {
      console.log(`[hudlib] WARN: "${buttonId}" is not a declared button in this layout — ` +
        "the handler will never fire. Check the id against the markup.");
    }
    this.handlers.set(buttonId, handler);
  }

  /** Grey a button AND stop its clicks. The class alone is cosmetic; the click still arrives. */
  setDisabled(slot: number, buttonId: string, off: boolean): HudResult {
    let set = this.disabled.get(slot);
    if (!set) { set = new Set(); this.disabled.set(slot, set); }
    if (off) set.add(buttonId); else set.delete(buttonId);
    return this.setClass(slot, buttonId, "s2-btn-disabled", off);
  }

  /** Route a click from the engine hook. Returns true if a handler ran. */
  dispatchClick(slot: number, buttonId: string): boolean {
    if (this.disabled.get(slot)?.has(buttonId)) return false;   // visual-only disable, enforced here
    const h = this.handlers.get(buttonId);
    if (!h) return false;
    h(slot);
    return true;
  }

  /** Forget a disconnected player's state so the maps do not grow without bound. */
  forget(slot: number): void {
    this.disabled.delete(slot);
    this.theme.delete(slot);
    for (const key of [...this.generation.keys()]) {
      if (key.startsWith(`${slot}:`)) this.generation.delete(key);
    }
    for (const key of [...this.meterClass.keys()]) {
      if (key.startsWith(`${slot}:`)) this.meterClass.delete(key);
    }
  }
}
