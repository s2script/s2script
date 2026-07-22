import type { Recipe } from "../recipe.ts";
import { Entity } from "@s2script/sdk/entity";
import { GameRules, Fade } from "@s2script/cs2";

/**
 * GameRules.get() re-finds the live cs_gamerules proxy each call and returns
 * a liveness-gated view (round/warmup timing, phase). Fade is a CS2
 * usermessage-backed screen effect — Fade.blind flashes a client's screen
 * white for a duration.
 */
export const gamerulesRecipe: Recipe = {
  name: "gamerules",
  describe: "read live gamerules state and fire a Fade usermessage (cb_gamerules / cb_gamerules_blind)",
  register(ctx) {
    ctx.commands.register("cb_gamerules", (cmd) => {
      const gr = GameRules.get();
      const proxies = Entity.findByClass("cs_gamerules").length;
      if (!gr) { cmd.reply(`[cookbook] gamerules: no cs_gamerules proxy (findByClass=${proxies})`); return; }
      cmd.reply(`[cookbook] gamerules: warmup=${gr.warmupPeriod} freeze=${gr.freezePeriod} roundTime=${gr.roundTime} ` +
                `rounds=${gr.totalRoundsPlayed} phase=${gr.gamePhase} proxies=${proxies}`);
    });

    ctx.commands.register("cb_gamerules_blind", (cmd) => {
      // the slot is the FIRST arg (cmd.args excludes the command name; no target token here).
      const slot = cmd.args.length > 0 ? parseInt(cmd.args[0], 10) : (cmd.callerSlot >= 0 ? cmd.callerSlot : 0);
      const ok = Fade.blind(slot, 1500);
      cmd.reply(`[cookbook] gamerules: Fade.blind(slot=${slot}) -> ${ok}`);
    });
  },
};
