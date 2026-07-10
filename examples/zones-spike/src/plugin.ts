// zones-spike — sub-slice 1 of the zone system, on the ORIGIN-POLLING detection backend.
// (The real-trigger approach was spiked and parked: a runtime trigger_multiple's bbox writes land
//  but the entity is never registered with the collision spatial partition — that needs an internal
//  CCollisionProperty::MarkPartitionHandleDirty/SetSolid call, an un-published function requiring
//  from-scratch disassembly RE. Documented as a future enhancement.) Polling is what most SourceMod
//  zone mods use and is polling's sweet spot: box zones, player enter/leave.
import { Commands } from "@s2script/commands";
import { OnGameFrame } from "@s2script/frame";
import { Pawn, Player } from "@s2script/cs2";

interface Box { minX: number; minY: number; minZ: number; maxX: number; maxY: number; maxZ: number; }

let zone: Box | null = null;
const inside = new Set<number>();   // player slots currently inside the zone

function contains(b: Box, x: number, y: number, z: number): boolean {
  return x >= b.minX && x <= b.maxX && y >= b.minY && y <= b.maxY && z >= b.minZ && z <= b.maxZ;
}

export function onLoad(): void {
  // The detection loop: ~8 Hz, diff the inside-set → OnZoneEnter / OnZoneLeave (OnZoneStay while inside
  // is the real API but too noisy to log here). This is the spine sub-slices 2/3 build on.
  let frame = 0;
  OnGameFrame.subscribe(() => {
    if (!zone) return;
    if ((frame++ & 7) !== 0) return;
    const cur = new Set<number>();
    for (const p of Player.all()) {
      const pw = p.pawn;
      if (!pw) continue;
      const o = pw.origin;
      if (!o) continue;
      if (contains(zone, o.x, o.y, o.z)) {
        cur.add(p.slot);
        if (!inside.has(p.slot)) console.log(`[zonestest] ENTER: ${p.playerName} (slot ${p.slot})`);
      }
    }
    for (const s of inside) if (!cur.has(s)) console.log(`[zonestest] LEAVE: slot ${s}`);
    inside.clear();
    for (const s of cur) inside.add(s);
  });

  // sm_zonetest [slot] [half] — define a box centered on a bot's origin (default slot 0, ±128u).
  Commands.register("sm_zonetest", (ctx) => {
    const slot = ctx.args.length > 0 ? parseInt(ctx.args[0], 10) : 0;
    const half = ctx.args.length > 1 ? parseFloat(ctx.args[1]) : 128;
    const pw = Pawn.forSlot(slot);
    const o = pw ? pw.origin : null;
    if (!o) { ctx.reply(`[zonestest] no pawn/origin for slot ${slot}`); return; }
    zone = { minX: o.x - half, minY: o.y - half, minZ: o.z - half, maxX: o.x + half, maxY: o.y + half, maxZ: o.z + half };
    inside.clear();
    ctx.reply(`[zonestest] zone set @ (${o.x.toFixed(0)},${o.y.toFixed(0)},${o.z.toFixed(0)}) half=${half} — polling for enter/leave`);
  });

  Commands.register("sm_zonetest_clear", (ctx) => {
    zone = null;
    inside.clear();
    ctx.reply("[zonestest] zone cleared");
  });

  console.log("[zones-spike] onLoad — sm_zonetest / sm_zonetest_clear registered (origin-polling)");
}
