import { Commands } from "@s2script/commands";
import { Entity } from "@s2script/entity";
import { GameRules, Fade } from "@s2script/cs2";

export function onLoad(): void {
  Commands.register("sm_gamerules", (ctx) => {
    const gr = GameRules.get();
    const proxies = Entity.findByClass("cs_gamerules").length;
    if (!gr) { ctx.reply(`[gr] no cs_gamerules proxy (findByClass=${proxies})`); return; }
    ctx.reply(`[gr] warmup=${gr.warmupPeriod} freeze=${gr.freezePeriod} roundTime=${gr.roundTime} ` +
              `rounds=${gr.totalRoundsPlayed} phase=${gr.gamePhase} proxies=${proxies}`);
  });

  Commands.register("sm_umsg", (ctx) => {
    // sm_umsg <slot> — the slot is the FIRST arg (ctx.args excludes the command name; no target token here).
    const slot = ctx.args.length > 0 ? parseInt(ctx.args[0], 10) : (ctx.callerSlot >= 0 ? ctx.callerSlot : 0);
    const ok = Fade.blind(slot, 1500);
    ctx.reply(`[umsg] Fade.blind(slot=${slot}) -> ${ok}`);
  });

  console.log("[gamerules-usermsg-demo] onLoad — sm_gamerules/sm_umsg registered");
}
