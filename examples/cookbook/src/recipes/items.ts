import type { Recipe } from "../recipe.ts";
import { CsItem, Pawn } from "@s2script/cs2";
import { Server } from "@s2script/sdk/server";

// Acts on every LIVE pawn via Pawn.forSlot rather than SM-style Player.target()
// name/team resolution — the client-list offsets that back target()/allConnected()
// were stale on the build these were originally written against; Pawn.forSlot is
// schema-based and self-healing, and "every live pawn" is exactly what "@all"
// would have resolved to anyway.
const MAX_SLOTS = 12;

function livePawns(): Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> {
  const out: Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> = [];
  for (let slot = 0; slot < MAX_SLOTS; slot++) {
    const pawn = Pawn.forSlot(slot);
    if (pawn) out.push({ slot, pawn });
  }
  return out;
}

/**
 * cb_items  give|strip|drop|weapons — the item surface: giveNamedItem /
 *   stripWeapons / dropActiveWeapon / enumerate held weapons.
 * cb_weapon info|refill|disarm|nofire — the Weapon entity object + pawn fire
 *   control.
 */
export const itemsRecipe: Recipe = {
  name: "items",
  describe: "give/strip/enumerate items (cb_items) and weapon-specific control (cb_weapon)",
  register(ctx) {
    ctx.commands.register("cb_items", (cmd) => {
      const sub = cmd.arg(0) || "weapons";
      if (sub === "give") {
        const weapon = cmd.arg(1) || CsItem.AK47;
        let n = 0;
        for (const { pawn } of livePawns()) {
          const w = pawn.giveNamedItem(weapon);
          if (w && w.isValid()) n++;
        }
        cmd.reply("[items] gave " + weapon + " to " + n + " player(s)");
      } else if (sub === "strip") {
        let n = 0;
        for (const { pawn } of livePawns()) { if (pawn.stripWeapons()) n++; }
        cmd.reply("[items] stripped " + n + " player(s)");
      } else if (sub === "drop") {
        let n = 0;
        for (const { pawn } of livePawns()) { if (pawn.dropActiveWeapon()) n++; }
        cmd.reply("[items] dropped for " + n + " player(s)");
      } else {
        for (const { slot, pawn } of livePawns()) {
          cmd.reply("[items] " + slot + " has " + pawn.weapons.length + " weapon(s)");
        }
      }
    });

    ctx.commands.register("cb_weapon", (cmd) => {
      const sub = cmd.arg(0) || "info";
      if (sub === "refill") {
        let n = 0;
        for (const { pawn } of livePawns()) {
          const w = pawn.activeWeapon;
          if (w && w.setAmmo(90)) n++;
        }
        cmd.reply("[wpn] refilled clip1=90 on " + n + " active weapon(s)");
      } else if (sub === "disarm") {
        let n = 0;
        for (const { pawn } of livePawns()) { if (pawn.disarm()) n++; }
        cmd.reply("[wpn] disarmed " + n + " player(s)");
      } else if (sub === "nofire") {
        const secs = cmd.args[1] ? Number(cmd.args[1]) : 5;
        const now = Server.gameTime;
        for (const { slot, pawn } of livePawns()) {
          const ok = pawn.blockFiring(secs);
          cmd.reply("[wpn] slot=" + slot + " blockFiring(" + secs + ")=" + ok + " nextAttack=" + pawn.nextAttack + " gameTime=" + now);
        }
      } else {
        for (const { slot, pawn } of livePawns()) {
          const w = pawn.activeWeapon;
          const active = w ? "ref#" + w.ref.index + " clip1=" + w.clip1 + "/" + w.clip2 + " paint=" + w.paintKit : "none";
          cmd.reply("[wpn] slot=" + slot + " active=" + active + " count=" + pawn.weapons.length);
        }
      }
    });
  },
};
