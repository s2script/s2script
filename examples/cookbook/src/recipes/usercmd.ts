import type { Recipe } from "../recipe.ts";
import type { UserCmdView } from "@s2script/sdk/usercmd";
import { Pawn } from "@s2script/cs2";
import { HookResult } from "@s2script/sdk/events";

type Mode = "off" | "jump" | "side" | "block";
const IN_JUMP = 2n; // buttons is a bigint

/**
 * ctx.clients.onRunCmd reads a player's live per-tick input (7 fields) before
 * the engine processes it, and can modify or block it by returning a
 * HookResult. Gated behind sm_usercmd so it doesn't fight normal movement
 * until a player opts in:
 *
 *   sm_usercmd off      — read only (default)
 *   sm_usercmd jump     — force forwardMove=0 + IN_JUMP (modify proof)
 *   sm_usercmd side     — zero forwardMove, route it to sideMove (the sideways surf style — proves sign+effect)
 *   sm_usercmd block    — return HookResult.Handled (neutralize the whole input)
 *   sm_usercmd verbose  — toggle the read-proof log line below (off by default — this hook fires
 *                         every tick for every player, so even throttled to 1-in-64 it's ~1
 *                         line/sec/player; loading the cookbook must not spam the console on its
 *                         own, same reasoning as recipes/damage.ts defaulting its effect off)
 */
export const usercmdRecipe: Recipe = {
  name: "usercmd",
  describe: "read/modify/block a player's per-tick input (sm_usercmd off|jump|side|block|verbose)",
  register(ctx) {
    const modeBySlot = new Map<number, Mode>();
    let logN = 0;
    let verbose = false;

    ctx.clients.onRunCmd((cmd: UserCmdView, info: { slot: number }) => {
      const slot = info.slot;
      // Read proof (throttled ~1/64 cmds): all 7 fields + cross-check buttons against the SCHEMA source
      // (pawn.buttons = m_pButtonStates[0], a different read path) and the decoded slot vs the pawn's.
      // Opt-in via sm_usercmd verbose — see the toggle note above.
      if (verbose && (logN++ & 0x3f) === 0) {
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

    ctx.commands.register("sm_usercmd", (cmd) => {
      const arg = cmd.argsFrom(0).trim();
      if (arg === "verbose") {
        verbose = !verbose;
        cmd.reply(`usercmd verbose logging = ${verbose ? "on" : "off"}`);
        return;
      }
      if (cmd.callerSlot < 0) { cmd.reply("run in-game"); return; }
      const mode: Mode = arg === "jump" || arg === "side" || arg === "block" ? arg : "off";
      modeBySlot.set(cmd.callerSlot, mode);
      cmd.reply(`usercmd mode = ${mode}`);
    });
  },
};
