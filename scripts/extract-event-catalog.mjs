#!/usr/bin/env node
// Extract the CS2 game-event catalog from CounterStrikeSharp's generated event stubs.
//
// CS2 buries the real event definitions in the VPK (no live dump), so we borrow CSSharp's
// maintained Generated/GameEvents/Event*.g.cs — events are game CONTENT (stable across binary
// patches), and this catalog is IntelliSense-only (the runtime is a generic bus), so a stale
// field is at worst one untyped getInt call, never a crash.
//
// Usage:  node scripts/extract-event-catalog.mjs <path-to-CSSharp-repo> > games/cs2/gamedata/event-catalog.json
// Source ref is recorded in docs/superpowers/specs/2026-07-09-event-catalog-parity-design.md.
//
// Accessor -> catalog type -> eventgen getter:
//   Get<bool>                 bool     getBool
//   Get<int>                  int      getInt
//   Get<float>                float    getFloat
//   Get<string>               string   getString
//   Get<long> / Get<ulong>    uint64   getUint64 (-> decimal string)
//   GetPlayer / GetPlayerPawn player   getPlayerSlot
//   Get<IntPtr> / other       (skipped with a logged warning — no unmapped types in the .d.ts)

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const cssharp = process.argv[2];
if (!cssharp) {
  console.error("usage: extract-event-catalog.mjs <CSSharp-repo-path>");
  process.exit(1);
}
const dir = join(cssharp, "managed/CounterStrikeSharp.API/Generated/GameEvents");

const TYPEMAP = { int: "int", string: "string", float: "float", bool: "bool", long: "uint64", ulong: "uint64" };

const files = readdirSync(dir).filter((f) => /^Event.*\.g\.cs$/.test(f));
const catalog = {};
const skipped = [];

for (const f of files) {
  const src = readFileSync(join(dir, f), "utf8");
  const nameM = src.match(/\[EventName\("([^"]+)"\)\]/);
  if (!nameM) { skipped.push(`${f}: no [EventName]`); continue; }
  const event = nameM[1];
  const fields = {};
  // GetPlayer("x") / GetPlayerPawn("x") -> player. (SetPlayer won't match.)
  for (const m of src.matchAll(/\bGetPlayer(?:Pawn)?\("([^"]+)"\)/g)) fields[m[1]] = "player";
  // Get<T>("x") -> mapped type. (Set<T> won't match.)
  for (const m of src.matchAll(/\bGet<([A-Za-z0-9_]+)>\("([^"]+)"\)/g)) {
    const t = TYPEMAP[m[1]];
    if (t) { if (!(m[2] in fields)) fields[m[2]] = t; }
    else skipped.push(`${event}.${m[2]}: Get<${m[1]}> (unmapped -> skipped)`);
  }
  catalog[event] = fields;
}

// Emit sorted, one line per event (matches the seed file's style; greppable).
const events = Object.keys(catalog).sort();
let out = "{\n";
out += events
  .map((ev) => {
    const keys = Object.keys(catalog[ev]).sort();
    const inner = keys.map((k) => `${JSON.stringify(k)}: ${JSON.stringify(catalog[ev][k])}`).join(", ");
    return `  ${JSON.stringify(ev)}: {${inner ? ` ${inner} ` : ""}}`;
  })
  .join(",\n");
out += "\n}\n";
process.stdout.write(out);

console.error(`[extract] ${events.length} events; ${skipped.length} field(s)/file(s) skipped` + (skipped.length ? ":\n" + skipped.join("\n") : ""));
