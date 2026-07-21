// zones-consumer-demo — a SEPARATE plugin that consumes the @s2script/zones inter-plugin interface
// (published by the @s2script/zones plugin). Proves zones are a platform: this plugin reacts to zone
// events it doesn't own — logging enter/leave and HEALING players who stand in a zone named "heal".
//
// Topological activation guarantees the @s2script/zones producer is Active before this consumer's
// factory runs, so the old poll-until-producer deferral is gone — subscribe once, synchronously.
import { plugin } from "@s2script/sdk/plugin";
// TYPES come from the verified contract copy (.s2script/types/@s2script/zones/index.d.ts — a
// byte-copy of the producer's api.d.ts). `s2s build` typechecks against it AND hashes it into
// manifest.compiledAgainst, so if the producer's contract drifts, this consumer is refused at
// load instead of marshalling across a stale contract (B1). Refresh with:
//   cp plugins/zones/api.d.ts examples/zones-consumer-demo/.s2script/types/@s2script/zones/index.d.ts
import type { Zones, ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "@s2script/zones";
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
