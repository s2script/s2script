/**
 * Movement hooks — each player's movement tick (CCSPlayer_MovementServices::ProcessMove). Hand-written:
 * the view is not the generic number-params shape hookgen emits.
 */
import type { QAngle, Vector } from "@s2script/sdk/math";
import type { Player } from "./index.d.ts";

/**
 * Block-scoped view of one player's movement tick (the engine's CMoveData). Valid only during the
 * handler: do not stash it or read it after an `await`. Each read returns a fresh copy.
 */
export interface MoveView {
  /** The player moving, or null when their movement services matched no live pawn. */
  readonly player: Player | null;
  /** The view angles this tick's movement uses. */
  readonly viewAngles: QAngle;
  /** Forward input this tick (positive forward, negative back). */
  readonly forwardMove: number;
  /** Side input this tick (positive left, negative right). */
  readonly sideMove: number;
  /** Where the player is: before the move in `onProcessMove`, after it in `onProcessMovePost`. */
  readonly origin: Vector;
  /**
   * The player's velocity. In `onProcessMove` you can assign it (a `Vector` or `[x, y, z]`), and the
   * engine moves the player with it this tick; friction, gravity and acceleration still apply. In
   * `onProcessMovePost` it is the velocity after the move, and an assignment is ignored.
   */
  velocity: Vector;
}

/**
 * Movement hooks. Register during the load window (module top level or `OnPluginStart`); a call after
 * the plugin settles throws. Subscriptions are owned by the calling plugin and torn down with it.
 *
 * The handler runs for every player on every movement tick (64 times a second each), so keep it
 * cheap. Its return value is ignored: movement always runs.
 */
export interface MovementApi {
  /**
   * Before a player's movement tick.
   * @example
   * import { movement } from "@s2script/cs2";
   * // Slow every fall: damp the downward speed each tick.
   * movement.onProcessMove(v => {
   *   const vel = v.velocity;
   *   if (vel.z < 0) v.velocity = [vel.x, vel.y, vel.z * 0.9];
   * });
   */
  onProcessMove(handler: (view: MoveView) => void): void;
  /** After a player's movement tick, with the resulting origin and velocity. */
  onProcessMovePost(handler: (view: MoveView) => void): void;
}

/** Load-window movement hooks: `import { movement } from "@s2script/cs2"`. Throws after settle. */
export declare const movement: MovementApi;
