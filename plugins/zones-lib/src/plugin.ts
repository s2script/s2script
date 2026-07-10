// @s2script/zones — sub-slice 2: DB-backed, per-map, coordinate-defined zones with JSON export/import
// and operator CRUD, driving the sub-slice-1 origin-polling detection (ENTER/LEAVE per named zone).
// The inter-plugin event interface (publishInterface) is sub-slice 3; here the events are logged.
import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Database } from "@s2script/db";
import { Server } from "@s2script/server";
import { config } from "@s2script/config";
import { OnGameFrame } from "@s2script/frame";
import { publishInterface, PublishHandle } from "@s2script/interfaces";
import { Player, Pawn } from "@s2script/cs2";

interface Vec3 { x: number; y: number; z: number; }
interface Zone { name: string; min: Vec3; max: Vec3; inside: Set<number>; }

let db: Database | null = null;
let currentMap = "";
const zones = new Map<string, Zone>();
// Published inter-plugin interface `zones@1.0.0` — the detection loop emits enter/leave/stay through it.
// (The interface is named `zones`, NOT `@s2script/zones`: @s2script/* is reserved for prelude builtins
//  that resolve via __s2require, so a plugin-published interface must use a non-@s2script name.)
let iface: PublishHandle | null = null;

// Resolves once Database.open() + CREATE TABLE + the initial load have settled (success OR failure).
// upsertZone awaits this so an sm_zone_add issued during the boot window (before the async DB opens)
// still persists to the DB instead of silently landing registry-only and vanishing on restart.
let dbReadyResolve: () => void = () => {};
const dbReady: Promise<void> = new Promise<void>((r) => { dbReadyResolve = r; });

function sanitizeName(n: string): string { return (n || "").replace(/[^A-Za-z0-9_-]/g, "").slice(0, 64); }
function zonesFile(map: string): string { return "zones-" + sanitizeName(map) + ".json"; }
function normBox(a: Vec3, b: Vec3): { min: Vec3; max: Vec3 } {
  return {
    min: { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), z: Math.min(a.z, b.z) },
    max: { x: Math.max(a.x, b.x), y: Math.max(a.y, b.y), z: Math.max(a.z, b.z) },
  };
}
function contains(z: Zone, x: number, y: number, zc: number): boolean {
  return x >= z.min.x && x <= z.max.x && y >= z.min.y && y <= z.max.y && zc >= z.min.z && zc <= z.max.z;
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
  await dbReady;   // guarantee the DB is open (or failed) + the initial load ran, before we mutate
  zones.set(name, { name, min: box.min, max: box.max, inside: new Set<number>() });
  if (db) await db.execute(
    "INSERT OR REPLACE INTO zones (map, name, minX, minY, minZ, maxX, maxY, maxZ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    [currentMap, name, box.min.x, box.min.y, box.min.z, box.max.x, box.max.y, box.max.z]);
}

export function onLoad(): void {
  (async () => {
    try {
      db = await Database.open("zones");
      await db.execute(
        "CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, PRIMARY KEY (map, name))");
      await loadMap(Server.mapName);
      console.log("[zones] onLoad — DB ready");
    } catch (e) {
      console.log(`[zones] init error (zones will not persist): ${e}`);
    } finally {
      dbReadyResolve();   // unblock upsertZone regardless (on failure, db stays null -> registry-only)
    }
  })();

  Server.onMapStart((map) => { loadMap(map).catch((e) => console.log(`[zones] loadMap error: ${e}`)); });

  // Publish the inter-plugin interface: synchronous, registry-backed methods (a Promise can't cross the
  // structured-copy wire, so mutating methods update the registry immediately + fire-and-forget the DB).
  iface = publishInterface("zones", "1.0.0", {
    createZone(name: string, min: Vec3, max: Vec3): boolean {
      const nm = sanitizeName(name);
      if (!nm || !min || !max) return false;
      const box = normBox(min, max);
      if (box.min.x === box.max.x || box.min.y === box.max.y || box.min.z === box.max.z) return false;
      const prev = zones.get(nm);
      zones.set(nm, { name: nm, min: box.min, max: box.max, inside: prev ? prev.inside : new Set<number>() });
      upsertZone(nm, box).catch(() => {});   // durability, async (registry is already updated above)
      return true;
    },
    deleteZone(name: string): boolean {
      const nm = sanitizeName(name);
      if (!zones.has(nm)) return false;
      zones.delete(nm);
      if (db) db.execute("DELETE FROM zones WHERE map = ? AND name = ?", [currentMap, nm]).catch(() => {});
      return true;
    },
    getZones(): { name: string; min: Vec3; max: Vec3 }[] {
      return Array.from(zones.values()).map((z) => ({ name: z.name, min: z.min, max: z.max }));
    },
    isInZone(slot: number, name: string): boolean {
      const z = zones.get(sanitizeName(name));
      return !!z && z.inside.has(slot);
    },
    zonesFor(slot: number): string[] {
      const out: string[] = [];
      for (const z of zones.values()) if (z.inside.has(slot)) out.push(z.name);
      return out;
    },
  });
  console.log("[zones] publishing zones@1.0.0");

  // Detection poll (sub-slice-1 backend, generalized to N zones). ~8 Hz. Emits enter/leave/stay through
  // the interface with a WIRE-SAFE payload { zone, slot, userId } (never a Player — its methods don't
  // survive structured copy; consumers resolve via Player.fromSlot/fromUserId in their own context).
  let frame = 0;
  OnGameFrame.subscribe(() => {
    if ((frame++ & 7) !== 0 || zones.size === 0 || !iface) return;
    const players = Player.all();
    const uid = new Map<number, number>();
    for (const p of players) uid.set(p.slot, p.userId);
    for (const z of zones.values()) {
      const cur = new Set<number>();
      for (const p of players) {
        const pw = p.pawn;
        if (!pw) continue;
        const o = pw.origin;
        if (!o) continue;
        if (contains(z, o.x, o.y, o.z)) {
          cur.add(p.slot);
          const ev = { zone: z.name, slot: p.slot, userId: uid.get(p.slot) ?? -1 };
          if (!z.inside.has(p.slot)) iface.emit("enter", ev);
          iface.emit("stay", ev);
        }
      }
      for (const s of z.inside) if (!cur.has(s)) iface.emit("leave", { zone: z.name, slot: s, userId: uid.get(s) ?? -1 });
      z.inside = cur;
    }
  });

  // sm_zone_add <name> <x1 y1 z1 x2 y2 z2>  |  sm_zone_add <name> [size] (in-game, box around you)
  Commands.registerAdmin("sm_zone_add", ADMFLAG.GENERIC, (ctx) => {
    const name = sanitizeName(ctx.args[0] || "");
    if (!name) { ctx.reply("Usage: sm_zone_add <name> <x1 y1 z1 x2 y2 z2>  |  sm_zone_add <name> [size] (in-game)"); return; }
    let box: { min: Vec3; max: Vec3 } | null = null;
    if (ctx.args.length >= 7) {
      const n = ctx.args.slice(1, 7).map((s) => parseFloat(s));
      if (n.some((v) => !isFinite(v))) { ctx.reply("Invalid coordinates."); return; }
      box = normBox({ x: n[0], y: n[1], z: n[2] }, { x: n[3], y: n[4], z: n[5] });
    } else {
      if (ctx.callerSlot < 0) { ctx.reply("From the console, give explicit coords: sm_zone_add <name> <x1 y1 z1 x2 y2 z2>"); return; }
      const pw = Pawn.forSlot(ctx.callerSlot);
      const o = pw ? pw.origin : null;
      if (!o) { ctx.reply("No position — spawn in first, or give explicit coords."); return; }
      const size = ctx.args.length > 1 ? Math.abs(parseFloat(ctx.args[1])) || 128 : 128;
      box = normBox({ x: o.x - size, y: o.y - size, z: o.z - size }, { x: o.x + size, y: o.y + size, z: o.z + size });
    }
    if (box.min.x === box.max.x || box.min.y === box.max.y || box.min.z === box.max.z) { ctx.reply("Zero-volume zone rejected."); return; }
    const b = box;
    upsertZone(name, b)
      .then(() => ctx.reply(`Zone '${name}' saved (${b.min.x.toFixed(0)},${b.min.y.toFixed(0)},${b.min.z.toFixed(0)})-(${b.max.x.toFixed(0)},${b.max.y.toFixed(0)},${b.max.z.toFixed(0)})`))
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
      ctx.reply(`  ${z.name} (${z.min.x.toFixed(0)},${z.min.y.toFixed(0)},${z.min.z.toFixed(0)})-(${z.max.x.toFixed(0)},${z.max.y.toFixed(0)},${z.max.z.toFixed(0)}) inside=${z.inside.size}`);
  });

  Commands.registerAdmin("sm_zone_export", ADMFLAG.GENERIC, (ctx) => {
    const out: Record<string, { min: number[]; max: number[] }> = {};
    for (const z of zones.values()) out[z.name] = { min: [z.min.x, z.min.y, z.min.z], max: [z.max.x, z.max.y, z.max.z] };
    config.writeFile(zonesFile(currentMap), JSON.stringify(out, null, 2));
    ctx.reply(`Exported ${zones.size} zone(s) to ${zonesFile(currentMap)}.`);
  });

  Commands.registerAdmin("sm_zone_import", ADMFLAG.GENERIC, (ctx) => {
    const raw = config.readFile(zonesFile(currentMap));
    if (!raw) { ctx.reply(`No zones file for ${currentMap}.`); return; }
    let parsed: Record<string, { min: number[]; max: number[] }>;
    try { parsed = JSON.parse(raw); } catch { ctx.reply("Zones file is not valid JSON."); return; }
    let n = 0;
    const pending: Promise<void>[] = [];
    for (const key of Object.keys(parsed)) {
      const name = sanitizeName(key);
      const e = parsed[key];
      if (!name || !e || !Array.isArray(e.min) || !Array.isArray(e.max) || e.min.length < 3 || e.max.length < 3) continue;
      const box = normBox({ x: e.min[0], y: e.min[1], z: e.min[2] }, { x: e.max[0], y: e.max[1], z: e.max[2] });
      pending.push(upsertZone(name, box));
      n++;
    }
    Promise.all(pending).then(() => ctx.reply(`Imported ${n} zone(s).`)).catch((err) => ctx.reply(`Import error: ${err}`));
  });

  console.log("[zones] onLoad — commands registered (origin-polling backend)");
}
