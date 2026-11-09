// @s2script/workshopmaps — keep the server's workshop maps downloaded.
//
// Reads the workshop ids in maplist.txt (the `name:workshopId` lines nextmap, nominations and
// rockthevote share) plus any listed in `workshopmaps_items`, and downloads each one that is not
// installed or needs an update, through the server's own Steam session. A later
// `host_workshop_map` (or a vote that picks the map) then changes level without waiting for Steam.
//
// Runs once Steam is logged on, and again on every map start. One download at a time, so the log
// reads in order and Steam's queue is not flooded with a large list at once.

import { command, config, after, ADMFLAG, HookResult, Steam, Workshop } from "@s2script/sdk";

const ID = /^[0-9]{1,20}$/;
/** A download with no result by then (Steam dropped the session mid-download) is given up on, so one
 *  lost result never stalls the rest of the list. The next pass asks for it again. */
const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000;

/** The workshop ids in a maplist.txt: the part after the colon of each `name:workshopId` line. */
function maplistIds(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("//") || line.startsWith("#")) continue;
    const i = line.indexOf(":");
    if (i <= 0 || !line.slice(0, i).trim()) continue;   // no workshop id, or nextmap's skipped ":id" line
    const id = line.slice(i + 1).trim();
    if (ID.test(id)) out.push(id);
  }
  return out;
}

/** Every id to keep downloaded, de-duplicated, in list order. */
function wantedIds(): string[] {
  const ids: string[] = [];
  if (config.getBool("workshopmaps_from_maplist")) ids.push(...maplistIds(config.readFile("maplist.txt") ?? ""));
  for (const id of config.getString("workshopmaps_items").split(/[\s,]+/)) if (ID.test(id)) ids.push(id);
  return [...new Set(ids)].filter(id => id !== "0");
}

type Result = { ok: boolean; result: number };
/** One outstanding download promise per item. A pass that times out leaves it waiting; a later pass
 *  asks Steam again with `request` and keeps waiting on the same promise, so a session that never
 *  answers cannot pile up one waiter per item per pass. */
const inflight = new Map<string, Promise<Result>>();

function downloadWithTimeout(id: string): Promise<Result | null> {
  let pending = inflight.get(id);
  if (pending) {
    Workshop.request(id);
  } else {
    pending = Workshop.download(id);
    inflight.set(id, pending);
    pending.then(() => inflight.delete(id));
  }
  const settled = pending;
  return new Promise(resolve => {
    const timer = after(DOWNLOAD_TIMEOUT_MS, () => resolve(null));
    settled.then(r => { timer.kill(); resolve(r); });
  });
}

let running = false;
let again = false;

/** Download whatever is missing or out of date. A request while one pass runs queues one more pass. */
async function sync(reason: string): Promise<void> {
  if (running) { again = true; return; }
  if (!Steam.isLoggedOn()) return;   // Steam.onConnected runs the pass once it is
  running = true;
  try {
    do {
      again = false;
      const todo = wantedIds().filter(id => !Workshop.isInstalled(id));
      if (todo.length === 0) continue;
      console.log(`[workshopmaps] ${reason}: downloading ${todo.length} workshop map(s)`);
      let ok = 0;
      for (const id of todo) {
        const r = await downloadWithTimeout(id);
        if (r?.ok) ok++;
        else console.log(`[workshopmaps] ${id}: ` + (r ? `download failed (EResult ${r.result})` : "no result from Steam; will retry"));
      }
      console.log(`[workshopmaps] ${ok}/${todo.length} ready`);
    } while (again);
  } finally {
    running = false;
  }
}

const logErr = (e: unknown) => console.log("[workshopmaps] error: " + e);

export function OnPluginStart(): void {
  Steam.onConnected(() => {
    // A dropped session can lose an outstanding download's result. Ask again for every item still
    // waiting, so the result arrives and the running pass moves on instead of sitting out its timeout.
    for (const id of inflight.keys()) Workshop.request(id);
    sync("Steam connected").catch(logErr);
  });
  // Never gate the load on downloads that can take minutes.
  void sync("plugin start").catch(logErr);

  command.admin("sm_workshop_sync", ADMFLAG.CHANGEMAP, cmd => {
    if (!Steam.isLoggedOn()) { cmd.reply("[workshopmaps] the server is not logged on to Steam"); return HookResult.Handled; }
    const ids = wantedIds();
    const missing = ids.filter(id => !Workshop.isInstalled(id));
    cmd.reply(`[workshopmaps] ${ids.length} workshop map(s) listed, ${missing.length} to download`);
    sync("sm_workshop_sync").catch(logErr);
    return HookResult.Handled;
  });
}

export function OnMapStart(): void {
  if (config.getBool("workshopmaps_check_on_map_start")) sync("map start").catch(logErr);
}
