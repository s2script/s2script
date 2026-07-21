// @s2script/nextmap — SourceMod nextmap: a maplist.txt rotation with an `sm_setnextmap` admin
// override, seeding the `nextlevel` cvar (basetriggers' `nextmap` chat trigger reads it) and
// auto-changelevel at mp_maxrounds/mp_timelimit.
//
// Task 1: the plugin scaffold, maplist parsing + rotation, the sm_setnextmap override, and the
// per-map-change poll that reseeds nextlevel.
// Task 2 (this file, current state): round_end (mp_maxrounds) + mp_timelimit detection, and the
// real changeToNext body (delayed, validated changelevel/host_workshop_map).

import { plugin } from "@s2script/sdk/plugin";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Server } from "@s2script/sdk/server";
import { config } from "@s2script/sdk/config";
import { delay } from "@s2script/sdk/timers";
import { Chat } from "@s2script/sdk/chat";

/** A map option: its stock/BSP name, or a workshop id (mutually informative). */
interface MapEntry { name: string; workshopId: string | null; }

// --- module state (persists across a changelevel — see pollTick below) ---
let override: MapEntry | null = null;
let roundsPlayed = 0;
let currentMap = "";
let changing = false;
let frameCounter = 0; // throttles the map-change poll to ~once/sec
let failNotified = false; // debounces the misconfiguration log so a persistent failure doesn't spam every tick

const logErr = (e: unknown) => console.log("[nextmap] error: " + e);

// nextmap OWNS its rotation source: it auto-generates this maplist.txt template when the file is
// absent, so it is fully standalone and does NOT depend on nominations having created it. The
// template is write-if-absent (idempotent) and its parsed pool is identical to the nominations
// template, so the two plugins co-own the same file harmlessly (whichever loads first writes it).
const MAPLIST_TEMPLATE =
  "// s2script maplist — one map per line (used by nextmap rotation + nominations).\n" +
  "// Workshop maps: name:workshopId  (e.g. awp_lego_2:3070284539)\n" +
  "// Lines starting with // or # are ignored.\n" +
  "de_dust2\nde_inferno\nde_mirage\nde_nuke\nde_ancient\nde_anubis\n";

// maplist.txt parsing — copied verbatim from plugins/disabled/rockthevote/src/plugin.ts (colon-split
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
  // Own the list source: create the template if absent (write-if-absent = idempotent), else read it.
  // A later read failure still degrades gracefully to an empty pool (rotationNext then returns null).
  let text = config.readFile("maplist.txt");
  if (text === null) { config.writeFile("maplist.txt", MAPLIST_TEMPLATE); text = MAPLIST_TEMPLATE; }
  return parseMaplist(text);
}

/** The map that follows `map` in the pool (wraps around); null if the pool is empty. */
function rotationNext(map: string): MapEntry | null {
  const list = loadPool();
  if (list.length === 0) return null;
  const i = list.findIndex(m => m.name === map);
  return i < 0 ? list[0] : list[(i + 1) % list.length];
}

/** Injection guard for any entry whose name/id reaches Server.command/setCvar (the console splits on `;`).
 *  Applied at EVERY sink — the nextlevel seed, sm_setnextmap, and changeToNext. */
function isValidEntry(e: MapEntry): boolean {
  return /^[A-Za-z0-9_]+$/.test(e.name) && (e.workshopId === null || /^[0-9]+$/.test(e.workshopId));
}

/** The delayed, validated changelevel/host_workshop_map — announces then switches after a config delay. */
function changeToNext(): void {
  if (changing) return;
  changing = true;
  const next = override ?? rotationNext(currentMap);
  if (!next) {
    // No candidate (e.g. maplist.txt empty/missing on a fresh deploy) — leave `changing` reset so the
    // NEXT round_end/timelimit tick (or a later sm_setnextmap) retries; log ONCE per failure episode
    // (an operator config error → the server log, not repeated player chat — SM parity).
    changing = false;
    if (!failNotified) { failNotified = true; console.log("[nextmap] no next map available (check maplist.txt / sm_setnextmap)"); }
    return;
  }
  if (!isValidEntry(next)) {
    changing = false;
    if (!failNotified) { failNotified = true; console.log("[nextmap] next map failed validation: " + JSON.stringify(next)); }
    return;
  }
  failNotified = false; // a valid candidate — clear so a future failure re-logs
  const secs = config.getInt("nextmap_change_delay");
  console.log("[nextmap] map end reached — changing to " + next.name + " in " + secs + "s");
  const scheduledMap = currentMap; // captured now — if an external actor changes the map before
  // our delay fires, Server.mapName will have moved on and the stale changelevel is skipped below.
  Chat.toAll("[nextmap] Changing to " + next.name + " in " + secs + "s");
  delay(secs * 1000).then(() => {
    if (Server.mapName !== scheduledMap) {
      console.log("[nextmap] scheduled change to " + next.name + " skipped — map already changed to " + Server.mapName);
      return;
    }
    Server.command(next.workshopId ? "host_workshop_map " + next.workshopId : "changelevel " + next.name);
  }).catch(logErr);
}

// Plugins persist across a changelevel — the shim has no level-init reload hook, so onLoad fires
// once per plugin-load, NOT per map. Poll Server.mapName (throttled) to catch map transitions and
// reset all per-map state, reseeding nextlevel from the rotation (mirrors the rockthevote pattern).
function pollTick(): void {
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (m && m !== currentMap) {                 // map changed — reset + seed nextlevel
    currentMap = m; roundsPlayed = 0; changing = false; override = null; failNotified = false;
    const next = rotationNext(m); if (next && isValidEntry(next)) Server.setCvar("nextlevel", next.name);
    return;
  }
  if (!changing && config.getBool("nextmap_use_timelimit")) {
    const tl = parseFloat(Server.getCvar("mp_timelimit"));   // float cvar in CS2 — parseFloat for exact SM parity
    if (tl > 0 && Server.gameTime >= tl * 60) changeToNext();
  }
}

export default plugin((ctx) => {
  loadPool();   // eager: auto-generate maplist.txt now (if absent) so the operator can edit the
                // rotation before the first map-end — nextmap owns this, independent of nominations.
  ctx.server.onGameFrame(pollTick);

  ctx.events.on("round_end", () => {
    if (changing) return;
    roundsPlayed++;
    const max = parseInt(Server.getCvar("mp_maxrounds"), 10);
    if (max > 0 && roundsPlayed >= max) changeToNext();
  });

  ctx.commands.registerAdmin("sm_setnextmap", ADMFLAG.CHANGEMAP, cmd => {
    const m = cmd.arg(0);
    if (!m) { cmd.reply("Usage: sm_setnextmap <map>"); return; }
    const inList = loadPool().find(e => e.name === m);
    const entry = inList ?? (Server.isMapValid(m) ? { name: m, workshopId: null } : null);
    if (!entry) { cmd.reply("'" + m + "' is not a valid map"); return; }
    if (!isValidEntry(entry)) { cmd.reply("Invalid map name"); return; }
    override = entry;
    Server.setCvar("nextlevel", entry.name);
    cmd.reply("Next map set to " + entry.name);
  });

  // DESCOPED: SM's sm_maphistory (list the recently-played maps) is intentionally not implemented —
  // it would require reading the map_history the nominations plugin owns in the shared mapvote DB,
  // coupling standalone nextmap back to nominations. nextmap tracks no play history of its own.
  console.log("[nextmap] onLoad — sm_setnextmap registered");
});
