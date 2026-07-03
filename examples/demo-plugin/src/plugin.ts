import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5C.4 — pointer-chain nav. Every ~256 frames, read each in-game player's pawn world position +
// rotation via the generated pointer-chain accessors (entity -> CBodyComponent -> CGameSceneNode).
//   - pawn.origin -> m_vecAbsOrigin (Vector) — world position.
//   - pawn.angles -> m_angAbsRotation (QAngle) — body rotation (distinct from eyeAngles = view/aim).
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (origin/angles pointer-chain)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;
      if (!body) { console.log("  slot=" + p.slot + " (no pawn)"); continue; }
      const o = body.origin;                              // Vector | null (world position)
      const a = body.angles;                              // QAngle | null (body rotation)
      console.log("  slot=" + p.slot
        + " origin=" + (o ? o.toString() : "null")
        + " angles=" + (a ? a.toString() : "null"));
    }
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
