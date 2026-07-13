// Live-gate demo for entity lifecycle listeners. Self-contained + bot-provable: a spawned/removed
// logic_relay exercises onCreate/onSpawn/onDelete; a "*" spawn logger shows the create/spawn burst of
// any entity (incl. weapons given to a bot). No human client needed. The delivered `entity` is a
// serial-gated EntityRef (may be null); `className` is always valid.
import { Commands } from "@s2script/commands";
import { createEntity, Entity } from "@s2script/entity";

Entity.onSpawn("logic_relay", (e, cls) => {
  const valid = !!(e && e.isValid());
  console.log("[entlisten] onSpawn " + cls + " entity=" + (e ? ("EntityRef(valid=" + valid + ")") : "null"));
});
Entity.onCreate("logic_relay", (e, cls) => {
  console.log("[entlisten] onCreate " + cls + " entity=" + (e ? "EntityRef" : "null"));
});
Entity.onDelete("logic_relay", (e, cls) => {
  console.log("[entlisten] onDelete " + cls + " entity=" + (e ? "EntityRef" : "null"));
});

// A global "*" spawn logger — proves the wildcard + shows real engine spawns (weapons, projectiles).
let starCount = 0;
Entity.onSpawn("*", (_e, cls) => {
  starCount++;
  if (starCount <= 40) console.log("[entlisten] * onSpawn: " + cls);   // cap the map-load burst log
});

Commands.register("sm_entlisten", (ctx) => {
  const relay = createEntity("logic_relay");
  if (!relay) { ctx.reply("[entlisten] createEntity failed"); return; }
  relay.spawn();   // -> onCreate then onSpawn fire (watch the log)
  relay.remove();  // -> onDelete fires (next tick)
  ctx.reply("[entlisten] spawned+removed a logic_relay; watch the log for onCreate/onSpawn/onDelete");
});

export function onLoad(): void {
  console.log("[entity-listeners-demo] onLoad — sm_entlisten registered; * onSpawn logging armed");
}
export function onUnload(): void {}
