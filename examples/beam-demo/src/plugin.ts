import { OnGameFrame } from "@s2script/sdk/frame";
import { Pawn, Beam, BeamHandle } from "@s2script/cs2";
import { Commands } from "@s2script/sdk/commands";
import { Vector } from "@s2script/sdk/math";
import { delay } from "@s2script/sdk/timers";

const IN_USE = 32;                       // the E key bit
const state = new Map<number, { held: boolean; beam: BeamHandle | null }>();

function eyeOf(pawn: Pawn): Vector | null {
  const sn = pawn.sceneNode;
  const o = sn && sn.absOrigin;
  return o ? new Vector(o.x, o.y, o.z + 64) : null;   // standing eye height
}

function clearBeam(slot: number): void {
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
        if (s.beam) s.beam.update(eye, hit.endPos);
        else s.beam = Beam.draw(eye, hit.endPos, { color: [255, 0, 0, 255], width: 2 });
      }
    } else if (s.beam) {
      clearBeam(slot);
    }
    s.held = held;
  }
});

// Bot-provable rcon check: create a static beam at fixed coords and report the EntityRef validity.
Commands.register("sm_beam", (ctx) => {
  const start = new Vector(0, 0, 100), end = new Vector(200, 0, 100);
  const h = Beam.draw(start, end, { color: [0, 255, 0, 255], width: 3 });
  if (!h) { ctx.reply("[beam] createEntity FAILED"); return; }
  ctx.reply("[beam] drawn ref valid=" + h.ref.isValid() + " index=" + h.ref.index);
  // leave it for 3s then remove (prove teleport/remove too)
  delay(3000).then(() => {
    ctx.reply("[beam] remove -> " + h.remove());
  });
});

export function onUnload(): void {
  for (const slot of state.keys()) clearBeam(slot);
  state.clear();
}
