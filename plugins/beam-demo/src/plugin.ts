import { OnGameFrame } from "@s2script/frame";
import { Pawn, Beam, BeamHandle } from "@s2script/cs2";
import { Commands } from "@s2script/commands";
import { createEntity } from "@s2script/entity";

const IN_USE = 32;                       // the E key bit
const state = new Map<number, { held: boolean; beam: BeamHandle | null }>();

function eyeOf(pawn: any): { x: number; y: number; z: number } | null {
  const sn = pawn.sceneNode;
  const o = sn && sn.absOrigin;
  return o ? { x: o.x, y: o.y, z: o.z + 64 } : null;   // standing eye height
}

function clearBeam(slot: number) {
  const s = state.get(slot);
  if (s && s.beam) { s.beam.remove(); s.beam = null; }
}

OnGameFrame.subscribe(() => {
  for (let slot = 0; slot < 12; slot++) {
    const pawn = Pawn.forSlot(slot);
    if (!pawn) { clearBeam(slot); state.delete(slot); continue; }
    let s = state.get(slot);
    if (!s) { s = { held: false, beam: null }; state.set(slot, s); }
    const held = (pawn.buttons & IN_USE) !== 0;
    if (held) {
      const eye = eyeOf(pawn);
      const hit = pawn.aimTrace();
      if (eye && hit) {
        if (s.beam) s.beam.update(eye as any, hit.endPos as any);
        else s.beam = Beam.draw(eye as any, hit.endPos as any, { color: [255, 0, 0, 255], width: 2 });
      }
    } else if (s.beam) {
      clearBeam(slot);
    }
    s.held = held;
  }
});

// Bot-provable rcon check: create a static beam at fixed coords and report the EntityRef validity.
Commands.register("sm_beam", (ctx) => {
  const start = { x: 0, y: 0, z: 100 }, end = { x: 200, y: 0, z: 100 };
  const h = Beam.draw(start as any, end as any, { color: [0, 255, 0, 255], width: 3 });
  if (!h) { ctx.reply("[beam] createEntity FAILED"); return; }
  ctx.reply("[beam] drawn ref valid=" + h.ref.isValid() + " index=" + (h.ref as any).index);
  // leave it for 3s then remove (prove teleport/remove too)
  (globalThis as any).__s2pkg_timers.delay(3000).then(() => {
    ctx.reply("[beam] remove -> " + h.remove());
  });
});

export function onUnload() {
  for (const slot of state.keys()) clearBeam(slot);
  state.clear();
}
