// zones-consumer-demo — a SEPARATE plugin that consumes the @s2script/zones inter-plugin interface
// (published by the @s2script/zones plugin). Proves zones are a platform: this plugin reacts to zone
// events it doesn't own — logging enter/leave and HEALING players who stand in a zone named "heal".
//
// Topological activation guarantees the @s2script/zones producer is Active before this consumer's
// factory runs, so the old poll-until-producer deferral is gone — subscribe once, synchronously.
import { plugin } from "@s2script/sdk/plugin";
import type { Zones } from "../../../plugins/zones/api";
// TYPES come straight from the producer's contract, so they cannot drift from it.
// @s2script/zones is published by a PLUGIN, so it has no packagesDir stub for the typecheck gate
// to resolve (the contract-grammar slice deleted packages/zones — design spec §6); the gate's
// ambient fallback types the VALUES above as `any`, and an ambient `declare module` cannot carry
// these types (TS forbids relative re-exports inside one). Reaching across the monorepo is honest
// for a demo and zero-drift; a real consumer outside this repo does `s2script add @s2script/zones`
// and gets .s2script/types/@s2script/zones/index.d.ts instead (spec §4.6, plan 2). Replace this
// import when that lands — tracked in the spec's §10.
import type { ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "../../../plugins/zones/api";
import { Player } from "@s2script/cs2";

export default plugin((ctx) => {
  const zones = ctx.use<Zones>("@s2script/zones");
  zones.on("enter", (p: ZoneEvent) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] ENTER ${p.zone}: ${nm}`);
  });
  zones.on("leave", (p: ZoneEvent) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] LEAVE ${p.zone}: ${nm}`);
  });
  zones.on("stay", (p: ZoneEvent) => {
    if (p.zone !== "heal") return;
    const pw = Player.fromSlot(p.slot)?.pawn;
    if (pw && pw.health != null && pw.health < 100) {
      const nh = Math.min(100, pw.health + 1);
      pw.health = nh;
      if (nh % 20 === 0 || nh === 100) console.log(`[zones-consumer] healed slot ${p.slot} -> ${nh}`);
    }
  });
  zones.on("created", (p: ZoneCreatedEvent) => { console.log(`[zones-consumer] CREATED ${p.zone} tags=[${p.tags.join(",")}]`); });
  zones.on("deleted", (p: ZoneDeletedEvent) => { console.log(`[zones-consumer] DELETED ${p.zone}`); });
});
