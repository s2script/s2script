import type { Recipe } from "../recipe.ts";
import { Voice } from "@s2script/sdk/voice";
import { Player } from "@s2script/cs2";

/**
 * Voice hearability — who can hear whom, per (receiver, sender) pair.
 *
 * The rule is declarative: you set it, the shim enforces it on the engine's listen matrix. There is
 * deliberately no per-pair callback, because that path runs up to 64x64 times per voice refresh.
 *
 *   cb_voice_solo <slot>   only that slot is audible; everyone else is silenced
 *   cb_voice_reset         drop this plugin's rules
 *   cb_voice_stats         show the hot-path counters (rewrites = the effect)
 */
export const voiceRecipe: Recipe = {
  name: "voice",
  describe: "per-pair voice hearability (cb_voice_solo / _reset / _stats)",
  register(ctx) {
    ctx.commands.register("cb_voice_solo", (cmd) => {
      const keep = cmd.argInt(0, -1);
      if (keep < 0) { cmd.reply("[cookbook] usage: cb_voice_solo <slot>"); return; }
      // Everyone except `keep` becomes audible to nobody. `keep` gets its rule dropped so the
      // engine decides for them again.
      let silenced = 0;
      for (const p of Player.allConnected()) {
        if (p.slot === keep) { Voice.reset(p.slot); continue; }
        if (Voice.setAudibleTo(p.slot, [])) silenced++;
      }
      cmd.reply(`[cookbook] voice: silenced ${silenced} sender(s); slot ${keep} still audible`);
    });

    ctx.commands.register("cb_voice_reset", (cmd) => {
      Voice.resetAll();
      cmd.reply("[cookbook] voice: dropped this plugin's hearability rules");
    });

    ctx.commands.register("cb_voice_stats", (cmd) => {
      const s = Voice.stats();
      if (!s) { cmd.reply("[cookbook] voice: stats unavailable (shim predates this capability)"); return; }
      cmd.reply(
        `[cookbook] voice: calls=${s.calls} entries=${s.entries} rewrites=${s.rewrites}` +
          (s.rewrites > 0 ? " (rules ARE taking effect)" : " (no rewrites yet — nobody has spoken)")
      );
    });
  },
};
