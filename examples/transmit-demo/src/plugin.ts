// Live-gate demo for @s2script/sdk/transmit (per-client entity visibility). rcon/chat-driven:
//   sm_tspawn        -> find the first HUMAN, spawn a labelled point_worldtext at their feet
//                       (so it is genuinely in their PVS), and bind that slot
//   sm_thide [slot]  -> hide it from that viewer slot (default: the bound human) — visible to everyone else
//   sm_tonly [slot]  -> show it ONLY to that slot (default: the bound human)
//   sm_tshow         -> reset this plugin's rule (visible to all)
//   sm_tstats        -> print Transmit.stats()
// Server-side proof of the clear = bitsCleared grows while a hide rule is active and the entity is
// in a viewer's PVS (the game set the transmit bit; our post-hook cleared it). Client-side visual
// proof (the text pops in/out for exactly the filtered client) is the human check.
import { plugin } from "@s2script/sdk/plugin";
import { createEntity, EntityRef } from "@s2script/sdk/entity";
import { Transmit } from "@s2script/sdk/transmit";
import { Player } from "@s2script/cs2";

let prop: EntityRef | null = null;
let boundSlot = -1;
const ALL_SLOTS: number[] = [];
for (let i = 0; i < 64; i++) ALL_SLOTS.push(i);

export default plugin((ctx) => {
  ctx.commands.register("sm_tspawn", (cmd) => {
    // Find the first connected human (bots report steamId "0") with a live pawn.
    let origin: { x: number; y: number; z: number } | null = null;
    for (const p of Player.allConnected()) {
      if (p.steamId === "0") continue;
      const o = p.pawn ? p.pawn.origin : null;
      if (o) { origin = o; boundSlot = p.slot; break; }
    }
    if (!origin) { cmd.reply("[transmit] no live human pawn found (spawn in first)"); return; }
    const e = createEntity("point_worldtext", {
      message: "S2 TRANSMIT TEST",
      enabled: true,
      fullbright: true,
      font_size: 100,
      world_units_per_pixel: 0.25,
      color: "255 0 0",
    });
    if (!e) { cmd.reply("[transmit] createEntity failed"); return; }
    e.teleport([origin.x, origin.y, origin.z + 72], [0, 0, 0], null);
    prop = e;
    cmd.reply("[transmit] spawned at human slot " + boundSlot + " idx=" + e.index + " id=" + e.id +
      " pos=" + Math.round(origin.x) + "," + Math.round(origin.y) + "," + Math.round(origin.z + 72));
  });

  ctx.commands.register("sm_thide", (cmd) => {
    if (!prop || !prop.isValid()) { cmd.reply("[transmit] no live prop — sm_tspawn first"); return; }
    const slot = cmd.argInt(0, boundSlot);
    if (slot < 0 || slot >= 64) { cmd.reply("[transmit] usage: sm_thide <slot 0-63>"); return; }
    const ok = Transmit.setVisibleTo(prop, ALL_SLOTS.filter((s) => s !== slot));
    cmd.reply("[transmit] hide from slot " + slot + " -> " + ok);
  });

  ctx.commands.register("sm_tonly", (cmd) => {
    if (!prop || !prop.isValid()) { cmd.reply("[transmit] no live prop — sm_tspawn first"); return; }
    const slot = cmd.argInt(0, boundSlot);
    if (slot < 0 || slot >= 64) { cmd.reply("[transmit] usage: sm_tonly <slot 0-63>"); return; }
    const ok = Transmit.setVisibleTo(prop, [slot]);
    cmd.reply("[transmit] visible ONLY to slot " + slot + " -> " + ok);
  });

  ctx.commands.register("sm_tshow", (cmd) => {
    if (!prop) { cmd.reply("[transmit] no prop"); return; }
    cmd.reply("[transmit] reset -> " + Transmit.reset(prop));
  });

  ctx.commands.register("sm_tstats", (cmd) => {
    const s = Transmit.stats();
    if (!s) { cmd.reply("[transmit] stats unavailable (capability off)"); return; }
    cmd.reply("[transmit] snapshots=" + s.snapshots + " entries=" + s.entries +
      " bitsCleared=" + s.bitsCleared + " nsLast=" + s.nsLast + " nsMax=" + s.nsMax);
  });

  console.log("[transmit-demo] onLoad — sm_tspawn/sm_thide/sm_tonly/sm_tshow/sm_tstats registered");
});
