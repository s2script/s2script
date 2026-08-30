import type { Recipe } from "../recipe.ts";
import { Voice } from "@s2script/sdk/voice";
import { Player } from "@s2script/cs2";
import { command } from "@s2script/sdk/commands";

/** Highest slot the hearability mask can address (the receiver set is a uint64). */
const MAX_SLOT = 63;

/**
 * Voice hearability — who can hear whom, per (receiver, sender) pair.
 *
 * The rule is declarative: you set it, the shim enforces it on the engine's listen matrix. There is
 * deliberately no per-pair callback, because that path runs up to 64x64 times per voice refresh.
 *
 *   sm_voice_only <sender> <receiver>   sender is audible ONLY to receiver — the receiver dimension
 *   sm_voice_solo <slot>                everyone except <slot> is silenced outright
 *   sm_voice_reset                      drop this plugin's rules
 *   sm_voice_stats                      hot-path counters (rewrites = the effect)
 *
 * `sm_voice_only` is the one that actually demonstrates this capability. `sm_voice_solo` uses an
 * EMPTY receiver list, which is behaviourally just the old per-sender mute — if that were the only
 * command here, a shim that ignored the receiver bit entirely would still look correct.
 */
export const voiceRecipe: Recipe = {
  name: "voice",
  describe: "per-pair voice hearability (sm_voice_only / _solo / _reset / _stats)",
  register() {
    command("sm_voice_only", (cmd) => {
      const sender = cmd.argInt(0, -1);
      const receiver = cmd.argInt(1, -1);
      if (sender < 0 || sender > MAX_SLOT || receiver < 0 || receiver > MAX_SLOT) {
        cmd.reply(`[cookbook] usage: sm_voice_only <sender 0-${MAX_SLOT}> <receiver 0-${MAX_SLOT}>`);
        return;
      }
      const ok = Voice.setAudibleTo(sender, [receiver]);
      cmd.reply(
        ok
          ? `[cookbook] voice: slot ${sender} is now audible ONLY to slot ${receiver}`
          : `[cookbook] voice: rule rejected (voice control degraded, or a slot out of range)`
      );
    });

    command("sm_voice_solo", (cmd) => {
      const keep = cmd.argInt(0, -1);
      if (keep < 0 || keep > MAX_SLOT) {
        cmd.reply(`[cookbook] usage: sm_voice_solo <slot 0-${MAX_SLOT}>`);
        return;
      }
      // Everyone except `keep` becomes audible to nobody. `keep` gets its rule dropped so the
      // engine decides for them again.
      let silenced = 0;
      for (const p of Player.allConnected()) {
        if (p.slot === keep) { Voice.reset(p.slot); continue; }
        if (Voice.setAudibleTo(p.slot, [])) silenced++;
      }
      cmd.reply(`[cookbook] voice: silenced ${silenced} sender(s); slot ${keep} still audible`);
    });

    command("sm_voice_reset", (cmd) => {
      Voice.resetAll();
      cmd.reply("[cookbook] voice: dropped this plugin's hearability rules");
    });

    command("sm_voice_stats", (cmd) => {
      const s = Voice.stats();
      if (!s) { cmd.reply("[cookbook] voice: stats unavailable (shim predates this capability)"); return; }
      // `rewrites` counts hearability denials ONLY — a basecomm gag does not move it, so this stays a
      // clean signal that YOUR rules are the thing taking effect.
      cmd.reply(
        `[cookbook] voice: calls=${s.calls} entries=${s.entries} rewrites=${s.rewrites}` +
          (s.rewrites > 0 ? " (rules ARE taking effect)" : " (no rewrites yet — nobody has spoken)")
      );
    });
  },
};
