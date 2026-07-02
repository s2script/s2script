import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5C.2 — the player model. Every ~256 frames, iterate the in-game players (each a CONTROLLER,
// the persistent player identity) and demonstrate the honest controller/pawn split:
//   - Player.all()      -> the in-game players (controllers with a live pawn).
//   - player.teamNum    -> a generated CCSPlayerController accessor (the persistent identity).
//   - player.pawn       -> the in-world body (Pawn|null), via the m_hPlayerPawn handle.
//   - pawn.controller   -> the reverse hop back to the Player (m_hController) — round-trips to the same slot.
// All EntityRef-backed + serial-gated (T|null); a stored Player degrades to null on reuse/disconnect.
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (player model)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;                             // controller -> pawn navigation
      const back = body ? body.controller : null;      // pawn -> controller round-trip
      console.log("  slot=" + p.slot
        + " teamNum=" + p.teamNum                       // generated CCSPlayerController accessor
        + " health=" + (body ? body.health : "none")    // .pawn -> generated CCSPlayerPawn accessor
        + " backSlot=" + (back ? back.slot : "null"));   // reverse hop resolves to the same slot
    }
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
