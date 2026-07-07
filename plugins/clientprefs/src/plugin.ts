// @s2script/clientprefs (plugin) — the cookie DB lifecycle: load a client's cookies from SQLite into
// the core cache on connect, flush the dirty ones back on disconnect. The cookie API itself is the
// @s2script/clientprefs MODULE; this plugin only drives persistence.
import { Database } from "@s2script/db";
import { Clients, Client } from "@s2script/clients";
import { OnGameFrame } from "@s2script/frame";

// The core natives (injected globals; not in the module's typed surface).
declare function __s2_cookie_load(steamid: string, name: string, value: string, updated: number): void;
declare function __s2_cookie_mark_cached(steamid: string): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;
declare function __s2_cookie_clear(steamid: string): void;
declare function __s2_cookie_take_offline_writes(): Array<[string, string, string, number]>;

let db: Database | null = null;

export async function onLoad(): Promise<void> {
  try {
    db = await Database.open("clientprefs");
    await db.execute(
      "CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))"
    );
    Clients.onPutInServer(loadCookies);
    Clients.onDisconnect(saveCookies);
    OnGameFrame.subscribe(drainOfflineWrites);
    console.log("[clientprefs] onLoad — table ready, lifecycle hooked");
  } catch (e) {
    console.log("[clientprefs] onLoad ERROR: " + String(e));
  }
}

async function loadCookies(client: Client): Promise<void> {
  if (!db || client.steamId === "0") return;   // skip bots
  const steamId = client.steamId;
  try {
    const rows = await db.query("SELECT name, value, updated FROM cookies WHERE steamid = ?", [steamId]);
    for (const row of rows) __s2_cookie_load(steamId, String(row.name), String(row.value), Number(row.updated));
    __s2_cookie_mark_cached(steamId);
  } catch (e) {
    console.log("[clientprefs] load ERROR for " + steamId + ": " + String(e));
  }
}

async function saveCookies(client: Client): Promise<void> {
  if (!db || client.steamId === "0") return;   // skip bots
  const steamId = client.steamId;
  const dirty = __s2_cookie_get_dirty(steamId);   // capture synchronously
  __s2_cookie_clear(steamId);                     // then clear (writes below use the captured values)
  const now = Math.floor(Date.now() / 1000);
  try {
    for (const name of Object.keys(dirty)) {
      await db.execute(
        "INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
        [steamId, name, dirty[name], now]
      );
    }
  } catch (e) {
    console.log("[clientprefs] save ERROR for " + steamId + ": " + String(e));
  }
}

// setAuthId (SetAuthIdCookie parity) writes for a SteamID that may not be connected — it can't ride
// the onDisconnect flush, so drain the core's offline-write queue every frame (cheap idle check).
function drainOfflineWrites(): void {
  if (!db) return;
  const writes = __s2_cookie_take_offline_writes();
  if (writes.length === 0) return;
  for (const [steamid, name, value, updated] of writes) {
    db.execute(
      "INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
      [steamid, name, value, updated]
    ).catch((e) => console.log("[clientprefs] offline-write ERROR: " + String(e)));
  }
}

export function onUnload(): void { console.log("[clientprefs] onUnload"); }
