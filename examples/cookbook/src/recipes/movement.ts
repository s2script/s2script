import type { Recipe } from "../recipe.ts";
import { Player } from "@s2script/cs2";

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
export const movementRecipe: Recipe = {
  name: "movement",
  describe: "writable movement fields: sm_speed <slot> <mult>, sm_movement <slot>",

  register(ctx) {
    // Remember each slot's original maxspeed so sm_speed can restore it, rather than assuming
    // 260 — the base value depends on the active weapon.
    const original = new Map<number, number>();

    ctx.commands.register("sm_movement", (cmd) => {
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
    ctx.commands.register("sm_speed", (cmd) => {
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
  },
};
