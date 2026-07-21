import { plugin } from "@s2script/sdk/plugin";
import type { Ent } from "../../entref-producer/api";

// Consumer: hard-deps @demo/ent (a proxy that throws while the producer is unloaded). On a tick it
// calls pawnRef(0) — a LIVE EntityRef received ACROSS the plugin boundary — and logs whether it
// still validates against the SHARED entity system. ref.isValid() is TRUE while the pawn lives and
// FALSE once it is destroyed: that flip is the cross-plugin host-invalidation proof, and it is
// OFFSET-FREE (the consumer never resolves a schema offset — pawnHealth is the producer's read).
export default plugin((ctx) => {
  console.log("[consumer] onLoad");
  const ent = ctx.use<Ent>("@demo/ent");
  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 !== 0) return;
    try {
      const ref = ent.pawnRef(0);                         // a LIVE EntityRef received across the wire
      // ref.isValid() validates against the SHARED entity system: TRUE while the pawn lives, FALSE
      // once it is destroyed — the cross-plugin host-invalidation proof (offset-free, no schema on
      // the consumer side). `pawnHealth(0)` shows a real number while alive.
      const alive = ref ? ref.isValid() : false;
      console.log("[consumer] tick " + ticks + " received-ref valid=" + alive
        + " health=" + (alive ? ent.pawnHealth(0) : "null"));
    } catch (e) { console.log("[consumer] failed (degraded): " + String(e)); }
  });
});
