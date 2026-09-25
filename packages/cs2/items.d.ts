/**
 * Pickup gates — first-class CanAcquire (PR1). Hand-written: the view is not the generic
 * number-params shape hookgen emits. See docs/superpowers/specs/2026-08-14-pickup-gates-design.md.
 */
import type { HookResultValue } from "@s2script/sdk/events";
import type { Player } from "./index.d.ts";

/** How the engine is trying to grant the item. Numeric values match the engine enum. */
export enum AcquireMethod {
  PickUp = 0,
  Buy = 1,
}

/** Engine return of CanAcquire. `Allowed` is 0; every other code is a deny. */
export enum AcquireResult {
  Allowed = 0,
  InvalidItem = 1,
  AlreadyOwned = 2,
  AlreadyPurchased = 3,
  AlreadyRedeemed = 4,
  NotAllowedByLimit = 5,
  NotAllowedByTeam = 6,
  NotAllowedByProhibited = 7,
}

/**
 * Block-scoped view of one CanAcquire. Valid only during the handler.
 * Across an await every read is stale; do not stash this object.
 */
export interface CanAcquireView {
  /** Owning player, or null when the ItemServices→pawn hop missed. The hook still fires. */
  readonly player: Player | null;
  /** `CEconItemView.m_iItemDefinitionIndex`. */
  readonly defIndex: number;
  readonly method: AcquireMethod;
  /** Writable on Pre. Seed `Allowed`. Readonly on Post. */
  result: AcquireResult;
  /** Post only: true when Pre skipped the original. Always false on Pre. */
  readonly skipped: boolean;
}

/**
 * Pickup gates. Register during the load window (module top level or `OnPluginStart`); a call
 * after the plugin settles throws. Subscriptions are owned by the calling plugin and torn down
 * with it.
 */
export interface ItemsApi {
  /**
   * Vote on a CanAcquire. Return `Changed` to vote the current `result`, `Handled`/`Stop` to skip
   * the original and vote the result this handler wrote (or `InvalidItem` when it wrote none).
   * Handled/Stop votes outrank Changed votes; any deny beats `Allowed`; the first deny wins.
   *
   * A plugin's gate sees acquisitions the plugin causes elsewhere (a command's `giveNamedItem`).
   * An acquisition triggered from inside the plugin's own gate callback skips only that plugin's
   * gate for the nested call; other plugins' gates still vote on it.
   */
  onCanAcquire(handler: (view: CanAcquireView) => HookResultValue | void): void;
  /** Observe the effective result after the original ran (or was skipped). */
  onCanAcquirePost(handler: (view: CanAcquireView) => void): void;
}

/** @deprecated Use {@link ItemsApi}. There is no `ctx.items`; import `items` instead. */
export type CtxItems = ItemsApi;

/** Load-window pickup gates: `import { items } from "@s2script/cs2"`. Throws after settle. */
export declare const items: ItemsApi;
