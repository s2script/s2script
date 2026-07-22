/**
 * @s2script/usercmd — a SourceMod `OnPlayerRunCmd`-equivalent: intercept, read, modify, and block a
 * player's per-tick input (buttons, view angles, movement, impulse) before the game processes it.
 * NO runtime code (injected at load) — `CBaseUserCmdPB`/`CMsgQAngle`/`CInButtonStatePB` are a
 * Source2-shared concept (`usercmd.proto`), so this module is engine-generic.
 */
import type { QAngle } from "./math";

/**
 * A block-scoped view of the CURRENT tick's usercmd (valid only during a `ctx.clients.onRunCmd`
 * handler — a stashed `UserCmdView` used after the handler returns, or across an `await`,
 * reads/writes nothing). There is exactly ONE `UserCmdView` instance for the whole process; every
 * handler call operates on it.
 * @example
 * import type { UserCmdView } from "@s2script/sdk/usercmd";
 * // examples/usercmd-demo/src/plugin.ts:21 — read this tick's input
 * ctx.clients.onRunCmd((cmd: UserCmdView, info: { slot: number }) => {
 *   console.log(`slot=${info.slot} fwd=${cmd.forwardMove} btn=${cmd.buttons}`);
 * });
 */
export interface UserCmdView {
  /** +forward / -back. Normalized to roughly [-1, 1] (not the legacy ±450 units). */
  forwardMove: number;
  /**
   * +right / -left (SourceMod convention). Assigning MODIFIES the live input.
   * Note: internally negated from the raw protobuf `leftmove` (which is +LEFT) — this field is
   * already sign-corrected, so `sideMove > 0` always means "moving right".
   */
  sideMove: number;
  /** +up / -down (e.g. crouch-jump analog). */
  upMove: number;
  /** The pressed impulse (e.g. 100 = +usering). */
  impulse: number;
  /** The pressed-button mask (`IN_*` bit values, mirrors `pawn.buttons`). 64-bit — always a real `bigint`. */
  buttons: bigint;
  /** View angles for this tick ({x: pitch, y: yaw, z: roll}). Assigning writes all three fields. */
  viewAngles: QAngle;
  /**
   * Drop this tick's subtick analog-move deltas (optional helper). A coarse `forwardMove`/`sideMove`/
   * `upMove` write already takes effect without this (live-verified) — call it only if you need to
   * make sure no residual subtick analog data survives your modify. No-op if there are none.
   */
  clearSubtickMoves(): void;
}
