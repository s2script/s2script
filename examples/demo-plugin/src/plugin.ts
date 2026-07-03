import { Player, Events } from "@s2script/cs2";

// Slice 5D.2 live gate:
//  A) game events deliver (sig-scanned IGameEventManager2): subscribe to round_start + player_spawn.
//  B) engine identity: Player.allConnected() (pawnless-safe), player.userId, Player.fromUserId round-trip.
export function onLoad(): void {
  console.log("[demo] onLoad (5D.2 events + identity)");

  Events.on("round_start", (ev) => {
    console.log("[demo] EVENT round_start timelimit=" + ev.getInt("timelimit"));
    reportPlayers();
  });
  Events.on("player_spawn", (ev) => {
    const slot = ev.getPlayerSlot("userid");
    console.log("[demo] EVENT player_spawn slot=" + slot);
  });
}

function reportPlayers(): void {
  const conn = Player.allConnected();
  console.log("[demo] allConnected=" + conn.length);
  for (const p of conn) {
    const uid = p.userId;
    const back = Player.fromUserId(uid);
    console.log("  slot=" + p.slot + " userId=" + uid
      + " teamNum=" + p.teamNum
      + " pawn=" + (p.pawn ? "yes" : "none")
      + " fromUserId(uid).slot=" + (back ? back.slot : "null"));
  }
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
