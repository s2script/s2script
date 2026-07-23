import type { Recipe } from "../recipe.ts";
import type { Zones, ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "@s2script/zones";
import { Player } from "@s2script/cs2";

/**
 * Consuming another PLUGIN's interface (not an SDK module). @s2script/zones is
 * published by plugins/zones. Declared under optionalPluginDependencies, so
 * tryUse() returns null when that plugin isn't loaded and the cookbook still
 * works — a hard dep (ctx.use) would refuse to load the whole plugin instead.
 *
 * Types come from the verified contract copy at
 * .s2script/types/@s2script/zones/index.d.ts — a byte-copy of the producer's
 * api.d.ts that s2s build hashes into manifest.compiledAgainst, so a drifted
 * contract is refused at load rather than marshalled across. Refresh with:
 *   cp plugins/zones/api.d.ts examples/cookbook/.s2script/types/@s2script/zones/index.d.ts
 */
export const zonesRecipe: Recipe = {
  name: "zones",
  describe: "react to zone enter/leave/stay/created/deleted from the zones plugin (optional dep)",
  register(ctx) {
    const zones = ctx.tryUse<Zones>("@s2script/zones");
    if (!zones) {
      console.log("[cookbook] zones recipe idle — @s2script/zones is not loaded");
      return;
    }
    zones.on("enter", (p: ZoneEvent) => {
      const name = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
      console.log(`[cookbook] ENTER ${p.zone}: ${name}`);
    });
    zones.on("leave", (p: ZoneEvent) => {
      const name = Player.fromSlot(p.slot)?.playerName ?? `slot ${p.slot}`;
      console.log(`[cookbook] LEAVE ${p.zone}: ${name}`);
    });
    // `stay` fires every tick a player is inside a zone — cheap continuous work
    // like this heal-over-time on a zone named "heal" belongs here, not in a
    // command. Capped at 100 and logged only every 20hp to avoid a log line
    // per tick per player.
    zones.on("stay", (p: ZoneEvent) => {
      if (p.zone !== "heal") return;
      const pawn = Player.fromSlot(p.slot)?.pawn;
      if (pawn && pawn.health != null && pawn.health < 100) {
        const nh = Math.min(100, pawn.health + 1);
        pawn.health = nh;
        if (nh % 20 === 0 || nh === 100) {
          console.log(`[cookbook] healed slot ${p.slot} -> ${nh}`);
        }
      }
    });
    // `created`/`deleted` fire when zones are added or removed at runtime —
    // via createZone/deleteZone, the sm_zone_* commands, or the editor — so a
    // consumer can react to the zone layout changing without polling getZones().
    zones.on("created", (p: ZoneCreatedEvent) => {
      console.log(`[cookbook] CREATED ${p.zone} tags=[${p.tags.join(",")}]`);
    });
    zones.on("deleted", (p: ZoneDeletedEvent) => {
      console.log(`[cookbook] DELETED ${p.zone}`);
    });
  },
};
