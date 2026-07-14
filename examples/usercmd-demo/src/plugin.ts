// @demo/usercmd-demo — the @s2script/usercmd live gate (Task 5). Reads a player's live per-tick input
// (all 7 fields) through UserCmd.onRun, and a chat-command-gated modify/block so a human can prove the
// write direction without fighting normal movement:
//
//   !ucmode off    — read only (default)
//   !ucmode jump   — force forwardMove=0 + IN_JUMP (modify proof)
//   !ucmode side   — zero forwardMove, route it to sideMove (the sideways surf style — proves sign+effect)
//   !ucmode block  — return HookResult.Handled (neutralize the whole input)

import { UserCmd } from "@s2script/usercmd";
import { Commands } from "@s2script/commands";
import { Pawn } from "@s2script/cs2";
import { HookResult } from "@s2script/events";

type Mode = "off" | "jump" | "side" | "block";
const modeBySlot = new Map<number, Mode>();
const IN_JUMP = 2n; // buttons is a bigint
let logN = 0;

export function onLoad(): void {
  UserCmd.onRun((cmd, ctx) => {
    const slot = ctx.slot;
    // Read proof (throttled ~1/64 cmds): all 7 fields + cross-check buttons against the SCHEMA source
    // (pawn.buttons = m_pButtonStates[0], a different read path) and the decoded slot vs the pawn's.
    if ((logN++ & 0x3f) === 0) {
      const schemaBtn = Pawn.forSlot(slot)?.buttons ?? -1;
      const va = cmd.viewAngles;
      console.log(
        `[usercmd-demo] slot=${slot} fwd=${cmd.forwardMove} side=${cmd.sideMove} up=${cmd.upMove}` +
          ` imp=${cmd.impulse} btn=${cmd.buttons} schemaBtn=${schemaBtn}` +
          ` pitch=${va.x.toFixed(0)} yaw=${va.y.toFixed(0)}`,
      );
    }
    // Modify / block (command-gated so it doesn't fight normal play until the player opts in).
    const mode = modeBySlot.get(slot) ?? "off";
    if (mode === "jump") {
      cmd.forwardMove = 0;
      cmd.buttons = cmd.buttons | IN_JUMP;
    } else if (mode === "side") {
      const fwd = cmd.forwardMove;
      cmd.forwardMove = 0;
      cmd.sideMove = fwd; // route forward input to strafe = the "sideways" style
    } else if (mode === "block") {
      return HookResult.Handled; // engine processes a neutralized command
    }
    return HookResult.Continue;
  });

  Commands.register("ucmode", (ctx) => {
    if (ctx.callerSlot < 0) { ctx.reply("run in-game"); return; }
    const arg = ctx.argsFrom(0).trim();
    const mode: Mode = arg === "jump" || arg === "side" || arg === "block" ? arg : "off";
    modeBySlot.set(ctx.callerSlot, mode);
    ctx.reply(`usercmd mode = ${mode}`);
  });

  console.log("[usercmd-demo] onLoad — UserCmd.onRun armed; !ucmode <off|jump|side|block>");
}

export function onUnload(): void {}
