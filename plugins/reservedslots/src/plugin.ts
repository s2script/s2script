// @s2script/reservedslots — SourceMod reservedslots: keep N slots free for players with the reservation
// admin flag (ADMFLAG.RESERVATION).
//
//   Our connect events are notify-only (no reject), so enforcement is admit-then-kick, checked at
//   ONACTIVE — not onConnect: `Client.steamId` reads "0" until Steam auth completes, so at onConnect a
//   reserved admin would be misread as non-reserved and wrongly kicked. By onActive the SteamID is
//   reliable and the client is counted by Player.allConnected().
//
//   A non-reserved client whose arrival pushes the connected count over (maxPlayers - reserved_slots) is
//   kicked, capping non-reserved players at that limit so `reserved_slots` slots always stay open for
//   reserved players (no need to kick existing players). `reserved_slots = 0` disables. The alternate
//   "kick an existing non-reserved player to make room for a connecting reserved one" variant is deferred
//   (needs victim selection / a ping primitive we don't have yet).

import { Clients } from "@s2script/sdk/clients";
import { Server } from "@s2script/sdk/server";
import { Admin, ADMFLAG } from "@s2script/sdk/admin";
import { Player } from "@s2script/cs2";
import { config } from "@s2script/sdk/config";

const KICK_MESSAGE =
  "[SM] This server has reserved slots — you were disconnected to keep a slot open for a reserved player.";

export function onLoad(): void {
  Clients.onActive((c) => {
    const reserved = config.getInt("reserved_slots");
    if (reserved <= 0) return; // disabled
    if (c.isBot) return; // bots are never reservation-gated
    const admin = Admin.forSlot(c.slot);
    if (admin && admin.hasFlags(ADMFLAG.RESERVATION)) return; // reserved player — always allowed
    const max = Server.maxPlayers;
    if (max <= 0) return; // maxPlayers unavailable (degrade) — never kick on bad data
    if (reserved >= max) return; // misconfig: reserved >= capacity would kick everyone — treat as disabled
    if (Player.allConnected().length > max - reserved) {
      c.kick(KICK_MESSAGE);
    }
  });

  console.log(
    "[reservedslots] onLoad — reserved_slots=" + config.getInt("reserved_slots") + " maxPlayers=" + Server.maxPlayers,
  );
}

export function onUnload(): void {
  console.log("[reservedslots] onUnload");
}
