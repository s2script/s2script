import { plugin } from "@s2script/sdk/plugin";
import { Entity } from "@s2script/sdk/entity";
import { GameRules, Fade } from "@s2script/cs2";

export default plugin((ctx) => {
  ctx.commands.register("sm_gamerules", (cmd) => {
    const gr = GameRules.get();
    const proxies = Entity.findByClass("cs_gamerules").length;
    if (!gr) { cmd.reply(`[gr] no cs_gamerules proxy (findByClass=${proxies})`); return; }
    cmd.reply(`[gr] warmup=${gr.warmupPeriod} freeze=${gr.freezePeriod} roundTime=${gr.roundTime} ` +
              `rounds=${gr.totalRoundsPlayed} phase=${gr.gamePhase} proxies=${proxies}`);
  });

  ctx.commands.register("sm_umsg", (cmd) => {
    // sm_umsg <slot> — the slot is the FIRST arg (cmd.args excludes the command name; no target token here).
    const slot = cmd.args.length > 0 ? parseInt(cmd.args[0], 10) : (cmd.callerSlot >= 0 ? cmd.callerSlot : 0);
    const ok = Fade.blind(slot, 1500);
    cmd.reply(`[umsg] Fade.blind(slot=${slot}) -> ${ok}`);
  });

  console.log("[gamerules-usermsg-demo] onLoad — sm_gamerules/sm_umsg registered");
});
