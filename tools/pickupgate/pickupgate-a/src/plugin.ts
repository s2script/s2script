// pickupgate-a — live-gate fixture for CanAcquire (PR1). NOT a shipped plugin.
//
// Drives the six checks in docs/superpowers/specs/2026-08-14-pickup-gates-design.md §8.
// Prefix [PICKUPGATE] so `docker logs | grep PICKUPGATE` reads as a transcript.
import { HookResult } from "@s2script/sdk/events";
import { AcquireResult, CsItem, Player, items } from "@s2script/cs2";
import { command } from "@s2script/sdk/commands";
// AcquireResult is types-only (@s2script/cs2 ships no runtime). Numeric values match the enum.

export function OnPluginStart(): void {
  const L = (m: string) => console.log(`[PICKUPGATE] ${m}`);
  L("loaded");

  let pre = 0;
  let post = 0;
  let lastSkipped = false;
  let mode: "observe" | "deny" | "reenter" = "observe";
  // Item definition index for weapon_negev (not CSWeaponID). Bots do not poll this, so a
  // one-shot mode is not stolen by the AK AlreadyOwned spam.
  const TARGET = 28;

  items.onCanAcquire((acq) => {
    pre += 1;
    if (acq.defIndex !== TARGET && mode === "observe") return;
    L(`onCanAcquire #${pre}: defIndex=${acq.defIndex} method=${acq.method} result=${acq.result} player=${acq.player ? acq.player.slot : "null"} skipped=${acq.skipped}`);
    if (acq.defIndex !== TARGET) return;
    if (mode === "deny") {
      mode = "observe";
      acq.result = 1 satisfies AcquireResult; // InvalidItem — enum is types-only, no runtime object
      L("  -> Handled + InvalidItem (item must be withheld)");
      return HookResult.Handled;
    }
    if (mode === "reenter") {
      mode = "observe";
      const pawn = acq.player?.pawn;
      L(`  -> giveNamedItem from inside handler (must skip + name); pawn=${pawn ? "yes" : "null"}`);
      if (pawn) pawn.giveNamedItem(CsItem.Negev);
      return HookResult.Continue;
    }
    return HookResult.Continue;
  });

  items.onCanAcquirePost((acq) => {
    post += 1;
    if (acq.defIndex !== TARGET) return;
    lastSkipped = acq.skipped;
    L(`onCanAcquirePost #${post}: defIndex=${acq.defIndex} result=${acq.result} skipped=${acq.skipped}`);
  });

  command.server("pickup_report", () => {
    L(`REPORT pre=${pre} post=${post} lastSkipped=${lastSkipped}`);
  });
  command.server("pickup_deny", () => {
    mode = "deny";
    L("armed: next CanAcquire returns Handled + InvalidItem");
  });
  command.server("pickup_reenter", () => {
    mode = "reenter";
    L("armed: next CanAcquire calls giveNamedItem from inside the handler");
  });
  command.server("pickup_give", () => {
    const players = Player.all();
    const p = players[0];
    const pawn = p?.pawn;
    L(`pickup_give: players=${players.length} pawn=${pawn ? "yes" : "null"}`);
    if (pawn) {
      const w = pawn.giveNamedItem(CsItem.Negev);
      L(`pickup_give: giveNamedItem returned ${w ? "weapon" : "null"}`);
    }
  });
}

export function OnPluginEnd(): void {
  console.log("[PICKUPGATE] unloading");
}
