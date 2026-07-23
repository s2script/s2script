import type { Recipe } from "../recipe.ts";
import { Trace } from "@s2script/sdk/trace";
import { Vector } from "@s2script/sdk/math";
import { Pawn } from "@s2script/cs2";

function fmt(v: { x: number; y: number; z: number }): string {
  return "(" + v.x.toFixed(1) + "," + v.y.toFixed(1) + "," + v.z.toFixed(1) + ")";
}

/**
 * Trace.line() ray-casts against the Source 2 physics scene and returns a
 * CGameTrace-shaped result: didHit, fraction, endPos, normal, and the entity
 * hit (if any). pawn.aimTrace() does the same from a pawn's eyes along its
 * aim. Uses Pawn.forSlot rather than Player.allConnected — pawn lookups here
 * are schema-based and self-healing across engine updates.
 */
export const traceRecipe: Recipe = {
  name: "trace",
  describe: "ray-cast down, up, and along a pawn's aim (sm_trace)",
  register(ctx) {
    ctx.commands.register("sm_trace", (cmd) => {
      let pawn: ReturnType<typeof Pawn.forSlot> = null;
      for (let slot = 0; slot < 12; slot++) {
        const pw = Pawn.forSlot(slot);
        if (pw && pw.origin) { pawn = pw; break; }
      }
      if (!pawn) { console.log("[cookbook] trace: no live pawn via forSlot(0..11)"); cmd.reply("no live pawn"); return; }
      const o = pawn.origin!;
      console.log("[cookbook] trace: pawn origin=" + fmt(o));

      // 1. straight DOWN from 500u above -> must hit the world floor (fraction<1, didHit).
      const down = Trace.line(new Vector(o.x, o.y, o.z + 500), new Vector(o.x, o.y, o.z - 100));
      console.log("[cookbook] trace DOWN: didHit=" + down.didHit + " fraction=" + down.fraction.toFixed(3) +
        " endPos=" + fmt(down.endPos) + " normal=" + fmt(down.normal) + " ent=" + (down.entity ? down.entity.index : "null"));

      // 2. straight UP into open sky -> must MISS (didHit=false, fraction=1).
      const up = Trace.line(new Vector(o.x, o.y, o.z + 100), new Vector(o.x, o.y, o.z + 20000));
      console.log("[cookbook] trace UP(sky): didHit=" + up.didHit + " fraction=" + up.fraction.toFixed(3));

      // 3. pawn.aimTrace — from the pawn's eyes along its aim.
      const aim = pawn.aimTrace();
      console.log("[cookbook] trace AIM: " + (aim ? ("didHit=" + aim.didHit + " endPos=" + fmt(aim.endPos) +
        " normal=" + fmt(aim.normal) + " ent=" + (aim.entity ? aim.entity.index : "null") +
        " fraction=" + aim.fraction.toFixed(3)) : "null"));
      cmd.reply("trace done — see server log");
    });
  },
};
