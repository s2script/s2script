import { Entity } from "@s2script/sdk/entity";
import { Commands } from "@s2script/sdk/commands";
import { Server } from "@s2script/sdk/server";

// Live-gate demo for the entity_name primitive: dump every trigger_multiple's targetname.
// EntityRef.name reads CEntityIdentity::m_name — on a CS2Surf-spec map (e.g. surf_kitsune) this
// should list map_start / map_end / stageN_start / bonusN_* / surftimer_* (NOT all "" or null).
function dumpTriggers(): void {
  const triggers = Entity.findByClass("trigger_multiple");
  console.log(`[entity-name-demo] ${triggers.length} trigger_multiple on ${Server.mapName}:`);
  for (const t of triggers) {
    console.log(`[entity-name-demo]   #${t.index} name=${JSON.stringify(t.name)}`);
  }
}

export function onLoad(): void {
  // Run `entity_names` from rcon once the map is fully loaded (the reliable gate path).
  Commands.register("entity_names", () => dumpTriggers());
  // Also auto-dump on each map start (a bonus; entities may still be spawning at this point).
  Server.onMapStart(() => dumpTriggers());
  console.log("[entity-name-demo] loaded — run `entity_names` via rcon after the map loads");
}

export function onUnload(): void {}
