// Live-gate demo for @s2script/trace on CS2 2000870 — validates the SDK Ray_t/CGameTrace layouts
// against the running engine. Uses Pawn.forSlot (schema-based, self-healing) not Player.allConnected
// (client-list OFFSETS are stale on 2000870 — a separate offset-treadmill item).
import { Commands } from "@s2script/sdk/commands";
import { Trace } from "@s2script/sdk/trace";
import { Vector } from "@s2script/sdk/math";
import { Pawn } from "@s2script/cs2";

function fmt(v: { x: number; y: number; z: number }): string {
  return "(" + v.x.toFixed(1) + "," + v.y.toFixed(1) + "," + v.z.toFixed(1) + ")";
}

export function onLoad(): void {
  Commands.register("sm_trace", (ctx) => {
    let pawn: ReturnType<typeof Pawn.forSlot> = null;
    for (let slot = 0; slot < 12; slot++) { const pw = Pawn.forSlot(slot); if (pw && pw.origin) { pawn = pw; break; } }
    if (!pawn) { console.log("[trace] no live pawn via forSlot(0..11)"); ctx.reply("no live pawn"); return; }
    const o = pawn.origin!;
    console.log("[trace] pawn origin=" + fmt(o));

    // 1. straight DOWN from 500u above -> must hit the world floor (fraction<1, didHit).
    const down = Trace.line(new Vector(o.x, o.y, o.z + 500), new Vector(o.x, o.y, o.z - 100));
    console.log("[trace] DOWN: didHit=" + down.didHit + " fraction=" + down.fraction.toFixed(3) +
      " endPos=" + fmt(down.endPos) + " normal=" + fmt(down.normal) + " ent=" + (down.entity ? down.entity.index : "null"));

    // 2. straight UP into open sky -> must MISS (didHit=false, fraction=1).
    const up = Trace.line(new Vector(o.x, o.y, o.z + 100), new Vector(o.x, o.y, o.z + 20000));
    console.log("[trace] UP(sky): didHit=" + up.didHit + " fraction=" + up.fraction.toFixed(3));

    // 3. pawn.aimTrace — from the pawn's eyes along its aim.
    const aim = pawn.aimTrace();
    console.log("[trace] AIM: " + (aim ? ("didHit=" + aim.didHit + " endPos=" + fmt(aim.endPos) +
      " normal=" + fmt(aim.normal) + " ent=" + (aim.entity ? aim.entity.index : "null") +
      " fraction=" + aim.fraction.toFixed(3)) : "null"));
    ctx.reply("trace done — see server log");
  });
  console.log("[trace-demo] onLoad — sm_trace registered");
}
export function onUnload(): void {}
