import { watchOptional } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

/**
 * Consuming a protocol-2 PLUGIN interface. Enable interfaceProtocol: 2 and obtain
 * the verified declaration with s2s add @s2script/zones. Move its generated entry
 * from pluginDependencies into optionalPluginDependencies in package.json.
 * The CLI downloads types only; the operator installs the provider archive
 * separately. In this repository the checked-in
 * .s2script/types/@s2script/zones/index.d.ts is a byte-copy of plugins/zones/api.d.ts.
 *
 * A hard dependency can instead import { on, getZones } from "@s2script/zones".
 * Optional watches attach after both plugins are Active and attach again after a
 * compatible reload. Register synchronously through the supplied service and own
 * subscriptions with the attachment scope; methods on retired services throw.
 */
export const name = "zones";
export const describe = "react to zone enter/leave/stay/created/deleted from the zones plugin (optional dep)";

export function OnPluginStart(): void {
  watchOptional("@s2script/zones", (zones, scope) => {
    scope.own(zones.on("enter", (p) => {
      const name = Player.fromUserId(p.userId)?.playerName ?? `userId ${p.userId}`;
      console.log(`[cookbook] ENTER ${p.zone}: ${name}`);
    }));
    scope.own(zones.on("leave", (p) => {
      const name = Player.fromUserId(p.userId)?.playerName ?? `userId ${p.userId}`;
      console.log(`[cookbook] LEAVE ${p.zone}: ${name}`);
    }));
    // Every eighth game frame while inside: resolve the connection for each event.
    // Never heal a replacement player just because it reused the copied slot.
    scope.own(zones.on("stay", (p) => {
      if (p.zone !== "heal") return;
      const player = Player.fromUserId(p.userId);
      const pawn = player?.pawn;
      if (pawn && pawn.health != null && pawn.health < 100) {
        const nh = Math.min(100, pawn.health + 1);
        pawn.health = nh;
        if (nh % 20 === 0 || nh === 100) {
          console.log(`[cookbook] healed userId ${p.userId} -> ${nh}`);
        }
      }
    }));
    scope.own(zones.on("created", (p) => {
      console.log(`[cookbook] CREATED ${p.zone} tags=[${p.tags.join(",")}]`);
    }));
    scope.own(zones.on("deleted", (p) => {
      console.log(`[cookbook] DELETED ${p.zone}`);
    }));
    // Notifications are changes, not history. Query the current layout after
    // subscribing on EVERY attachment, including a compatible provider reload.
    for (const zone of zones.getZones()) {
      console.log(`[cookbook] EXISTING ${zone.name} tags=[${zone.tags.join(",")}]`);
    }
  });
}
