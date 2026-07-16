// Live-gate demo for CEntityKeyValues-configured spawn: createEntity(className, keyvalues) builds a
// CEntityKeyValues shim-side and DispatchSpawns the entity with it, so the entity's OWN Spawn() parses
// the keys — proven both by reading the parsed fields back through the live schema (string/bool/float)
// AND behaviorally (an int-tagged keyvalue drives the entity's own logic to fire an output). No human
// client needed.
import { Commands } from "@s2script/sdk/commands";
import { createEntity, Entity, EntityRef } from "@s2script/sdk/entity";

const off = (cls: string, field: string): number =>
  (globalThis as any).__s2_schema_offset(cls, field);
const delay = (ms: number): Promise<void> => (globalThis as any).__s2pkg_timers.delay(ms);

// Behavioral proof arm: OnHitMax only fires if the kv-configured startvalue(5)+Add(5) reaches
// the kv-configured max(10) — the INT keyvalue path proven through the entity's own logic.
Entity.onOutput("math_counter", "OnHitMax", () => {
  console.log("[ekv] OnHitMax fired — startvalue/max keyvalues took effect (int path PROVEN)");
});

Commands.register("sm_ekv", (ctx) => {
  // 1. STRING + BOOL read-back: point_worldtext parses "message"/"fullbright" in its Spawn.
  const wt = createEntity("point_worldtext", { message: "s2-ekv-proof", enabled: true, fullbright: true });
  if (!wt) { ctx.reply("[ekv] point_worldtext createEntity(kv) FAILED"); }
  else {
    const msg = wt.readString(off("CPointWorldText", "m_messageText"), 512);
    const fb = wt.readBool(off("CPointWorldText", "m_bFullbright"));
    ctx.reply("[ekv] worldtext message=" + JSON.stringify(msg) + " (want \"s2-ekv-proof\") fullbright=" + fb);
  }

  // 2. FLOAT read-back (from int-tagged kv — engine-side KV3 coercion) + INT behavioral.
  const mc = createEntity("math_counter", { startvalue: 5, min: 1, max: 10 });
  if (!mc) { ctx.reply("[ekv] math_counter createEntity(kv) FAILED"); }
  else {
    const mx = mc.readFloat32(off("CMathCounter", "m_flMax"));
    const mn = mc.readFloat32(off("CMathCounter", "m_flMin"));
    ctx.reply("[ekv] counter min=" + mn + " max=" + mx + " (want 1/10); firing Add 5 -> expect OnHitMax");
    mc.acceptInput("Add", "5");   // 5 (kv startvalue) + 5 = 10 (kv max) -> OnHitMax next tick
  }

  // 3. Cleanup after 3s (prove remove; keep the world clean).
  delay(3000).then(() => {
    const r1 = wt ? wt.remove() : false;
    const r2 = mc ? mc.remove() : false;
    ctx.reply("[ekv] cleanup remove -> " + r1 + "/" + r2);
  });
});

// Marshal-rejection sanity (loud, no engine call): bad value type must fail closed.
Commands.register("sm_ekv_bad", (ctx) => {
  const e = createEntity("logic_relay", { nested: { a: 1 } as any });
  ctx.reply("[ekv] bad-kv createEntity -> " + e + " (want null)");
});

export function onLoad(): void {
  console.log("[ekv-demo] onLoad — sm_ekv/sm_ekv_bad registered");
}
export function onUnload(): void {}
