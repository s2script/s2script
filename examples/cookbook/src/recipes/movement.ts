import type { Recipe } from "../recipe.ts";
import { Player } from "@s2script/cs2";
import { command } from "@s2script/sdk/commands";

/**
 * Writable movement-services fields — `pawn.movementServices` gained setters for a curated
 * allowlist (games/cs2/nav-targets.json), the s2script equivalent of ModSharp's `GetMaxSpeed`
 * and friends.
 *
 * Only allowlisted fields are writable. Engine bookkeeping (`traceCount`, `inStuckTest`, …) stays
 * `readonly` on purpose: which byte a field lives at is regenerable layout, but whether writing it
 * is safe is a reviewed behavioural decision.
 *
 * These writes are NOT flagged for replication — see the note on the `MovementServices` interface.
 * The server reads them during movement, so gameplay effects apply immediately.
 */
const original = new Map<number, number>();
let sampling = false;
let sample: { slot: number; remaining: number; ticks: number; peak: number;
              peakByPos: number; last: { x: number; y: number } | null } | null = null;

export const movementRecipe: Recipe = {
  name: "movement",
  describe: "writable movement fields: sm_speed <slot> <mult>, sm_movement <slot>",

  onGameFrame() {
    if (!sampling || !sample) return;
    const pawn = Player.fromSlot(sample.slot)?.pawn;
    const v = pawn?.absVelocity;
    if (v) sample.peak = Math.max(sample.peak, Math.hypot(v.x, v.y));
    const o = pawn?.origin;
    if (o) {
      if (sample.last) {
        const d = Math.hypot(o.x - sample.last.x, o.y - sample.last.y) * 64;
        sample.peakByPos = Math.max(sample.peakByPos, d);
      }
      sample.last = { x: o.x, y: o.y };
    }
    if (--sample.remaining <= 0) {
      sampling = false;
      console.log(`[cookbook] movement slot=${sample.slot} over ${sample.ticks} frames: ` +
        `peak absVelocity=${sample.peak.toFixed(1)} u/s, peak by position delta=` +
        `${sample.peakByPos.toFixed(1)} u/s`);
    }
  },

  register() {
    command("sm_movement", (cmd) => {
      const slot = cmd.argInt(0, -1);
      const pawn = slot >= 0 ? Player.fromSlot(slot)?.pawn : null;
      const ms = pawn?.movementServices;
      if (!ms) { cmd.reply(`[cookbook] movement: no live pawn/movementServices for slot ${slot}`); return; }
      cmd.reply(
        `[cookbook] movement slot=${slot} maxspeed=${ms.maxspeed} stamina=${ms.stamina} ` +
        `friction=${ms.surfaceFriction} ducked=${ms.ducked} duckAmount=${ms.duckAmount}`);
    });

    // The live gate: bots move under server-side movement code, so a maxspeed change is
    // observable on hardware without a human client.
    command("sm_speed", (cmd) => {
      const slot = cmd.argInt(0, -1);
      const mult = parseFloat(cmd.arg(1) ?? "");
      if (slot < 0 || slot > 63 || !isFinite(mult) || mult <= 0) {
        cmd.reply("[cookbook] usage: sm_speed <slot 0-63> <multiplier>   (1 = restore)");
        return;
      }
      const ms = Player.fromSlot(slot)?.pawn?.movementServices;
      if (!ms) { cmd.reply(`[cookbook] movement: no live pawn for slot ${slot}`); return; }

      const base = original.get(slot) ?? ms.maxspeed;
      if (base === null) { cmd.reply(`[cookbook] movement: maxspeed unreadable for slot ${slot}`); return; }
      original.set(slot, base);

      const before = ms.maxspeed;
      ms.maxspeed = base * mult;          // <- the write this slice exists for
      cmd.reply(`[cookbook] movement slot=${slot} maxspeed ${before} -> ${ms.maxspeed} (base ${base} x ${mult})`);
    });

    // sm_speedsample <slot> <ticks> — peak horizontal speed over N frames. Reading maxspeed back
    // only proves the write stuck in memory; this proves the ENGINE acts on it, which is the whole
    // claim. Bots move under server-side movement code, so no human client is needed.
    command("sm_speedsample", (cmd) => {
      const slot = cmd.argInt(0, -1);
      const ticks = Math.min(Math.max(cmd.argInt(1, 128), 1), 2048);
      if (slot < 0 || slot > 63) { cmd.reply("[cookbook] usage: sm_speedsample <slot 0-63> [ticks]"); return; }
      if (sampling) { cmd.reply("[cookbook] movement: a sample is already running"); return; }
      sample = { slot, remaining: ticks, ticks, peak: 0, peakByPos: 0, last: null };
      sampling = true;
      cmd.reply(`[cookbook] movement: sampling slot ${slot} for ${ticks} frames…`);
    });
  },
};
