import type { Recipe } from "../recipe.ts";
import type { UserCmdView } from "@s2script/sdk/usercmd";
import { Pawn } from "@s2script/cs2";
import { HookResult } from "@s2script/sdk/events";

type Mode = "off" | "jump" | "side" | "block";
const IN_JUMP = 2n; // buttons is a bigint

/**
 * ctx.clients.onRunCmd reads a player's live per-tick input (7 fields) before
 * the engine processes it, and can modify or block it by returning a
 * HookResult. Gated behind cb_usercmd so it doesn't fight normal movement
 * until a player opts in:
 *
 *   cb_usercmd off    — read only (default)
 *   cb_usercmd jump   — force forwardMove=0 + IN_JUMP (modify proof)
 *   cb_usercmd side   — zero forwardMove, route it to sideMove (the sideways surf style — proves sign+effect)
 *   cb_usercmd block  — return HookResult.Handled (neutralize the whole input)
 */
export const usercmdRecipe: Recipe = {
  name: "usercmd",
  describe: "read/modify/block a player's per-tick input (cb_usercmd off|jump|side|block)",
  register(ctx) {
    const modeBySlot = new Map<number, Mode>();
    let logN = 0;

    ctx.clients.onRunCmd((cmd: UserCmdView, info: { slot: number }) => {
      const slot = info.slot;
      // Read proof (throttled ~1/64 cmds): all 7 fields + cross-check buttons against the SCHEMA source
      // (pawn.buttons = m_pButtonStates[0], a different read path) and the decoded slot vs the pawn's.
      if ((logN++ & 0x3f) === 0) {
        const schemaBtn = Pawn.forSlot(slot)?.buttons ?? -1;
        const va = cmd.viewAngles;
        console.log(
          `[cookbook] usercmd slot=${slot} fwd=${cmd.forwardMove} side=${cmd.sideMove} up=${cmd.upMove}` +
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

    ctx.commands.register("cb_usercmd", (cmd) => {
      if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return; }
      const arg = cmd.argsFrom(0).trim();
      const mode: Mode = arg === "jump" || arg === "side" || arg === "block" ? arg : "off";
      modeBySlot.set(cmd.callerSlot, mode);
      cmd.reply(`usercmd mode = ${mode}`);
    });
  },
};
