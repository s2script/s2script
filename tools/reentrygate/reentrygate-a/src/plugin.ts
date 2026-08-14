// reentrygate-a — live-gate for isolate re-entry (board-wide nest). NOT shipped.
//
// Asserts both proving-slice checks:
//   1. pawn.giveNamedItem runs onCanAcquire BEFORE giveNamedItem returns
//   2. player.respawn runs Events.onPre("player_spawn") BEFORE respawn returns
//
// Prefix [REENTRYGATE]. Drive: re_give, re_respawn, re_report.
import { plugin } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
import { nextTick } from "@s2script/sdk/timers";
import { Server } from "@s2script/sdk/server";
import { CsItem, Player } from "@s2script/cs2";

export default plugin((ctx) => {
  const L = (m: string) => console.log(`[REENTRYGATE] ${m}`);
  L("loaded");

  let acqDuringGive = false;
  let acqCount = 0;
  ctx.items.onCanAcquire((acq) => {
    if (acq.defIndex !== 28) return;
    acqCount += 1;
    acqDuringGive = true;
    L(`onCanAcquire #${acqCount} defIndex=${acq.defIndex} (duringGive=${acqDuringGive})`);
    return HookResult.Continue;
  });

  let preDuringRespawn = false;
  let preCount = 0;
  ctx.events.onPre("player_spawn", () => {
    preCount += 1;
    preDuringRespawn = true;
    L(`onPre player_spawn #${preCount}`);
    return HookResult.Continue;
  });

  let deathDuringSlay = false;
  let deathCount = 0;
  ctx.events.onPre("player_death", () => {
    deathCount += 1;
    deathDuringSlay = true;
    L(`onPre player_death #${deathCount}`);
    return HookResult.Continue;
  });

  ctx.commands.registerServer("re_give", () => {
    const p = Player.all()[0];
    const pawn = p?.pawn;
    L(`re_give: players=${Player.all().length} pawn=${pawn ? "yes" : "null"}`);
    if (!pawn) return;
    acqDuringGive = false;
    const w = pawn.giveNamedItem(CsItem.Negev);
    L(`re_give: returned=${w ? "weapon" : "null"} onCanAcquireDuringCall=${acqDuringGive} PASS=${acqDuringGive}`);
  });

  ctx.commands.registerServer("re_respawn", async () => {
    let p = Player.all()[0];
    L(`re_respawn: players=${Player.all().length} slot=${p ? p.slot : "none"} pawn=${p?.pawn ? "yes" : "null"} pawnIsAlive=${p ? p.pawnIsAlive : "none"}`);
    if (!p) return;
    // CommitSuicide does not flip m_bPawnIsAlive on this tick; warmup/auto-respawn
    // can also revive before we look. Kill, then wait until the controller is dead.
    if (p.pawnIsAlive === true) {
      deathDuringSlay = false;
      p.pawn?.slay();
      L(`re_respawn: slayed; deathDuringCall=${deathDuringSlay} waiting for pawnIsAlive=false`);
      for (let i = 0; i < 32; i++) {
        await nextTick();
        p = Player.fromSlot(p.slot) ?? p;
        if (p.pawnIsAlive !== true) break;
      }
      L(`re_respawn: after wait pawnIsAlive=${p.pawnIsAlive}`);
    }
    if (p.pawnIsAlive === true) {
      L("re_respawn: still alive (auto-respawn?) — cannot exercise Respawn");
      return;
    }
    preDuringRespawn = false;
    const r = p.respawn();
    L(`re_respawn: returned=${r} pawnIsAliveAfter=${p.pawnIsAlive} onPreDuringCall=${preDuringRespawn} PASS=${preDuringRespawn}`);
  });

  ctx.commands.registerServer("re_cvar", () => {
    let during = false;
    const sub = Server.onCvarChange("sv_gravity", (name, next, prev) => {
      during = true;
      L(`onCvarChange ${name} ${prev}->${next}`);
    });
    const before = Server.getCvar("sv_gravity");
    const ok = Server.setCvar("sv_gravity", "400");
    const after = Server.getCvar("sv_gravity");
    L(`re_cvar: set=${ok} before=${before} after=${after} onChangeDuringCall=${during} PASS=${!!ok && after === "400" && during}`);
    if (before) Server.setCvar("sv_gravity", before);
    sub.dispose();
  });

  ctx.commands.registerServer("re_report", () => {
    L(`REPORT acqCount=${acqCount} preCount=${preCount} deathCount=${deathCount}`);
  });

  return { onUnload() { L("unloading"); } };
});
