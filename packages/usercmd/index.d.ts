/**
 * @s2script/usercmd — a SourceMod `OnPlayerRunCmd`-equivalent: intercept, read, modify, and block a
 * player's per-tick input (buttons, view angles, movement, impulse) before the game processes it.
 * NO runtime code (injected at load) — `CBaseUserCmdPB`/`CMsgQAngle`/`CInButtonStatePB` are a
 * Source2-shared concept (`usercmd.proto`), so this module is engine-generic.
 */
import type { QAngle } from "@s2script/math";
import type { HookResultValue } from "@s2script/events";

/**
 * A block-scoped view of the CURRENT tick's usercmd (valid only during a `UserCmd.onRun` handler —
 * a stashed `Cmd` used after the handler returns, or across an `await`, reads/writes nothing).
 * There is exactly ONE `Cmd` instance for the whole process; every handler call operates on it.
 */
export interface Cmd {
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

export declare const UserCmd: {
  /**
   * Subscribe to the per-tick input hook. The handler runs SYNCHRONOUSLY during the engine's
   * usercmd processing, once per player per batched tick; `cmd` is the singleton, block-scoped
   * current usercmd and `ctx.slot` is the firing player's 0-based slot.
   *
   * Return a `HookResultValue >= Handled` to SUPPRESS this tick's input (the game processes a
   * zeroed/idle command instead); return `Continue`/`undefined` to let the (possibly modified)
   * command through unblocked.
   */
  onRun(handler: (cmd: Cmd, ctx: { slot: number }) => HookResultValue | void): void;
};
