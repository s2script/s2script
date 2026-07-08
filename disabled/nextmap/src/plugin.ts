// @s2script/nextmap — SourceMod nextmap: a maplist.txt rotation with an `sm_setnextmap` admin
// override, seeding the `nextlevel` cvar (basetriggers' `nextmap` chat trigger reads it) and
// auto-changelevel at mp_maxrounds/mp_timelimit.
//
// Task 1 (this file, current state): the plugin scaffold, maplist parsing + rotation, the
// sm_setnextmap override, and the per-map-change poll that reseeds nextlevel. `changeToNext` is
// an intentional STUB here — Task 2 fills in the delayed, validated changelevel/host_workshop_map.
// Task 2: round_end (mp_maxrounds) + mp_timelimit detection, and the real changeToNext body.

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { OnGameFrame } from "@s2script/frame";
import { Server } from "@s2script/server";
import { config } from "@s2script/config";

/** A map option: its stock/BSP name, or a workshop id (mutually informative). */
interface MapEntry { name: string; workshopId: string | null; }

// --- module state (persists across a changelevel — see pollTick below) ---
let override: MapEntry | null = null;
let roundsPlayed = 0;
let currentMap = "";
let changing = false;
let frameCounter = 0; // throttles the map-change poll to ~once/sec

const logErr = (e: unknown) => console.log("[nextmap] error: " + e);

// maplist.txt parsing — copied verbatim from disabled/rockthevote/src/plugin.ts (colon-split
// "name:workshopId", `//`/`#`/blank skip, skip an empty-name entry).
function parseMaplist(text: string): MapEntry[] {
  const out: MapEntry[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("//") || line.startsWith("#")) continue;
    const i = line.indexOf(":");
    const name = i >= 0 ? line.slice(0, i).trim() : line;
    if (!name) continue;   // skip a malformed ":123" (empty name) entry
    out.push({ name, workshopId: i >= 0 ? (line.slice(i + 1).trim() || null) : null });
  }
  return out;
}

function loadPool(): MapEntry[] {
  const t = config.readFile("maplist.txt");
  return t === null ? [] : parseMaplist(t);
}

/** The map that follows `map` in the pool (wraps around); null if the pool is empty. */
function rotationNext(map: string): MapEntry | null {
  const list = loadPool();
  if (list.length === 0) return null;
  const i = list.findIndex(m => m.name === map);
  return i < 0 ? list[0] : list[(i + 1) % list.length];
}

/** Task 1 STUB — Task 2 fills this in with the delayed, validated changelevel/host_workshop_map. */
function changeToNext(): void {
  if (changing) return;
  changing = true;
  console.log("[nextmap] changeToNext (stub)");
}

// Plugins persist across a changelevel — the shim has no level-init reload hook, so onLoad fires
// once per plugin-load, NOT per map. Poll Server.mapName (throttled) to catch map transitions and
// reset all per-map state, reseeding nextlevel from the rotation (mirrors the rockthevote pattern).
function pollTick(): void {
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (m && m !== currentMap) {                 // map changed — reset + seed nextlevel
    currentMap = m; roundsPlayed = 0; changing = false; override = null;
    const next = rotationNext(m); if (next) Server.setCvar("nextlevel", next.name);
    return;
  }
  // (Task 2 adds the timelimit check here)
}

export function onLoad(): void {
  OnGameFrame.subscribe(pollTick);

  Commands.registerAdmin("sm_setnextmap", ADMFLAG.CHANGEMAP, ctx => {
    const m = ctx.arg(0);
    if (!m) { ctx.reply("Usage: sm_setnextmap <map>"); return; }
    const inList = loadPool().find(e => e.name === m);
    const entry = inList ?? (Server.isMapValid(m) ? { name: m, workshopId: null } : null);
    if (!entry) { ctx.reply("'" + m + "' is not a valid map"); return; }
    if (!/^[A-Za-z0-9_]+$/.test(entry.name)) { ctx.reply("Invalid map name"); return; }
    override = entry;
    Server.setCvar("nextlevel", entry.name);
    ctx.reply("Next map set to " + entry.name);
  });

  console.log("[nextmap] onLoad — sm_setnextmap registered");
}

export function onUnload(): void {
  console.log("[nextmap] onUnload");
}
