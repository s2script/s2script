# Zones sub-slice 2 — persistence + per-map CRUD — Implementation Plan

> **Execution:** controller-authored (one cohesive game-layer plugin, no engine change → no sniper, hot-reloadable). Live-gated on bots via rcon.

**Goal:** `plugins/zones` (`@s2script/zones`) — DB-backed named per-map zones with JSON export/import, per-map load on `Server.onMapStart`, operator CRUD, driving the sub-slice-1 origin poll (`ENTER`/`LEAVE <name>`).

**Architecture:** In-memory registry (hot path for the poll) mirrored to a SQLite `zones` table (durability) + JSON files (portability). `Server.onMapStart` reloads the registry per map.

## Global Constraints

- Game-layer only; no core/shim. Both boundary gates trivially green. Full-strict typecheck must pass.
- Async DB (Promise). `db` is published only after `CREATE TABLE` resolves; guard every path that touches it.
- git commit `-F -` heredoc; Claude-Session trailer.

## File Structure

- `plugins/zones/package.json` — `@s2script/zones`, deps `@s2script/db`/`server`/`config`/`commands`/`admin`/`cs2`/`frame`.
- `plugins/zones/tsconfig.json` — extends base.
- `plugins/zones/src/plugin.ts` — the manager.
- Retire `examples/zones-spike/` (superseded; `git rm`).

## Task 1: The zones manager plugin

- [ ] **Step 1: scaffold** — `package.json` (mirror an existing plugin) + `tsconfig.json`.

- [ ] **Step 2: `src/plugin.ts`** — the full manager:

```ts
import { Commands, CommandContext } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Database } from "@s2script/db";
import { Server } from "@s2script/server";
import { config } from "@s2script/config";
import { OnGameFrame } from "@s2script/frame";
import { Player, Pawn } from "@s2script/cs2";

interface Vec3 { x: number; y: number; z: number; }
interface Zone { name: string; min: Vec3; max: Vec3; inside: Set<number>; }

let db: Database | null = null;
let currentMap = "";
const zones = new Map<string, Zone>();

function sanitizeName(n: string): string { return (n || "").replace(/[^A-Za-z0-9_-]/g, "").slice(0, 64); }
function normBox(a: Vec3, b: Vec3): { min: Vec3; max: Vec3 } {
  return {
    min: { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), z: Math.min(a.z, b.z) },
    max: { x: Math.max(a.x, b.x), y: Math.max(a.y, b.y), z: Math.max(a.z, b.z) },
  };
}
function contains(z: Zone, x: number, y: number, z2: number): boolean {
  return x >= z.min.x && x <= z.max.x && y >= z.min.y && y <= z.max.y && z2 >= z.min.z && z2 <= z.max.z;
}

async function loadMap(map: string): Promise<void> {
  currentMap = map;
  zones.clear();
  if (!db) return;
  const rows = await db.query("SELECT name, minX, minY, minZ, maxX, maxY, maxZ FROM zones WHERE map = ?", [map]);
  for (const r of rows) {
    const name = String(r.name);
    zones.set(name, {
      name,
      min: { x: Number(r.minX), y: Number(r.minY), z: Number(r.minZ) },
      max: { x: Number(r.maxX), y: Number(r.maxY), z: Number(r.maxZ) },
      inside: new Set<number>(),
    });
  }
  console.log(`[zones] loaded ${zones.size} zone(s) for ${map}`);
}

async function upsertZone(name: string, box: { min: Vec3; max: Vec3 }): Promise<void> {
  zones.set(name, { name, min: box.min, max: box.max, inside: new Set<number>() });
  if (db) await db.execute(
    "INSERT OR REPLACE INTO zones (map, name, minX, minY, minZ, maxX, maxY, maxZ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    [currentMap, name, box.min.x, box.min.y, box.min.z, box.max.x, box.max.y, box.max.z]);
}

export function onLoad(): void {
  (async () => {
    db = await Database.open("zones");
    await db.execute(
      "CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, PRIMARY KEY (map, name))");
    await loadMap(Server.mapName);
    console.log(`[zones] onLoad — DB ready`);
  })().catch((e) => console.log(`[zones] init error: ${e}`));

  Server.onMapStart((map) => { loadMap(map).catch((e) => console.log(`[zones] loadMap error: ${e}`)); });

  // Detection poll (sub-slice-1 backend, all zones). ~8 Hz.
  let frame = 0;
  OnGameFrame.subscribe(() => {
    if ((frame++ & 7) !== 0 || zones.size === 0) return;
    const players = Player.all();
    for (const z of zones.values()) {
      const cur = new Set<number>();
      for (const p of players) {
        const pw = p.pawn; if (!pw) continue;
        const o = pw.origin; if (!o) continue;
        if (contains(z, o.x, o.y, o.z)) {
          cur.add(p.slot);
          if (!z.inside.has(p.slot)) console.log(`[zones] ENTER ${z.name}: ${p.playerName} (slot ${p.slot})`);
        }
      }
      for (const s of z.inside) if (!cur.has(s)) console.log(`[zones] LEAVE ${z.name}: slot ${s}`);
      z.inside = cur;
    }
  });

  // sm_zone_add <name> <x1 y1 z1 x2 y2 z2> | <name> [size]
  Commands.registerAdmin("sm_zone_add", ADMFLAG.GENERIC, (ctx) => {
    const name = sanitizeName(ctx.args[0] || "");
    if (!name) { ctx.reply("Usage: sm_zone_add <name> <x1 y1 z1 x2 y2 z2>  |  sm_zone_add <name> [size] (in-game)"); return; }
    let box: { min: Vec3; max: Vec3 } | null = null;
    if (ctx.args.length >= 7) {
      const n = ctx.args.slice(1, 7).map((s) => parseFloat(s));
      if (n.some((v) => !isFinite(v))) { ctx.reply("Invalid coordinates."); return; }
      box = normBox({ x: n[0], y: n[1], z: n[2] }, { x: n[3], y: n[4], z: n[5] });
    } else {
      if (ctx.callerSlot < 0) { ctx.reply("From the server console, give explicit coords: sm_zone_add <name> <x1 y1 z1 x2 y2 z2>"); return; }
      const pw = Pawn.forSlot(ctx.callerSlot); const o = pw ? pw.origin : null;
      if (!o) { ctx.reply("No position — spawn in first, or give explicit coords."); return; }
      const size = ctx.args.length > 1 ? Math.abs(parseFloat(ctx.args[1])) || 128 : 128;
      box = normBox({ x: o.x - size, y: o.y - size, z: o.z - size }, { x: o.x + size, y: o.y + size, z: o.z + size });
    }
    if (box.min.x === box.max.x || box.min.y === box.max.y || box.min.z === box.max.z) { ctx.reply("Zero-volume zone rejected."); return; }
    const b = box;
    upsertZone(name, b).then(() => ctx.reply(`Zone '${name}' saved (${b.min.x.toFixed(0)},${b.min.y.toFixed(0)},${b.min.z.toFixed(0)})–(${b.max.x.toFixed(0)},${b.max.y.toFixed(0)},${b.max.z.toFixed(0)})`))
      .catch((e) => ctx.reply(`Save failed: ${e}`));
  });

  Commands.registerAdmin("sm_zone_delete", ADMFLAG.GENERIC, (ctx) => {
    const name = sanitizeName(ctx.args[0] || "");
    if (!name || !zones.has(name)) { ctx.reply(`No zone '${name}' on this map.`); return; }
    zones.delete(name);
    if (db) db.execute("DELETE FROM zones WHERE map = ? AND name = ?", [currentMap, name]).catch(() => {});
    ctx.reply(`Zone '${name}' deleted.`);
  });

  Commands.registerAdmin("sm_zone_list", ADMFLAG.GENERIC, (ctx) => {
    ctx.reply(`Zones on ${currentMap}: ${zones.size}`);
    for (const z of zones.values())
      ctx.reply(`  ${z.name} (${z.min.x.toFixed(0)},${z.min.y.toFixed(0)},${z.min.z.toFixed(0)})–(${z.max.x.toFixed(0)},${z.max.y.toFixed(0)},${z.max.z.toFixed(0)}) inside=${z.inside.size}`);
  });

  Commands.registerAdmin("sm_zone_export", ADMFLAG.GENERIC, (ctx) => {
    const out: Record<string, { min: number[]; max: number[] }> = {};
    for (const z of zones.values()) out[z.name] = { min: [z.min.x, z.min.y, z.min.z], max: [z.max.x, z.max.y, z.max.z] };
    config.writeFile("zones-" + sanitizeName(currentMap), JSON.stringify(out, null, 2));
    ctx.reply(`Exported ${zones.size} zone(s) to zones-${currentMap}.`);
  });

  Commands.registerAdmin("sm_zone_import", ADMFLAG.GENERIC, (ctx) => {
    const raw = config.readFile("zones-" + sanitizeName(currentMap));
    if (!raw) { ctx.reply(`No zones file for ${currentMap}.`); return; }
    let parsed: Record<string, { min: number[]; max: number[] }>;
    try { parsed = JSON.parse(raw); } catch { ctx.reply("Zones file is not valid JSON."); return; }
    let n = 0;
    const pending: Promise<void>[] = [];
    for (const key of Object.keys(parsed)) {
      const name = sanitizeName(key); const e = parsed[key];
      if (!name || !e || !Array.isArray(e.min) || !Array.isArray(e.max) || e.min.length < 3 || e.max.length < 3) continue;
      const box = normBox({ x: e.min[0], y: e.min[1], z: e.min[2] }, { x: e.max[0], y: e.max[1], z: e.max[2] });
      pending.push(upsertZone(name, box)); n++;
    }
    Promise.all(pending).then(() => ctx.reply(`Imported ${n} zone(s).`)).catch((err) => ctx.reply(`Import error: ${err}`));
  });

  console.log("[zones] onLoad — commands registered (origin-polling backend)");
}
```

- [ ] **Step 3: build + typecheck** — `( cd packages/cli && node build.mjs ) && node packages/cli/dist/cli.js build plugins/zones`; `bash scripts/check-plugins-typecheck.sh` green. Confirm `CommandContext.callerSlot`/`args` + `Player.all()`/`pawn.origin`/`playerName` + `config`/`Database`/`Server` signatures against the real `.d.ts` (adjust if any differ — e.g. `config` default export shape).

- [ ] **Step 4: retire the spike** — `git rm -r examples/zones-spike`.

- [ ] **Step 5: commit.**

## Deploy + live gate

- [ ] Ensure `dist/addons/s2script/data` exists + is host-writable (the DB; the clientprefs/db-demo RW mount). Deploy the plugin `.s2sp` (hot-reload if the server is up: `cp plugins/zones/dist/*.s2sp dist/addons/s2script/plugins/`), no sniper.
- [ ] **Gate** (de_inferno, `bot_quota 4`, rcon):
  - `[zones] loaded 0 zone(s) for de_inferno` + `onLoad — DB ready`.
  - `sm_zone_add zoneA <6 coords bounding bot 0's area>` → `Zone 'zoneA' saved`; `sm_zone_list` → shows `zoneA`; a bot inside → `[zones] ENTER zoneA: <name> (slot N)`.
  - `sm_zone_export` → the JSON file written; `sm_zone_delete zoneA` → gone from list; `sm_zone_import` → `zoneA` restored.
  - **Restart** → boot `[zones] loaded 1 zone(s) for de_inferno` (DB persistence) + re-fires `ENTER zoneA`.
  - `changelevel de_dust2` → `[zones] loaded 0 zone(s) for de_dust2` (per-map isolation); `RestartCount=0`, no crash.
- [ ] Merge, push, document (CLAUDE.md + memory), update the sub-slice-3 note.

## Self-review

- Spec coverage: DB+registry (upsert/load/delete), JSON export/import, per-map `onMapStart`, the 5 commands, the generalized poll — all present.
- Async safety: `db` guarded everywhere; `loadMap`/`upsertZone` are async + awaited/caught; the poll never awaits.
- The in-game `sm_zone_add <name> [size]` (human) vs explicit-coords (rcon-testable) split matches the spec.
