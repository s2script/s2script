// a5bgate-b — live-gate fixture B for A5b. NOT a shipped plugin.
//
// The SECOND plugin context. Two jobs:
//   1. G4/G5 — act on the same target in the same frame as a, so the cross-plugin dedupe question
//      is answered by counting, not by reasoning. The shim's s_pendingRespawn/s_pendingTerminate
//      were host-global statics; pawn.js is evaluated per plugin context, so the dedupe moved with
//      it. 1 dispatch means something still collapses them; 2 means it is genuinely gone.
//   2. G6 — hold an Events.onPre subscriber for the events these calls fire. The review confirmed
//      pre-hooks are NOT deferrable, so this handler is expected NOT to run. Recording it is the
//      point: it is an accepted, permanent regression, not a surprise.
import { plugin } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
import { Player, GameRules } from "@s2script/cs2";

export default plugin((ctx) => {
  const L = (m: string) => console.log(`[A5B-B] ${m}`);
  L("loaded");

  let frame = 0;
  ctx.server.onGameFrame(() => { frame += 1; });

  // The coordination channel: a fires this, b acts on the same target in the same dispatch.
  ctx.events.on("player_changename", (e) => {
    const tag = e.getString("oldname");
    if (tag === "a5b-dupe-respawn") {
      const slot = Number(e.getString("newname"));
      const p = Player.all().find((x) => x.slot === slot);
      L(`dupe_respawn: b's call on slot=${slot} -> ${p ? p.respawn() : "no player"} frame=${frame}`);
    } else if (tag === "a5b-dupe-round") {
      L(`dupe_round: b's call -> ${GameRules.terminateRound(8)} frame=${frame}`);
    }
  });

  // The counters that answer G4/G5. Post-dispatch (Events.on) IS delivered — deferred one frame by
  // the deferred-dispatch queue when the call re-enters.
  let spawns = 0, roundEnds = 0;
  ctx.events.on("player_spawn", () => { spawns += 1; L(`player_spawn #${spawns} frame=${frame}`); });
  ctx.events.on("round_end", () => { roundEnds += 1; L(`round_end #${roundEnds} frame=${frame}`); });

  // G6: expected NOT to run for the round_end that terminateRound fires — pre-hooks cannot be
  // deferred, so a re-entrant pre-dispatch is skipped. If this DOES log, the regression the review
  // documented is not real and spec §9.2 should be corrected.
  let pres = 0;
  ctx.events.onPre("round_end", () => {
    pres += 1;
    L(`onPre round_end #${pres} RAN (unexpected — pre-hooks were documented as skipped here)`);
    return HookResult.Continue;
  });

  ctx.commands.registerServer("a5b_report_b", () => {
    L(`REPORT spawns=${spawns} roundEnds=${roundEnds} onPre=${pres} frame=${frame}`);
  });

  return { onUnload() { L(`unloading (spawns=${spawns} roundEnds=${roundEnds} onPre=${pres})`); } };
});
