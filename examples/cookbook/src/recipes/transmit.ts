import { createEntity, EntityRef, Transmit, command, HookResult } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

const ALL_SLOTS: number[] = [];
for (let i = 0; i < 64; i++) ALL_SLOTS.push(i);

/**
 * Transmit controls per-client entity visibility (PVS transmit bits) — a
 * declarative rule the engine enforces every snapshot. Server-side proof:
 * bitsCleared grows while a hide rule is active and the entity is in a
 * viewer's PVS (the game set the transmit bit; Transmit cleared it).
 *
 *   sm_transmit          spawn a labelled point_worldtext at the first human's
 *                        feet (so it is genuinely in their PVS) and bind that slot
 *   sm_transmit_hide     hide it from a slot (default: the bound human) — visible to everyone else
 *   sm_transmit_only     show it ONLY to a slot (default: the bound human)
 *   sm_transmit_show     reset the rule (visible to all)
 *   sm_transmit_stats    print Transmit.stats()
 */
export const name = "transmit";
export const describe = "per-client entity visibility rules (sm_transmit / _hide / _only / _show / _stats)";

export function OnPluginStart(): void {
  let prop: EntityRef | null = null;
  let boundSlot = -1;

  command("sm_transmit", (cmd) => {
    // Find the first connected human (bots report steamId "0") with a live pawn.
    let origin: { x: number; y: number; z: number } | null = null;
    for (const p of Player.allConnected()) {
      if (p.steamId === "0") continue;
      const o = p.pawn ? p.pawn.origin : null;
      if (o) { origin = o; boundSlot = p.slot; break; }
    }
    if (!origin) { cmd.reply("[cookbook] transmit: no live human pawn found (spawn in first)"); return HookResult.Handled; }
    const e = createEntity("point_worldtext", {
      message: "S2 TRANSMIT TEST",
      enabled: true,
      fullbright: true,
      font_size: 100,
      world_units_per_pixel: 0.25,
      color: "255 0 0",
    });
    if (!e) { cmd.reply("[cookbook] transmit: createEntity failed"); return HookResult.Handled; }
    e.teleport([origin.x, origin.y, origin.z + 72], [0, 0, 0], null);
    prop = e;
    cmd.reply("[cookbook] transmit: spawned at human slot " + boundSlot + " idx=" + e.index + " id=" + e.id +
      " pos=" + Math.round(origin.x) + "," + Math.round(origin.y) + "," + Math.round(origin.z + 72));
    return HookResult.Handled;
  });

  command("sm_transmit_hide", (cmd) => {
    if (!prop || !prop.isValid()) { cmd.reply("[cookbook] transmit: no live prop — sm_transmit first"); return HookResult.Handled; }
    const slot = cmd.argInt(0, boundSlot);
    if (slot < 0 || slot >= 64) { cmd.reply("[cookbook] usage: sm_transmit_hide <slot 0-63>"); return HookResult.Handled; }
    const ok = Transmit.setVisibleTo(prop, ALL_SLOTS.filter((s) => s !== slot));
    cmd.reply("[cookbook] transmit: hide from slot " + slot + " -> " + ok);
    return HookResult.Handled;
  });

  command("sm_transmit_only", (cmd) => {
    if (!prop || !prop.isValid()) { cmd.reply("[cookbook] transmit: no live prop — sm_transmit first"); return HookResult.Handled; }
    const slot = cmd.argInt(0, boundSlot);
    if (slot < 0 || slot >= 64) { cmd.reply("[cookbook] usage: sm_transmit_only <slot 0-63>"); return HookResult.Handled; }
    const ok = Transmit.setVisibleTo(prop, [slot]);
    cmd.reply("[cookbook] transmit: visible ONLY to slot " + slot + " -> " + ok);
    return HookResult.Handled;
  });

  command("sm_transmit_show", (cmd) => {
    if (!prop) { cmd.reply("[cookbook] transmit: no prop"); return HookResult.Handled; }
    cmd.reply("[cookbook] transmit: reset -> " + Transmit.reset(prop));
    return HookResult.Handled;
  });

  command("sm_transmit_stats", (cmd) => {
    const s = Transmit.stats();
    if (!s) { cmd.reply("[cookbook] transmit: stats unavailable (capability off)"); return HookResult.Handled; }
    cmd.reply("[cookbook] transmit: snapshots=" + s.snapshots + " entries=" + s.entries +
      " bitsCleared=" + s.bitsCleared + " nsLast=" + s.nsLast + " nsMax=" + s.nsMax);
    return HookResult.Handled;
  });
}
