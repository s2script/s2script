import type { Recipe } from "../recipe.ts";
import type { Zones, ZoneEvent } from "@s2script/zones";
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
  describe: "react to zone enter/leave from the zones plugin (optional dep)",
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
  },
};
