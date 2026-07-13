// @s2script/zones — DB-backed, per-map, coordinate-defined zones with JSON export/import, operator CRUD,
// and a published inter-plugin interface (`@s2script/zones@0.1.0`, emits enter/leave/stay).
//
// DETECTION BACKEND: REAL ENGINE TRIGGERS. Each zone is a runtime `trigger_multiple` whose collision is an
// arbitrary box built from the zone bounds (createEntity -> SetModel registers the touch aggregate ->
// SetSolid(SOLID_BBOX) reshapes it to the box). The engine's own touch system fires OnStartTouch/OnEndTouch,
// which we hook via Entity.onOutput -> enter/leave. This replaces the previous ~8Hz origin-polling backend:
// engine-accurate edges, no per-frame position math, and it can see non-player entities too. A tiny poll
// remains only to emit `stay` for currently-inside players (no position tests — just re-emitting the
// engine-maintained inside-set).
import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Database } from "@s2script/db";
import { Server } from "@s2script/server";
import { config } from "@s2script/config";
import { OnGameFrame } from "@s2script/frame";
import { publishInterface, PublishHandle } from "@s2script/interfaces";
import { Entity } from "@s2script/entity";
import { Player, Pawn, TriggerZone, TriggerZoneHandle, Beam, BeamHandle } from "@s2script/cs2";
import { Vector } from "@s2script/math";

interface Vec3 { x: number; y: number; z: number; }
interface Zone { name: string; min: Vec3; max: Vec3; tags: string[]; inside: Set<number>; trigger: TriggerZoneHandle | null; }

let db: Database | null = null;
let currentMap = "";
const zones = new Map<string, Zone>();
let iface: PublishHandle | null = null;

// Zones whose trigger still needs to be (re)created. createEntity is unsafe at onMapStart (the entity
// system isn't live yet — it crashes), so we NEVER create a trigger inline: we queue the zone here and
// build it on the next OnGameFrame, when the map is fully live. loadMap + upsertZone both queue.
const pendingTriggers = new Set<string>();

// Resolves once Database.open() + CREATE TABLE + the initial load have settled (success OR failure).
let dbReadyResolve: () => void = () => {};
const dbReady: Promise<void> = new Promise<void>((r) => { dbReadyResolve = r; });

function sanitizeName(n: string): string { return (n || "").replace(/[^A-Za-z0-9_-]/g, "").slice(0, 64); }
function sanitizeTag(t: string): string { return (t || "").toLowerCase().replace(/[^a-z0-9_-]/g, "").slice(0, 32); }
function parseTags(v: unknown): string[] {
  return String(v ?? "").split(",").map((t) => sanitizeTag(t)).filter((t) => t.length > 0);
}
function zonesByTag(tag: string): Zone[] {
  const t = sanitizeTag(tag);
  if (!t) return [];
  return Array.from(zones.values()).filter((z) => z.tags.includes(t));
}
function zonesFile(map: string): string { return "zones-" + sanitizeName(map) + ".json"; }
function normBox(a: Vec3, b: Vec3): { min: Vec3; max: Vec3 } {
  return {
    min: { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), z: Math.min(a.z, b.z) },
    max: { x: Math.max(a.x, b.x), y: Math.max(a.y, b.y), z: Math.max(a.z, b.z) },
  };
}

// --- beam wireframe viz --------------------------------------------------------------------------------
interface ShownEntry { beams: BeamHandle[]; expiresAt: number; }   // expiresAt: wall-clock ms; 0 = persistent
const shown = new Map<string, ShownEntry>();

// The 12 edges of the AABB [min,max]: 8 corners -> 4 bottom + 4 top + 4 vertical.
function box12(min: Vec3, max: Vec3): { a: Vec3; b: Vec3 }[] {
  const c: Vec3[] = [
    { x: min.x, y: min.y, z: min.z }, { x: max.x, y: min.y, z: min.z },
    { x: max.x, y: max.y, z: min.z }, { x: min.x, y: max.y, z: min.z },
    { x: min.x, y: min.y, z: max.z }, { x: max.x, y: min.y, z: max.z },
    { x: max.x, y: max.y, z: max.z }, { x: min.x, y: max.y, z: max.z },
  ];
  const e: [number, number][] = [[0,1],[1,2],[2,3],[3,0],[4,5],[5,6],[6,7],[7,4],[0,4],[1,5],[2,6],[3,7]];
  return e.map(([i, j]) => ({ a: c[i], b: c[j] }));
}

function showZone(z: Zone, seconds: number): void {
  hideZone(z.name);   // never stack two beam sets for one zone
  const beams: BeamHandle[] = [];
  for (const e of box12(z.min, z.max)) {
    const b = Beam.draw(new Vector(e.a.x, e.a.y, e.a.z), new Vector(e.b.x, e.b.y, e.b.z), { color: [0, 255, 0, 255], width: 2 });
    if (b) beams.push(b);
  }
  shown.set(z.name, { beams, expiresAt: seconds > 0 ? Date.now() + seconds * 1000 : 0 });
}
function hideZone(name: string): void {
  const entry = shown.get(name);
  if (!entry) return;
  for (const b of entry.beams) { try { b.remove(); } catch { /* stale/already-gone */ } }
  shown.delete(name);
}
function clearAllBeams(): void { for (const name of Array.from(shown.keys())) hideZone(name); }

// --- trigger lifecycle ---------------------------------------------------------------------------------
function removeTrigger(z: Zone): void {
  if (z.trigger) { try { z.trigger.remove(); } catch { /* stale/already-gone */ } z.trigger = null; }
  z.inside.clear();
}
function buildTrigger(z: Zone): void {
  removeTrigger(z);
  z.trigger = TriggerZone.create(z.min, z.max);   // arbitrary engine box; fires OnStartTouch/OnEndTouch
}
function clearAllTriggers(): void { for (const z of zones.values()) removeTrigger(z); pendingTriggers.clear(); }

// Map an OnStartTouch/OnEndTouch back to a zone (by the firing trigger entity) and a player (by the
// touching pawn entity). Both are looked up by live entity index — no stored raw pointers.
function zoneByTriggerIndex(idx: number): Zone | null {
  for (const z of zones.values()) if (z.trigger && z.trigger.ref.index === idx) return z;
  return null;
}
function playerByPawnIndex(idx: number): { slot: number; userId: number } | null {
  for (const p of Player.all()) {
    const pw = p.pawn;
    if (pw && pw.ref.index === idx) return { slot: p.slot, userId: p.userId };
  }
  return null;
}

async function loadMap(map: string): Promise<void> {
  clearAllBeams();
  clearAllTriggers();
  currentMap = map;
  zones.clear();
  if (!db) return;
  const rows = await db.query("SELECT name, minX, minY, minZ, maxX, maxY, maxZ, tags FROM zones WHERE map = ?", [map]);
  for (const r of rows) {
    const name = String(r.name);
    zones.set(name, {
      name,
      min: { x: Number(r.minX), y: Number(r.minY), z: Number(r.minZ) },
      max: { x: Number(r.maxX), y: Number(r.maxY), z: Number(r.maxZ) },
      tags: parseTags(r.tags),
      inside: new Set<number>(),
      trigger: null,
    });
    pendingTriggers.add(name);   // build on the next frame (entity system live)
  }
  console.log(`[zones] loaded ${zones.size} zone(s) for ${map}`);
}

async function upsertZone(name: string, box: { min: Vec3; max: Vec3 }, tags?: string[]): Promise<void> {
  await dbReady;   // guarantee the DB is open (or failed) before we mutate
  const prev = zones.get(name);
  const t = tags !== undefined ? tags : (prev ? prev.tags : []);
  zones.set(name, { name, min: box.min, max: box.max, tags: t, inside: prev ? prev.inside : new Set<number>(), trigger: prev ? prev.trigger : null });
  pendingTriggers.add(name);   // (re)build the trigger on the next frame
  if (db) await db.execute(
    "INSERT OR REPLACE INTO zones (map, name, minX, minY, minZ, maxX, maxY, maxZ, tags) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    [currentMap, name, box.min.x, box.min.y, box.min.z, box.max.x, box.max.y, box.max.z, t.join(",")]);
}

function dropZone(name: string): void {
  const z = zones.get(name);
  if (z) removeTrigger(z);
  zones.delete(name);
  pendingTriggers.delete(name);
  hideZone(name);
  if (db) db.execute("DELETE FROM zones WHERE map = ? AND name = ?", [currentMap, name]).catch(() => {});
}

export function onLoad(): void {
  (async () => {
    try {
      db = await Database.open("zones");
      await db.execute(
        "CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, tags TEXT, PRIMARY KEY (map, name))");
      try { await db.execute("ALTER TABLE zones ADD COLUMN tags TEXT"); } catch { /* duplicate column name — already migrated */ }
      await loadMap(Server.mapName);
      console.log("[zones] onLoad — DB ready (real-trigger backend)");
    } catch (e) {
      console.log(`[zones] init error (zones will not persist): ${e}`);
    } finally {
      dbReadyResolve();
    }
  })();

  Server.onMapStart((map) => { loadMap(map).catch((e) => console.log(`[zones] loadMap error: ${e}`)); });

  iface = publishInterface("@s2script/zones", "0.1.0", {
    createZone(name: string, min: Vec3, max: Vec3): boolean {
      const nm = sanitizeName(name);
      if (!nm || !min || !max) return false;
      const box = normBox(min, max);
      if (box.min.x === box.max.x || box.min.y === box.max.y || box.min.z === box.max.z) return false;
      const prev = zones.get(nm);
      zones.set(nm, { name: nm, min: box.min, max: box.max, tags: prev ? prev.tags : [], inside: prev ? prev.inside : new Set<number>(), trigger: prev ? prev.trigger : null });
      pendingTriggers.add(nm);
      upsertZone(nm, box).catch(() => {});   // durability, async (registry already updated)
      return true;
    },
    deleteZone(name: string): boolean {
      const nm = sanitizeName(name);
      if (!zones.has(nm)) return false;
      dropZone(nm);
      return true;
    },
    getZones(): { name: string; min: Vec3; max: Vec3; tags: string[] }[] {
      return Array.from(zones.values()).map((z) => ({ name: z.name, min: z.min, max: z.max, tags: z.tags }));
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
    getZonesByTag(tag: string): { name: string; min: Vec3; max: Vec3; tags: string[] }[] {
      return zonesByTag(String(tag ?? "")).map((z) => ({ name: z.name, min: z.min, max: z.max, tags: z.tags }));
    },
    setZoneTags(name: string, tags: string[]): boolean {
      const nm = sanitizeName(name);
      const z = zones.get(nm);
      if (!z || !Array.isArray(tags)) return false;
      const t = tags.map((x) => sanitizeTag(String(x))).filter((x) => x.length > 0);
      z.tags = t;
      if (db) db.execute("UPDATE zones SET tags = ? WHERE map = ? AND name = ?", [t.join(","), currentMap, nm]).catch(() => {});
      return true;
    },
  });
  console.log("[zones] publishing @s2script/zones@0.1.0");

  // ENTER/LEAVE come from the engine's own touch outputs on OUR trigger entities. Entity.onOutput fires for
  // ALL trigger_multiple (incl. map triggers), so we filter to our zone triggers by the firing entity.
  Entity.onOutput("trigger_multiple", "OnStartTouch", (ev) => {
    if (!ev.caller || !ev.activator || !iface) return;
    const z = zoneByTriggerIndex(ev.caller.index);
    if (!z) return;
    const who = playerByPawnIndex(ev.activator.index);
    if (!who || z.inside.has(who.slot)) return;
    z.inside.add(who.slot);
    iface.emit("enter", { zone: z.name, slot: who.slot, userId: who.userId });
  });
  Entity.onOutput("trigger_multiple", "OnEndTouch", (ev) => {
    if (!ev.caller || !ev.activator || !iface) return;
    const z = zoneByTriggerIndex(ev.caller.index);
    if (!z) return;
    const who = playerByPawnIndex(ev.activator.index);
    if (!who || !z.inside.has(who.slot)) return;
    z.inside.delete(who.slot);
    iface.emit("leave", { zone: z.name, slot: who.slot, userId: who.userId });
  });

  // Per-frame: (1) build any queued triggers now that the entity system is live; (2) a light STAY re-emit
  // for players the engine reports as currently inside (no position tests — just the engine-maintained set).
  let frame = 0;
  OnGameFrame.subscribe(() => {
    if (shown.size > 0) {
      const now = Date.now();
      for (const [name, entry] of shown) if (entry.expiresAt > 0 && now >= entry.expiresAt) hideZone(name);
    }
    if (pendingTriggers.size > 0) {
      for (const name of pendingTriggers) { const z = zones.get(name); if (z) buildTrigger(z); }
      pendingTriggers.clear();
    }
    if ((frame++ & 7) !== 0 || !iface) return;
    let any = false;
    for (const z of zones.values()) if (z.inside.size > 0) { any = true; break; }
    if (!any) return;
    const uid = new Map<number, number>();
    for (const p of Player.all()) uid.set(p.slot, p.userId);
    for (const z of zones.values())
      for (const slot of z.inside)
        iface.emit("stay", { zone: z.name, slot, userId: uid.get(slot) ?? -1 });
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
    dropZone(name);
    ctx.reply(`Zone '${name}' deleted.`);
  });

  Commands.registerAdmin("sm_zone_tag", ADMFLAG.GENERIC, (ctx) => {
    const name = sanitizeName(ctx.args[0] || "");
    const z = zones.get(name);
    if (!name || !z) { ctx.reply(`No zone '${name}' on this map. Usage: sm_zone_tag <name> [tag...] (no tags = clear)`); return; }
    const tags = ctx.args.slice(1).map((t) => sanitizeTag(t)).filter((t) => t.length > 0);
    z.tags = tags;
    if (db) db.execute("UPDATE zones SET tags = ? WHERE map = ? AND name = ?", [tags.join(","), currentMap, name]).catch(() => {});
    ctx.reply(tags.length > 0 ? `Zone '${name}' tags: ${tags.join(", ")}` : `Zone '${name}' tags cleared.`);
  });

  Commands.registerAdmin("sm_zone_list", ADMFLAG.GENERIC, (ctx) => {
    const filter = ctx.args.length > 0 ? sanitizeTag(ctx.args[0]) : "";
    const list = filter ? zonesByTag(filter) : Array.from(zones.values());
    ctx.reply(filter ? `Zones on ${currentMap} tagged '${filter}': ${list.length}` : `Zones on ${currentMap}: ${list.length}`);
    for (const z of list)
      ctx.reply(`  ${z.name} (${z.min.x.toFixed(0)},${z.min.y.toFixed(0)},${z.min.z.toFixed(0)})-(${z.max.x.toFixed(0)},${z.max.y.toFixed(0)},${z.max.z.toFixed(0)}) tags=[${z.tags.join(",")}] inside=${z.inside.size} trigger=${z.trigger ? "yes" : "pending"}`);
  });

  Commands.registerAdmin("sm_zone_export", ADMFLAG.GENERIC, (ctx) => {
    const out: Record<string, { min: number[]; max: number[]; tags: string[] }> = {};
    for (const z of zones.values()) out[z.name] = { min: [z.min.x, z.min.y, z.min.z], max: [z.max.x, z.max.y, z.max.z], tags: z.tags };
    config.writeFile(zonesFile(currentMap), JSON.stringify(out, null, 2));
    ctx.reply(`Exported ${zones.size} zone(s) to ${zonesFile(currentMap)}.`);
  });

  Commands.registerAdmin("sm_zone_import", ADMFLAG.GENERIC, (ctx) => {
    const raw = config.readFile(zonesFile(currentMap));
    if (!raw) { ctx.reply(`No zones file for ${currentMap}.`); return; }
    let parsed: Record<string, { min: number[]; max: number[]; tags?: string[] }>;
    try { parsed = JSON.parse(raw); } catch { ctx.reply("Zones file is not valid JSON."); return; }
    let n = 0;
    const pend: Promise<void>[] = [];
    for (const key of Object.keys(parsed)) {
      const name = sanitizeName(key);
      const e = parsed[key];
      if (!name || !e || !Array.isArray(e.min) || !Array.isArray(e.max) || e.min.length < 3 || e.max.length < 3) continue;
      const box = normBox({ x: e.min[0], y: e.min[1], z: e.min[2] }, { x: e.max[0], y: e.max[1], z: e.max[2] });
      const tags = Array.isArray(e.tags) ? e.tags.map((t) => sanitizeTag(String(t))).filter((t) => t.length > 0) : undefined;
      pend.push(upsertZone(name, box, tags));
      n++;
    }
    Promise.all(pend).then(() => ctx.reply(`Imported ${n} zone(s).`)).catch((err) => ctx.reply(`Import error: ${err}`));
  });

  Commands.registerAdmin("sm_zone_show", ADMFLAG.GENERIC, (ctx) => {
    const arg = ctx.args[0] || "";
    if (!arg) { ctx.reply("Usage: sm_zone_show <name|all> [seconds] (default 30; 0 = persistent)"); return; }
    const seconds = ctx.args.length > 1 ? Math.max(0, ctx.argFloat(1, 30)) : 30;
    if (arg === "all") {
      for (const z of zones.values()) showZone(z, seconds);
      ctx.reply(`Showing ${zones.size} zone(s)` + (seconds > 0 ? ` for ${seconds}s.` : " (persistent)."));
      return;
    }
    const z = zones.get(sanitizeName(arg));
    if (!z) { ctx.reply(`No zone '${sanitizeName(arg)}' on this map.`); return; }
    showZone(z, seconds);
    ctx.reply(`Showing '${z.name}'` + (seconds > 0 ? ` for ${seconds}s.` : " (persistent)."));
  });
  Commands.registerAdmin("sm_zone_hide", ADMFLAG.GENERIC, (ctx) => {
    const arg = ctx.args[0] || "all";
    if (arg === "all") { const n = shown.size; clearAllBeams(); ctx.reply(`Hid ${n} zone(s).`); return; }
    const name = sanitizeName(arg);
    if (!shown.has(name)) { ctx.reply(`Zone '${name}' is not shown.`); return; }
    hideZone(name);
    ctx.reply(`Hid '${name}'.`);
  });

  console.log("[zones] onLoad — commands registered (real-trigger backend)");
}

// Hot-reload cleanup: remove our runtime trigger entities so a reload doesn't orphan/duplicate them
// (created entities are game-world-owned, not auto-ledgered). onLoad rebuilds them from the DB.
export function onUnload(): void {
  clearAllBeams();
  clearAllTriggers();
}
