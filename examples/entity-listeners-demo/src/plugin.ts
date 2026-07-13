// Live-gate demo for entity lifecycle listeners. Bot-provable, no human client needed.
//
// The USEFUL, common case is reacting to ENGINE-driven lifecycle (map/round entities, weapons,
// grenades, projectiles, ragdolls) — those fire OUTSIDE any JS borrow, so the "*" loggers below catch
// them (a round restart / bot lifecycle produces a burst of all three). The delivered `entity` is a
// serial-gated EntityRef (may be null for a barely-constructed create / a dying delete); `className`
// is always valid.
//
// NOTE (documented re-entrancy limit — same as Events.fire-from-a-handler): sm_entlisten creates the
// relay SYNCHRONOUSLY inside a command handler, which already holds the isolate borrow, so the engine's
// OnEntityCreated/Spawned callbacks re-enter the dispatch and are gracefully SKIPPED (never a crash).
// So the self-spawn does NOT log — that's by design; watch the "*" loggers for engine-driven events.
import { Commands } from "@s2script/commands";
import { createEntity, Entity } from "@s2script/entity";

// Exact-class subscriptions for logic_relay (fire only when the engine creates/spawns/deletes one
// OUTSIDE a JS borrow — e.g. an entity placed by the map, or a console `ent_create`).
Entity.onCreate("logic_relay", (e, cls) => {
  console.log("[entlisten] onCreate " + cls + " entity=" + (e ? "EntityRef" : "null"));
});
Entity.onSpawn("logic_relay", (e, cls) => {
  console.log("[entlisten] onSpawn " + cls + " entity=" + (e ? ("EntityRef(valid=" + !!(e && e.isValid()) + ")") : "null"));
});
Entity.onDelete("logic_relay", (e, cls) => {
  console.log("[entlisten] onDelete " + cls + " entity=" + (e ? "EntityRef" : "null"));
});

// Global "*" loggers — prove each of the three kinds + the wildcard for ENGINE-driven lifecycle. Each
// capped so a map-load / round-restart burst stays readable; `valid` shows the EntityRef resolves live.
let nCreate = 0, nSpawn = 0, nDelete = 0;
Entity.onCreate("*", (_e, cls) => { if (++nCreate <= 15) console.log("[entlisten] * onCreate: " + cls); });
Entity.onSpawn("*", (e, cls) => { if (++nSpawn <= 15) console.log("[entlisten] * onSpawn: " + cls + " valid=" + !!(e && e.isValid())); });
Entity.onDelete("*", (_e, cls) => { if (++nDelete <= 15) console.log("[entlisten] * onDelete: " + cls); });

Commands.register("sm_entlisten", (ctx) => {
  // Demonstrates the re-entrancy limit (see the header note): the create/spawn/remove below run under
  // the command's isolate borrow, so their lifecycle callbacks are SKIPPED, not logged. This is by
  // design — engine-driven lifecycle (the "*" loggers) is the real use case.
  const relay = createEntity("logic_relay");
  if (!relay) { ctx.reply("[entlisten] createEntity failed"); return; }
  relay.spawn();
  relay.remove();
  ctx.reply("[entlisten] self-spawned+removed a logic_relay (re-entrant → intentionally NOT logged); " +
    "trigger a round restart / bot lifecycle to see the '*' loggers fire for engine-driven entities");
});

export function onLoad(): void {
  console.log("[entity-listeners-demo] onLoad — sm_entlisten registered; * onCreate/onSpawn/onDelete logging armed");
}
export function onUnload(): void {}
