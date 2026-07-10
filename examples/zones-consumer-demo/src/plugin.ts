// zones-consumer-demo — a SEPARATE plugin that consumes the `zones` inter-plugin interface
// (published by @s2script/zones). Proves zones are a platform: this plugin reacts to zone events
// it doesn't own — logging enter/leave and HEALING players who stand in a zone named "heal".
import { on, getZones } from "zones";        // hard dep -> producer-backed proxy (throws while producer unloaded)
import { Player } from "@s2script/cs2";

// The gate types a plugin-dependency import as `any`, so annotate the event payload explicitly
// (mirrors greeter-consumer; the hand-written zones.d.ts types it in an editor).
type ZoneEvent = { zone: string; slot: number; userId: number };

export function onLoad(): void {
  on("enter", (p: ZoneEvent) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] ENTER ${p.zone}: ${nm}`);
  });
  on("leave", (p: ZoneEvent) => {
    const nm = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
    console.log(`[zones-consumer] LEAVE ${p.zone}: ${nm}`);
  });
  // The real reaction: top up health (+1/tick, ~8/s at the producer's ~8 Hz stay) for anyone in "heal".
  on("stay", (p: ZoneEvent) => {
    if (p.zone !== "heal") return;
    const pw = Player.fromSlot(p.slot)?.pawn;
    if (pw && pw.health != null && pw.health < 100) {
      const nh = Math.min(100, pw.health + 1);
      pw.health = nh;
      if (nh % 20 === 0 || nh === 100) console.log(`[zones-consumer] healed slot ${p.slot} -> ${nh}`);  // throttled progression log
    }
  });
  try {
    console.log(`[zones-consumer] onLoad — subscribed; getZones()=${getZones().length}`);
  } catch (e) {
    console.log(`[zones-consumer] onLoad — subscribed (producer absent: ${String(e)})`);
  }
}
