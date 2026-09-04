import {
  command, translations, config, Database, Menu, MenuCancelReason, MenuStyle,
  Server, Chat, HookResult, Translations, topmenu,
} from "@s2script/sdk";
import type { PhraseKey, HookResultValue } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

interface MapEntry { name: string; workshopId: string | null; }

const logErr = (e: unknown) => console.log("[nominations] error: " + e);

const MAPLIST_TEMPLATE =
  "// nominations maplist — one map per line.\n" +
  "// Workshop maps: name:workshopId  (e.g. awp_lego_2:3070284539)\n" +
  "// Lines starting with // or # are ignored.\n" +
  "de_dust2\nde_inferno\nde_mirage\nde_nuke\nde_ancient\nde_anubis\n";

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
  let text = config.readFile("maplist.txt");
  if (text === null) { config.writeFile("maplist.txt", MAPLIST_TEMPLATE); text = MAPLIST_TEMPLATE; }
  return parseMaplist(text);
}

// exact-name match wins, else case-insensitive substring (mirrors Player.target).
function resolveMap(input: string, pool: MapEntry[]): MapEntry[] {
  const needle = input.toLowerCase();
  const exact = pool.filter(m => m.name.toLowerCase() === needle);
  if (exact.length) return exact;
  return pool.filter(m => m.name.toLowerCase().includes(needle));
}

let db!: Database;
let currentMap = "";     // the map we've claimed/recorded ("" until the DB is ready + first poll)
let frameCounter = 0;    // throttles the map-change poll to ~once/sec

async function cooldownSet(): Promise<Set<string>> {
  const rows = await db.query("SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?", [Math.max(0, config.getInt("map_cooldown"))]);
  return new Set(rows.map(r => String(r.map)));
}
async function nominatedSet(): Promise<Set<string>> {
  const rows = await db.query("SELECT map FROM nominations", []);
  return new Set(rows.map(r => String(r.map)));
}

// The current map is ALWAYS excluded from nomination — explicitly, not merely as a side effect of
// map_cooldown>=1 recording it in map_history (which fails to exclude it when map_cooldown is 0).
// Compared case-insensitively (maplist.txt names are operator-written; Server.mapName is the live map).
function isCurrentMap(name: string): boolean {
  const cur = Server.mapName;
  return cur !== "" && name.toLowerCase() === cur.toLowerCase();
}

async function nominate(slot: number, name: string): Promise<void> {
  if (isCurrentMap(name)) { Chat.toSlot(slot, Translations.translate(slot, "Nominate Is Current Map", name)); return; }
  if ((await cooldownSet()).has(name)) { Chat.toSlot(slot, Translations.translate(slot, "Nominate Played Too Recently", name)); return; }
  if ((await nominatedSet()).has(name)) { Chat.toSlot(slot, Translations.translate(slot, "Nominate Already Nominated", name)); return; }
  await db.execute("DELETE FROM nominations WHERE nominator = ?", [slot]);
  await db.execute("INSERT INTO nominations(map, nominator) VALUES(?, ?)", [name, slot]);
  const p = Player.fromSlot(slot);
  // Broadcast — everyone sees this, so it resolves at the server default language, not the
  // nominator's own. "A player" (unresolvable name) is a literal fallback, not a phrase, same
  // treatment as basechat's actorName() falling back to the literal "Console".
  Chat.toAll(Translations.translate(-1, "Nominate Announced", (p && p.playerName) ? p.playerName : "A player", name));
}

/**
 * Build the nominate menu.
 *
 * `recent` is listed FIRST and disabled. Cooldown maps used to be filtered out of the menu
 * entirely, which is indistinguishable from the map not being in the pool at all: a player looked
 * for the map they wanted, could not find it, and had no way to tell whether it was on cooldown,
 * missing from maplist.txt or misspelled in their head. Showing them — unselectable, and labelled
 * with the reason — answers that without a second command.
 *
 * They lead the list so they land on the first page, where the player is already looking. The
 * `disabled` flag keeps them unselectable on the HUD sheet (and off chat number keys on non-CS2).
 */
function mapMenu(slot: number, available: MapEntry[], recent: MapEntry[], titleKey: PhraseKey): void {
  const m = new Menu(Translations.translate(slot, titleKey));
  m.style = MenuStyle.Chat;   // ignored on CS2 (HUD either way); freeze stays off mid-round
  // Colour tags in the DISPLAY string still expand on a Chat backend. CS2 paints a HUD sheet, so
  // they render as plain labels there — the disabled cooldown rows stay visible and unselectable.
  for (const e of recent) {
    m.addItem(e.name, Translations.translate(slot, "Nominate Recent Item", e.name), { disabled: true });
  }
  for (const e of available) m.addItem(e.name, Translations.translate(slot, "Nominate Available Item", e.name));
  m.onSelect(e => { nominate(e.slot, e.info).catch(logErr); });   // nominate re-validates
  // Closing without picking said nothing at all, which is indistinguishable from the menu having
  // broken. Only the closes the PLAYER caused are worth a line: `NewMenu` means they opened
  // something else and are looking at it, and `Disconnect` has nobody left to tell.
  m.onCancel(e => {
    if (e.reason === MenuCancelReason.Exit) {
      Chat.toSlot(e.slot, Translations.translate(e.slot, "Nominate Menu Closed"));
    } else if (e.reason === MenuCancelReason.Timeout) {
      Chat.toSlot(e.slot, Translations.translate(e.slot, "Nominate Menu Timed Out"));
    }
  });
  m.display(slot, 30);
}

async function nominateMenu(slot: number): Promise<void> {
  const pool = loadPool();
  const cd = await cooldownSet(), nom = await nominatedSet();
  // Nominatable = pool − cooldown − already-nominated − the current map (the last is explicit,
  // see isCurrentMap). The current map and existing nominations stay hidden: "you cannot nominate
  // what is already running or already nominated" is self-evident from the map name and the
  // nomination announcement, whereas a cooldown is invisible state only the server knows.
  const available = pool.filter(m => !cd.has(m.name) && !nom.has(m.name) && !isCurrentMap(m.name));
  const recent = pool.filter(m => cd.has(m.name) && !isCurrentMap(m.name));
  if (available.length === 0 && recent.length === 0) {
    Chat.toSlot(slot, Translations.translate(slot, "Nominate None Available"));
    return;
  }
  mapMenu(slot, available, recent, "Nominate Menu Title");
}

async function recordMapStart(map: string): Promise<void> {
  const last = await db.query("SELECT map FROM map_history ORDER BY id DESC LIMIT 1", []);
  if (last.length && String(last[0].map) === map) return;         // already recorded (a reload) -> keep nominations
  await db.execute("INSERT INTO map_history(map, played_at) VALUES(?, ?)", [map, Math.floor(Date.now() / 1000)]);
  await db.execute("DELETE FROM nominations", []);                // new map -> fresh nominations
}

// Plugins persist across a changelevel — the shim has no level-init reload hook, so OnPluginStart fires
// once per plugin-load, NOT per map. Poll Server.mapName (throttled) to catch map transitions.
function pollMapChange(): void {
  if (++frameCounter < 64) return;        // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (!m || m === currentMap) return;     // no change
  currentMap = m;                          // claim synchronously so overlapping polls don't re-fire
  recordMapStart(m).catch(logErr);
}

export async function OnPluginStart(): Promise<void> {
  translations.load("nominations", "common");

  loadPool();   // eager: auto-generate maplist.txt now so the operator can edit it before anyone nominates
  db = await Database.open("mapvote");
  await db.execute("CREATE TABLE IF NOT EXISTS map_history(id INTEGER PRIMARY KEY AUTOINCREMENT, map TEXT NOT NULL, played_at INTEGER NOT NULL)", []);
  await db.execute("CREATE TABLE IF NOT EXISTS nominations(map TEXT PRIMARY KEY, nominator INTEGER NOT NULL)", []);

  // DESCOPED: SM's sm_nominate_addmap (an admin command that force-adds a map to the nomination
  // list at runtime) is intentionally not implemented. maplist.txt is the authoritative,
  // operator-edited pool; there is no runtime pool-mutation surface. Revisit only if an admin
  // "nominate on a player's behalf / add off-pool map" need is proven.
  command("sm_nominate", (cmd) => {
    const slot = cmd.callerSlot;
    if (slot < 0) { cmd.replyT("Must Be In Game"); return HookResult.Handled; }
    const arg = cmd.arg(0);
    if (!arg) { nominateMenu(slot).catch(logErr); return HookResult.Handled; }
    const matches = resolveMap(arg, loadPool());
    if (matches.length === 0) cmd.replyT("No Map Matching", arg);
    else if (matches.length === 1) nominate(slot, matches[0].name).catch(logErr);
    // Disambiguation lists exactly what the player's text matched, so every entry stays selectable
    // and there is no cooldown section — `nominate` re-validates and explains a refusal itself.
    else mapMenu(slot, matches, [], "Nominate Disambiguation Title");
    return HookResult.Handled;
  });

  topmenu.addTab({ id: "nominations", title: "Maps" });
  topmenu.addItem("nominations", {
    id: "nominations:open",
    name: "Nominate",
    flags: 0,
    sheets: ["menu"],
    onSelect: (slot) => { nominateMenu(slot).catch(logErr); },
  });
}

export function OnGameFrame(): void {
  pollMapChange();
}

/**
 * Bare-word chat trigger, mirroring @s2script/rockthevote's `rtv`.
 *
 * `sm_nominate` already answers `!nominate` and `/nominate` through the command system, but nobody
 * types the prefix — players type the word. RTV has accepted a bare `rtv` from the start, so a
 * server running both taught two different conventions for the same pair of features.
 *
 * Prefix parity with RTV: a `!`-prefixed form is SWALLOWED (it was plainly a command), while the
 * bare word passes through to chat, because "nominate" is also an ordinary English word someone
 * may be saying to the server rather than at it.
 */
export function OnClientSayCommand(slot: number, text: string, _teamonly: boolean): HookResultValue {
  const t = text.trim().toLowerCase();
  const bang = t === "!nominate" || t === "!nom";
  if (!bang && t !== "nominate" && t !== "nom") return HookResult.Continue;
  if (slot >= 0) nominateMenu(slot).catch(logErr);
  return bang ? HookResult.Handled : HookResult.Continue;
}
