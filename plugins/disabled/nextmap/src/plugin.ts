// @s2script/nextmap — SourceMod nextmap: a maplist.txt rotation with an `sm_setnextmap` admin
// override, seeding the `nextlevel` cvar (basetriggers' `nextmap` chat trigger reads it) and
// an optional random startup map, then auto-changelevel at mp_maxrounds/mp_timelimit.

import { command, hook, translations, publish, ADMFLAG, Server, config, delay, Chat, Translations, HookResult, Activity } from "@s2script/sdk";
import type { TypedPublishHandle } from "@s2script/sdk";
import type { Contract } from "../api";

/** A map option: its stock/BSP name, or a workshop id (mutually informative). */
interface MapEntry { name: string; workshopId: string | null; }

// --- module state (persists across a changelevel — see pollTick below) ---
let override: MapEntry | null = null;
let roundsPlayed = 0;
let currentMap = "";
let changing = false;
let readyChange: (() => void) | null = null;
let matchEnded = false;
let mapChanges: TypedPublishHandle<Contract>;
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

/** Announce and count down; rotation optionally waits for a round boundary. Startup bypasses it. */
function changeToNext(next: MapEntry | null = override ?? rotationNext(currentMap), startup = false): void {
  if (changing) return;
  changing = true;
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
  const waitForRound = !startup && config.getBool("nextmap_wait_for_round");
  const secs = config.getInt("nextmap_change_delay");
  console.log("[nextmap] changing to " + next.name + " after " + secs + "s" + (waitForRound ? " at the next round boundary" : ""));
  const scheduledMap = currentMap; // captured now — if an external actor changes the map before
  // our delay fires, Server.mapName will have moved on and the stale changelevel is skipped below.
  // Broadcast to everyone — no single recipient, so this resolves at the server default language.
  Chat.toAll(Translations.translate(-1, waitForRound ? "Changing After Round" : "Changing To", next.name, secs));
  const apply = () => {
    if (Server.mapName !== scheduledMap) {
      console.log("[nextmap] scheduled change to " + next.name + " skipped — map already changed to " + Server.mapName);
      return;
    }
    // A vote winner takes priority even if its boundary handler ran first and queued a command.
    if (!startup && mapChanges.dispatch("OnAutoMapChange", { currentMap: scheduledMap, nextMap: next.name }) >= HookResult.Handled) return;
    Server.command(next.workshopId ? "host_workshop_map " + next.workshopId : "changelevel " + next.name);
  };
  delay(secs * 1000).then(() => {
    if (Server.mapName !== scheduledMap || !waitForRound || matchEnded) apply();
    else readyChange = apply;
  }).catch(logErr);
}

function applyReadyChange(): void {
  const apply = readyChange;
  readyChange = null;
  apply?.();
}

/** Pick once per plugin load, after the server's initial map becomes available. */
function changeToStartupMap(): void {
  const list = loadPool().filter(e => isValidEntry(e) && (e.workshopId !== null || Server.isMapValid(e.name)));
  if (list.length === 0) {
    console.log("[nextmap] no usable startup map available (check maplist.txt)");
    return;
  }
  const next = list[Math.floor(Math.random() * list.length)];
  if (next.name.toLowerCase() !== currentMap.toLowerCase()) changeToNext(next, true);
}

// Plugins persist across a changelevel: OnPluginStart fires once per plugin load, not per map.
// Poll Server.mapName (throttled) to catch map transitions and
// reset all per-map state, reseeding nextlevel from the rotation (mirrors the rockthevote pattern).
function pollTick(): void {
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (!m) return; // do not schedule a map change before the initial map is live
  if (m !== currentMap) {                 // map changed — reset + seed nextlevel
    const startup = currentMap === ""; // consumed before scheduling; later maps keep normal rotation
    currentMap = m; roundsPlayed = 0; changing = false; override = null; failNotified = false;
    readyChange = null; matchEnded = false;
    const next = rotationNext(m); if (next && isValidEntry(next)) Server.setCvar("nextlevel", next.name);
    if (startup && config.getBool("nextmap_random_startup")) changeToStartupMap();
    return;
  }
  if (!changing && config.getBool("nextmap_use_timelimit")) {
    const tl = parseFloat(Server.getCvar("mp_timelimit"));   // float cvar in CS2 — parseFloat for exact SM parity
    if (tl > 0 && Server.gameTime >= tl * 60) changeToNext();
  }
}

export function OnPluginStart(): void {
  mapChanges = publish("@s2script/nextmap", {});
  translations.load("nextmap", "common");

  loadPool();   // eager: auto-generate maplist.txt now (if absent) so the operator can edit the
                // rotation before the first map-end — nextmap owns this, independent of nominations.

  hook.on("round_end", () => {
    if (changing) return;
    roundsPlayed++;
    const max = parseInt(Server.getCvar("mp_maxrounds"), 10);
    if (max > 0 && roundsPlayed >= max) changeToNext();
  });

  hook.on("round_prestart", () => {
    matchEnded = false;
    applyReadyChange();
  });
  // A completed match may never start another round. Its final panel is also a safe boundary;
  // if the countdown is still running, it will apply the change after it finishes.
  hook.on("cs_win_panel_match", () => {
    matchEnded = true;
    applyReadyChange();
  });

  command.admin("sm_setnextmap", ADMFLAG.CHANGEMAP, cmd => {
    const m = cmd.arg(0);
    if (!m) { cmd.replyT("Usage Setnextmap"); return HookResult.Handled; }
    const inList = loadPool().find(e => e.name === m);
    const entry = inList ?? (Server.isMapValid(m) ? { name: m, workshopId: null } : null);
    if (!entry) { cmd.replyT("Invalid Map", m); return HookResult.Handled; }
    if (!isValidEntry(entry)) { cmd.replyT("Invalid Map Name"); return HookResult.Handled; }
    override = entry;
    Server.setCvar("nextlevel", entry.name);
    Activity.showT(cmd, "Changed Next Map", entry.name);
    Activity.log(cmd, null, `"${Activity.describe(cmd)}" changed the next map to "${entry.name}"`);
    return HookResult.Handled;
  });

  // DESCOPED: SM's sm_maphistory (list the recently-played maps) is intentionally not implemented —
  // it would require reading the map_history the nominations plugin owns in the shared mapvote DB,
  // coupling standalone nextmap back to nominations. nextmap tracks no play history of its own.
}

export function OnGameFrame(): void {
  pollTick();
}
