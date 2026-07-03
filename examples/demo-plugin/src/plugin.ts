import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5C.5 — curated pointer-chain wrappers. Every ~256 frames, read fields that live BEHIND pointer
// chains, through the generated typed wrappers (each re-resolves the chain serial-gated at the root):
//   - pawn.sceneNode.absOrigin / .scale  (entity -> CBodyComponent -> CGameSceneNode)
//   - pawn.weaponServices.activeWeapon   (-> CCSPlayer_WeaponServices; a handle -> EntityRef)
//   - pawn.movementServices.ducked       (-> CCSPlayer_MovementServices)
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (ptr-nav wrappers)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;
      if (!body) { console.log("  slot=" + p.slot + " (no pawn)"); continue; }
      const sn = body.sceneNode;
      const ws = body.weaponServices;
      const mv = body.movementServices;
      console.log("  slot=" + p.slot
        + " absOrigin=" + (sn && sn.absOrigin ? sn.absOrigin.toString() : "null")
        + " scale=" + (sn ? sn.scale : "null")
        + " activeWeapon=" + (ws && ws.activeWeapon ? ("ref#" + ws.activeWeapon.index) : "null")
        + " ducked=" + (mv ? mv.ducked : "null")
        + " origin(alias)=" + (body.origin ? "ok" : "null"));
    }
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
