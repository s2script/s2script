// entity-playground — creating, configuring, wiring, and watching entities.
//
// Commands (run from rcon or console):
//   ent_create    spawn an entity, read its fields back, then remove it
//   ent_kv        spawn entities configured by keyvalues, proven two ways
//   ent_io        fire an input and catch the output it produces
//   ent_names     list every trigger_multiple on the map by targetname
//   ent_beam      draw a beam between two points for 3 seconds
//
// Everything an entity API hands you is an EntityRef — a serial-gated handle,
// never a raw pointer. Reads return `T | null`: if the entity died, you get
// null, not garbage and not a crash. Hold refs across time freely.
import { plugin } from "@s2script/sdk/plugin";
import { createEntity, Entity } from "@s2script/sdk/entity";
import { Server } from "@s2script/sdk/server";
import { Beam } from "@s2script/cs2";
import { Vector } from "@s2script/sdk/math";
import { delay } from "@s2script/sdk/timers";

// Schema offsets are resolved live from the engine's SchemaSystem — never
// hardcoded. A field moving in a CS2 patch must not require a code change.
declare const __s2_schema_offset: (cls: string, field: string) => number;

export default plugin((ctx) => {
  // --- Lifecycle listeners -------------------------------------------------
  // The useful case is reacting to ENGINE-driven lifecycle: map entities,
  // weapons, grenades, ragdolls. `entity` is a serial-gated EntityRef and may
  // be null for a barely-constructed create or a dying delete; `className` is
  // always valid. Counters keep a map-load burst readable.
  let created = 0, spawned = 0, deleted = 0;
  ctx.entities.onCreate("*", (_e, cls) => { if (++created <= 10) console.log(`[ent] created ${cls}`); });
  ctx.entities.onSpawn("*", (e, cls) => { if (++spawned <= 10) console.log(`[ent] spawned ${cls} valid=${!!e?.isValid()}`); });
  ctx.entities.onDelete("*", (_e, cls) => { if (++deleted <= 10) console.log(`[ent] deleted ${cls}`); });

  // Hook a named output on a class. Return a HookResult to suppress it.
  ctx.entities.onOutput("logic_relay", "OnTrigger", (ev) => {
    console.log(`[ent] OnTrigger caller=${ev.caller ? `valid=${ev.caller.isValid()}` : "null"}`);
  });
  ctx.entities.onOutput("math_counter", "OnHitMax", () => {
    console.log("[ent] OnHitMax — the counter reached the max its keyvalues set");
  });

  // --- Create, read back, remove -------------------------------------------
  ctx.commands.register("ent_create", (cmd) => {
    const text = createEntity("point_worldtext");
    if (!text) { cmd.reply("createEntity failed"); return; }
    text.spawn();
    text.teleport([0, 0, 100]);
    cmd.reply(`created point_worldtext #${text.index} valid=${text.isValid()}`);
    delay(3000).then(() => cmd.reply(`removed -> ${text.remove()}`));
  });

  // --- Keyvalue-configured spawn -------------------------------------------
  // createEntity(className, keyvalues) builds a CEntityKeyValues and dispatches
  // the spawn with it, so the entity's OWN Spawn() parses the keys. Proven two
  // ways: read the parsed fields back through the schema, and let an int
  // keyvalue drive the entity's own logic until it fires an output.
  ctx.commands.register("ent_kv", (cmd) => {
    const text = createEntity("point_worldtext", { message: "configured-by-keyvalues", enabled: true, fullbright: true });
    if (text) {
      const msg = text.readString(__s2_schema_offset("CPointWorldText", "m_messageText"), 512);
      const fullbright = text.readBool(__s2_schema_offset("CPointWorldText", "m_bFullbright"));
      cmd.reply(`worldtext message=${JSON.stringify(msg)} fullbright=${fullbright}`);
    }

    const counter = createEntity("math_counter", { startvalue: 5, min: 1, max: 10 });
    if (counter) {
      const max = counter.readFloat32(__s2_schema_offset("CMathCounter", "m_flMax"));
      cmd.reply(`counter max=${max}; adding 5 to its start of 5 -> expect OnHitMax`);
      counter.acceptInput("Add", "5");
    }

    delay(3000).then(() => { text?.remove(); counter?.remove(); });
  });

  // --- Entity I/O ----------------------------------------------------------
  // acceptInput queues an I/O event that the game's own pump routes to the
  // entity's outputs, which our onOutput subscriber above catches next tick.
  // Passing activator/caller gives the output hook live EntityRefs to report.
  ctx.commands.register("ent_io", (cmd) => {
    const relay = createEntity("logic_relay");
    if (!relay) { cmd.reply("createEntity failed"); return; }
    relay.spawn();
    const ok = relay.acceptInput("Trigger", "", relay, relay);
    cmd.reply(`fired Trigger ok=${ok} — watch the log for the output next tick`);
  });

  // --- Finding entities ----------------------------------------------------
  // EntityRef.name reads CEntityIdentity::m_name (the map's targetname).
  ctx.commands.register("ent_names", (cmd) => {
    const triggers = Entity.findByClass("trigger_multiple");
    cmd.reply(`${triggers.length} trigger_multiple on ${Server.mapName}`);
    for (const t of triggers) console.log(`[ent]   #${t.index} name=${JSON.stringify(t.name)}`);
  });

  // --- Beams ---------------------------------------------------------------
  ctx.commands.register("ent_beam", (cmd) => {
    const handle = Beam.draw(new Vector(0, 0, 100), new Vector(200, 0, 100), { color: [0, 255, 0, 255], width: 3 });
    if (!handle) { cmd.reply("beam failed"); return; }
    cmd.reply(`beam drawn ref valid=${handle.ref.isValid()}`);
    delay(3000).then(() => cmd.reply(`beam removed -> ${handle.remove()}`));
  });

  console.log("[ent] entity-playground loaded — try ent_create, ent_kv, ent_io, ent_names, ent_beam");
});
