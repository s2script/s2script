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

export interface CtxItems {
  onCanAcquire(handler: (view: CanAcquireView) => HookResultValue | void): void;
  onCanAcquirePost(handler: (view: CanAcquireView) => void): void;
}

declare module "@s2script/sdk/plugin" {
  interface PluginContext {
    readonly items: CtxItems;
  }
}

/** Load-window pickup gates. Same object as the former `ctx.items`. Throws after settle. */
export declare const items: CtxItems;
