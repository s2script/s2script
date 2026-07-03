import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5C.3 — Vector/QAngle value types. Every ~256 frames, read each in-game player's pawn view
// angles (QAngle) + velocity (Vector) via the generated accessors — copied {x,y,z} snapshots.
//   - pawn.eyeAngles   -> m_angEyeAngles (QAngle) — where the player is aiming.
//   - pawn.absVelocity -> m_vecAbsVelocity (Vector) — .length() is the player's speed.
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (Vector/QAngle)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;
      if (!body) { console.log("  slot=" + p.slot + " (no pawn)"); continue; }
      const ang = body.eyeAngles;                          // generated QAngle | null
      const vel = body.absVelocity;                        // generated Vector | null
      console.log("  slot=" + p.slot
        + " eyeAngles=" + (ang ? ang.toString() : "null")
        + " absVelocity=" + (vel ? vel.toString() : "null")
        + " speed=" + (vel ? vel.length().toFixed(1) : "null"));
    }
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
