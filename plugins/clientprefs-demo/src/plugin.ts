// clientprefs-demo — proves the cookie cache + DB round-trip for a synthetic SteamID (bots have no
// cookies, so the real client lifecycle is a deferred human-client test). A boot counter climbs
// across restarts: load DB -> cache, get+increment+set, flush dirty -> DB.
import { Database } from "@s2script/db";
import { Cookies } from "@s2script/cookies";

declare function __s2_cookie_load(steamid: string, name: string, value: string): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;

const FAKE = "76561199999999999";

export async function onLoad(): Promise<void> {
  try {
    const db = await Database.open("clientprefs");
    await db.execute("CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))");
    // load the fake client's cookies into the cache
    const rows = await db.query("SELECT name, value FROM cookies WHERE steamid = ?", [FAKE]);
    for (const row of rows) __s2_cookie_load(FAKE, String(row.name), String(row.value));
    // register + get + increment + set (a fake Client is just { steamId })
    const fakeClient = { steamId: FAKE } as any;
    const boots = Cookies.register("demo_boots", { default: "0" });
    const n = parseInt(Cookies.get(fakeClient, boots) || "0", 10) + 1;
    Cookies.set(fakeClient, boots, String(n));
    // flush the dirty set to the DB (what the clientprefs plugin does on disconnect)
    const dirty = __s2_cookie_get_dirty(FAKE);
    for (const name of Object.keys(dirty)) {
      await db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)", [FAKE, name, dirty[name], 0]);
    }
    console.log("[clientprefs-demo] onLoad — demo_boots=" + n + " (persisted via cookie cache + DB)");
    await db.close();
  } catch (e) {
    console.log("[clientprefs-demo] onLoad ERROR: " + String(e));
  }
}

export function onUnload(): void { console.log("[clientprefs-demo] onUnload"); }
