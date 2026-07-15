// s2bench — mirror of the swiftly-solution/profiler op-catalog in idiomatic s2script.
// DIRECT per-op timing via __s2_hrtime_ns (the Stopwatch analog): batch ops are timed as a single
// 1024-loop (exactly like the profiler), per-call ops are timed individually with min/avg/max and a
// subtracted timer baseline. NO loop-amortization multipliers. Throwaway informational tool.
// Run: `s2bench` (rcon/console).

import { Commands } from "@s2script/commands";
import { createEntity, Entity, EntityRef } from "@s2script/entity";
import { GameRules } from "@s2script/cs2";
import { Server } from "@s2script/server";
import { Events } from "@s2script/events";
import { UserMessage } from "@s2script/usermessages";

// Internal natives (dev instrumentation) — declared ambiently, probed with typeof at runtime.
declare const __s2_schema_offset: ((cls: string, field: string) => number | null) | undefined;
declare const __s2_v8_heap_used: (() => number) | undefined;
declare const __s2_v8_gc: (() => void) | undefined;
declare const __s2_hrtime_ns: (() => number) | undefined;

const ENT = 1024;      // entity batch size, matching the profiler
const SAMPLE = 4000;   // per-call sample count for min/avg/max (each timed individually)

const FALLBACK = { m_messageText: 2628, m_bEnabled: 3268, m_flFontSize: 3276, m_Color: 3300 };

let running = false;
let sink = 0;

function offOf(cls: string, field: string, fallback: number): number {
  if (typeof __s2_schema_offset !== "undefined" && __s2_schema_offset) {
    const o = __s2_schema_offset(cls, field);
    if (o !== null && o !== undefined) return o;
  }
  return fallback;
}
function heapMB(): number | null {
  if (typeof __s2_v8_heap_used === "undefined" || !__s2_v8_heap_used) return null;
  const b = __s2_v8_heap_used();
  return b >= 0 ? b / 1048576 : null;
}
function forceGC(): void { if (typeof __s2_v8_gc !== "undefined" && __s2_v8_gc) __s2_v8_gc(); }
function fmt(x: number | null): string { return x === null ? "n/a" : x.toFixed(3); }
function hrNs(): number { return __s2_hrtime_ns!(); }

// Timer baseline: the floor cost of one __s2_hrtime_ns() FFI call (min of many back-to-back deltas).
// Subtracted from per-call op measurements so we report the op, not the op+timer.
function calibrateTimer(): number {
  let min = Infinity;
  for (let i = 0; i < 20000; i++) { const a = hrNs(); const b = hrNs(); const d = b - a; if (d > 0 && d < min) min = d; }
  return min === Infinity ? 0 : min;
}

// Batch op: time the WHOLE n-item loop in ONE bracket (the profiler's exact methodology). One shot.
function timeBatch(name: string, n: number, fn: () => void): void {
  const t0 = hrNs(); fn(); const t1 = hrNs();
  const totalNs = t1 - t0;
  console.log(`[S2BENCH] ${name} | total=${(totalNs / 1e6).toFixed(3)}ms | per_op=${(totalNs / n).toFixed(1)}ns | n=${n} (single-shot)`);
}

// Per-call op: time EACH call individually, report min/avg/max (baseline-subtracted). No amortization.
function timeEach(name: string, iters: number, fn: () => void, baselineNs: number, note?: string): void {
  let min = Infinity, max = 0, sum = 0;
  for (let i = 0; i < iters; i++) {
    const a = hrNs(); fn(); const b = hrNs();
    let d = (b - a) - baselineNs; if (d < 0) d = 0;
    if (d < min) min = d; if (d > max) max = d; sum += d;
  }
  const avg = sum / iters;
  console.log(`[S2BENCH] ${name} | min=${(min / 1000).toFixed(4)}us avg=${(avg / 1000).toFixed(4)}us max=${(max / 1000).toFixed(4)}us | n=${iters}` + (note ? ` | ${note}` : ""));
}

function runBench(reply: (m: string) => void): void {
  if (running) { reply("[s2bench] already running"); return; }
  running = true;

  if (typeof __s2_hrtime_ns === "undefined" || !__s2_hrtime_ns) {
    reply("[s2bench] __s2_hrtime_ns unavailable (old core) — cannot do direct timing");
    running = false; return;
  }
  const baseline = calibrateTimer();
  console.log(`[S2BENCH] === run start (ENT=${ENT}) direct-timing; timer baseline=${baseline.toFixed(1)}ns/call ===`);

  // --- Memory phase (V8 heap before / 1024 alive / after GC / after release) ---
  if (heapMB() !== null) {
    forceGC();
    const hBefore = heapMB();
    const mem: EntityRef[] = [];
    for (let i = 0; i < ENT; i++) { const e = createEntity("point_worldtext"); if (e) { e.spawn(); mem.push(e); } }
    const mo = offOf("CPointWorldText", "m_messageText", FALLBACK.m_messageText);
    const co = offOf("CPointWorldText", "m_Color", FALLBACK.m_Color);
    for (let i = 0; i < mem.length; i++) { mem[i].readString(mo, 512); mem[i].writeUInt32(co, 0xff0000ff); mem[i].notifyStateChanged(co); }
    const hAfter = heapMB();
    forceGC();
    const hAfterGC = heapMB();
    for (let i = 0; i < mem.length; i++) mem[i].remove();
    forceGC();
    const hReleased = heapMB();
    console.log(`[S2BENCH] MEM (V8 heap MB) before=${fmt(hBefore)} | with_1024_alive=${fmt(hAfter)} | after_GC=${fmt(hAfterGC)} | after_release+GC=${fmt(hReleased)}`);
  }

  // --- Batch ops (single-shot 1024-loop, profiler-identical) ---
  const ents: EntityRef[] = [];
  timeBatch("Create 1024 entities", ENT, () => {
    for (let i = 0; i < ENT; i++) { const e = createEntity("point_worldtext"); if (e) ents.push(e); }
  });
  timeBatch("Spawn 1024 entities", ents.length, () => { for (let i = 0; i < ents.length; i++) ents[i].spawn(); });

  const mo = offOf("CPointWorldText", "m_messageText", FALLBACK.m_messageText);
  const co = offOf("CPointWorldText", "m_Color", FALLBACK.m_Color);
  const eo = offOf("CPointWorldText", "m_bEnabled", FALLBACK.m_bEnabled);
  timeBatch("Schema Write + Update (1024)", ents.length, () => {
    for (let i = 0; i < ents.length; i++) {
      ents[i].writeString(offOf("CPointWorldText", "m_messageText", mo), 512, "s2bench");
      ents[i].notifyStateChanged(mo);
      ents[i].writeUInt32(offOf("CPointWorldText", "m_Color", co), 0xff0000ff);
      ents[i].notifyStateChanged(co);
    }
  });
  timeBatch("Schema Read (1024)", ents.length, () => {
    for (let i = 0; i < ents.length; i++) {
      ents[i].readString(offOf("CPointWorldText", "m_messageText", mo), 512);
      ents[i].readUInt32(offOf("CPointWorldText", "m_Color", co));
      ents[i].readBool(offOf("CPointWorldText", "m_bEnabled", eo));
    }
  });
  timeBatch("Virtual Calls (teleport 1024)", ents.length, () => { for (let i = 0; i < ents.length; i++) ents[i].teleport([0, i * 10, 0]); });

  // --- Per-call ops (direct, min/avg/max, baseline-subtracted) ---
  timeEach("Get Game Rules (framework get())", SAMPLE, () => { const g = GameRules.get(); if (g) sink += (g.totalRoundsPlayed ?? 0); }, baseline, "get() is now internally cached");
  timeEach("findByClass('cs_gamerules') raw scan", SAMPLE, () => { sink += Entity.findByClass("cs_gamerules").length; }, baseline, "the un-cached scan get() used to do");
  timeEach("Read ConVar (sv_gravity)", SAMPLE, () => { Server.getCvar("sv_gravity"); }, baseline);
  timeEach("Fire Game Event to slot0", SAMPLE, () => { Events.fireToClient(0, "show_survival_respawn_status", { loc_token: "s2bench", duration: 1 }); }, baseline, "slot0 bot — unrepresentative if it misses");
  timeEach("Send UserMessage (Fade) to all", SAMPLE, () => { new UserMessage("Fade").setInt("duration", 512).setInt("hold_time", 0).setInt("flags", 0x0009).setInt("color", 0).sendAll(); }, baseline, "bots skipped at send");

  timeBatch("Despawn 1024 entities", ents.length, () => { for (let i = 0; i < ents.length; i++) ents[i].remove(); });

  console.log(`[S2BENCH] === run complete (sink=${sink}) ===`);
  reply(`[s2bench] done (direct timing, baseline=${baseline.toFixed(0)}ns) — grep [S2BENCH]`);
  running = false;
}

export function onLoad(): void {
  Commands.register("s2bench", (ctx) => { runBench((m) => ctx.reply(m)); });
  console.log("[s2bench] onLoad — run `s2bench` (direct per-op timing)");
}
