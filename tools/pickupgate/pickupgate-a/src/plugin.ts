// pickupgate-a — live-gate fixture for CanAcquire (PR1). NOT a shipped plugin.
//
// Drives the six checks in docs/superpowers/specs/2026-08-14-pickup-gates-design.md §8.
// Prefix [PICKUPGATE] so `docker logs | grep PICKUPGATE` reads as a transcript.
import { plugin } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
import { AcquireResult, CsItem, Player } from "@s2script/cs2";

export default plugin((ctx) => {
  const L = (m: string) => console.log(`[PICKUPGATE] ${m}`);
  L("loaded");

  let pre = 0;
  let post = 0;
  let lastSkipped = false;
  let mode: "observe" | "deny" | "reenter" = "observe";

  ctx.items.onCanAcquire((acq) => {
    pre += 1;
    L(`onCanAcquire #${pre}: defIndex=${acq.defIndex} method=${acq.method} result=${acq.result} player=${acq.player ? acq.player.slot : "null"} skipped=${acq.skipped}`);
    if (mode === "deny") {
      mode = "observe";
      acq.result = AcquireResult.InvalidItem;
      L("  -> Handled + InvalidItem (item must be withheld)");
      return HookResult.Handled;
    }
    if (mode === "reenter") {
      mode = "observe";
      const pawn = acq.player?.pawn;
      L(`  -> giveNamedItem from inside handler (must skip + name); pawn=${pawn ? "yes" : "null"}`);
      if (pawn) pawn.giveNamedItem(CsItem.AK47);
      return HookResult.Continue;
    }
    return HookResult.Continue;
  });

  ctx.items.onCanAcquirePost((acq) => {
    post += 1;
    lastSkipped = acq.skipped;
    L(`onCanAcquirePost #${post}: defIndex=${acq.defIndex} result=${acq.result} skipped=${acq.skipped}`);
  });

  ctx.commands.registerServer("pickup_report", () => {
    L(`REPORT pre=${pre} post=${post} lastSkipped=${lastSkipped}`);
  });
  ctx.commands.registerServer("pickup_deny", () => {
    mode = "deny";
    L("armed: next CanAcquire returns Handled + InvalidItem");
  });
  ctx.commands.registerServer("pickup_reenter", () => {
    mode = "reenter";
    L("armed: next CanAcquire calls giveNamedItem from inside the handler");
  });
  ctx.commands.registerServer("pickup_give", () => {
    const players = Player.all();
    const p = players[0];
    const pawn = p?.pawn;
    L(`pickup_give: players=${players.length} pawn=${pawn ? "yes" : "null"}`);
    if (pawn) {
      const w = pawn.giveNamedItem(CsItem.AK47);
      L(`pickup_give: giveNamedItem returned ${w ? "weapon" : "null"}`);
    }
  });

  return { onUnload() { L(`unloading (pre=${pre} post=${post})`); } };
});
