import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5B.4 — player identity through generated string + 64-bit accessors. Every ~256 frames, list
// the in-game players (each a CONTROLLER) and read their identity via the generated
// CCSPlayerController accessors:
//   - player.playerName -> m_iszPlayerName (a char[128] inline buffer) as a string.
//   - player.steamID    -> m_steamID (uint64) as a DECIMAL STRING (SourceMod-parity, wire-safe;
//                          SourcePawn has no 64-bit int, so GetClientAuthId(AuthId_SteamID64) is a string).
//   - player.pawn.health -> the pawn's generated accessor.
// All EntityRef-backed + serial-gated (T|null); Player.all() drops disconnected players (occupancy filter).
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (player identity)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      console.log("  slot=" + p.slot
        + " name=" + JSON.stringify(p.playerName)   // generated: m_iszPlayerName (char[128]) -> string
        + " steamID=" + p.steamID                    // generated: m_steamID (uint64) -> decimal string
        + " health=" + (p.pawn ? p.pawn.health : "none"));
    }
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
