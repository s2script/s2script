/**
 * @s2script/usercmd — a SourceMod `OnPlayerRunCmd`-equivalent: intercept, read, modify, and block a
 * player's per-tick input (buttons, view angles, movement, impulse) before the game processes it,
 * and observe the result after it has.
 * NO runtime code (injected at load) — `CBaseUserCmdPB`/`CMsgQAngle`/`CInButtonStatePB` are a
 * Source2-shared concept (`usercmd.proto`), so this module is engine-generic.
 *
 * ## `"pre"` vs `"post"`
 *
 * `ctx.clients.onRunCmd(handler, { phase })` (or the `OnPlayerRunCmd` / `OnPlayerRunCmdPost` exports)
 * runs `handler` once per usercmd the server runs for a player, at one of two points:
 *
 * | | `"pre"` (default, `OnPlayerRunCmd`) | `"post"` (`OnPlayerRunCmdPost`) |
 * |---|---|---|
 * | When | the command arrives, before any of it runs | after THIS command ran: movement applied and written back to the pawn |
 * | `cmd` | writable: change the input | read-only: writes are ignored (debug builds warn once) |
 * | Return | a `HookResult`; `Handled`/`Stop` neutralize the command | ignored |
 * | `info` | `{ slot }` | {@link UserCmdPostInfo} `{ slot, tick, commandNumber }` |
 * | Pawn reads | the state before the command | the state after it (origin, eye angles, velocity, flags, duck amount, active weapon) |
 *
 * **0..n commands per tick.** A tick can run zero, one or several commands for a player: a lost
 * packet means none that tick, and the client's catch-up then delivers several in the next one. Each
 * phase fires once PER COMMAND, never once per tick, so a replay recorder sampling in `"post"` gets
 * one frame per command the server actually simulated — compare `info.tick` across calls to see two
 * commands land in one tick, or a tick with none. In `"pre"`, every command of a batch is seen
 * before any of them runs, so pawn state there is the same for all of them.
 *
 * Bots and fake clients fire too; filter them yourself (`client.isBot`).
 *
 * Lifetime is the same in both phases: `cmd` (and `info`) are valid only synchronously inside the
 * handler. Both are reused objects, so copy what you need instead of keeping them.
 *
 * **Cost.** In `"post"` the host copies each command (and the movement-services buttons) once, so
 * every `cmd` field and `info` read is a plain memory read: read as many as you like. In `"pre"`
 * each field read still goes to the engine.
 */
import type { QAngle } from "./math";

/** The `info` argument of a `"post"` handler. One object is reused across calls: copy fields out. */
export interface UserCmdPostInfo {
  /** The player's slot. */
  readonly slot: number;
  /** The server tick this command ran in (several commands can share one; see the module docs). */
  readonly tick: number;
  /** The usercmd's own command number (the client's running counter). */
  readonly commandNumber: number;
}

/**
 * A block-scoped view of the CURRENT usercmd (valid only during an `OnPlayerRunCmd` /
 * `OnPlayerRunCmdPost` handler — a stashed `UserCmdView` used after the handler returns, or across
 * an `await`, reads/writes nothing). There is exactly ONE `UserCmdView` instance for the whole
 * process; every handler call operates on it. In `"post"` it is read-only: assignments are ignored.
 * @example
 * import type { UserCmdView } from "@s2script/sdk/usercmd";
 * // read this tick's input
 * export function OnPlayerRunCmd(cmd: UserCmdView, info: { slot: number }): void {
 *   console.log(`slot=${info.slot} fwd=${cmd.forwardMove} btn=${cmd.buttons}`);
 * }
 * @example
 * import type { UserCmdPostInfo, UserCmdView } from "@s2script/sdk/usercmd";
 * import { Pawn } from "@s2script/cs2";
 * // record one replay frame per executed command, after its movement
 * const frame = new Float64Array(Pawn.KINEMATICS_SLOTS);
 * export function OnPlayerRunCmdPost(cmd: UserCmdView, info: UserCmdPostInfo): void {
 *   const pawn = Pawn.forSlot(info.slot);
 *   if (pawn && pawn.readKinematics(frame)) record(info.slot, info.tick, frame, cmd.pressedButtons);
 * }
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
  /**
   * The pawn's movement-services "held" button state (`IN_*` bits). Read-only.
   * In `"post"`: the state THIS command left. In `"pre"`: the state the PREVIOUS command left.
   * Unlike {@link buttons} (what the client sent), this is what the server applied.
   */
  readonly pressedButtons: bigint;
  /** Buttons whose held state changed on the command that last ran. Read-only. */
  readonly changedButtons: bigint;
  /** Scroll-wheel "buttons" of the command that last ran. Read-only. */
  readonly scrollButtons: bigint;
  /** View angles for this tick ({x: pitch, y: yaw, z: roll}). Assigning writes all three fields. */
  viewAngles: QAngle;
  /**
   * Drop this tick's subtick analog-move deltas (optional helper). A coarse `forwardMove`/`sideMove`/
   * `upMove` write already takes effect without this (live-verified) — call it only if you need to
   * make sure no residual subtick analog data survives your modify. No-op if there are none.
   */
  clearSubtickMoves(): void;
}
