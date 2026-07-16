// zones-consumer-demo — a SEPARATE plugin that consumes the @s2script/zones inter-plugin interface
// (published by the @s2script/zones plugin). Proves zones are a platform: this plugin reacts to zone
// events it doesn't own — logging enter/leave and HEALING players who stand in a zone named "heal".
//
// LOAD-ORDER NOTE: the loader loads plugins alphabetically (no dependency order), so this @demo
// consumer loads BEFORE the @s2script producer. A subscription registered while the producer is absent
// does NOT wire — so we DEFER the subscription until the producer is live, probing with a method call
// (getZones, which throws InterfaceUnavailable while the producer is absent, unlike on()).
import { on, getZones } from "@s2script/zones";   // hard dep -> producer-backed proxy
// TYPES come straight from the producer's contract, so they cannot drift from it.
// @s2script/zones is published by a PLUGIN, so it has no packagesDir stub for the typecheck gate
// to resolve (the contract-grammar slice deleted packages/zones — design spec §6); the gate's
// ambient fallback types the VALUES above as `any`, and an ambient `declare module` cannot carry
// these types (TS forbids relative re-exports inside one). Reaching across the monorepo is honest
// for a demo and zero-drift; a real consumer outside this repo does `s2script add @s2script/zones`
// and gets .s2script/types/@s2script/zones/index.d.ts instead (spec §4.6, plan 2). Replace this
// import when that lands — tracked in the spec's §10.
import type { ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "../../../plugins/zones/api";
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

let subscribed = false;

function subscribe(): void {
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
      if (nh % 20 === 0 || nh === 100) console.log(`[zones-consumer] healed slot ${p.slot} -> ${nh}`);
    }
  });
  on("created", (p: ZoneCreatedEvent) => {
    console.log(`[zones-consumer] CREATED ${p.zone} tags=[${p.tags.join(",")}] min=(${p.min.x.toFixed(0)},${p.min.y.toFixed(0)},${p.min.z.toFixed(0)}) max=(${p.max.x.toFixed(0)},${p.max.y.toFixed(0)},${p.max.z.toFixed(0)})`);
  });
  on("deleted", (p: ZoneDeletedEvent) => { console.log(`[zones-consumer] DELETED ${p.zone}`); });
}

// True once the @s2script/zones producer is live (a method call no longer throws InterfaceUnavailable).
function tryConnect(): boolean {
  try { getZones(); } catch { return false; }   // producer not up yet
  subscribe();                                   // producer live -> on(...) wires now
  return true;
}

export function onLoad(): void {
  if (tryConnect()) {
    subscribed = true;
    console.log("[zones-consumer] onLoad — subscribed (producer live)");
    return;
  }
  console.log("[zones-consumer] onLoad — producer not up yet; deferring subscription");
  OnGameFrame.subscribe(() => {
    if (subscribed) return;
    if (tryConnect()) {
      subscribed = true;
      console.log("[zones-consumer] subscribed (deferred, producer now live)");
    }
  });
}
