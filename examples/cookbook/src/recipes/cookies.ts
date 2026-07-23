import type { Recipe } from "../recipe.ts";
import { Database } from "@s2script/sdk/db";
import { Cookies } from "@s2script/sdk/cookies";

declare function __s2_cookie_load(steamid: string, name: string, value: string, updated?: number): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;

const FAKE = "76561199999999999";          // demo_boots — the demo owns its flush
const FAKE_OFFLINE = "76561199888888888";  // authid_boots — persisted ONLY by the plugin's offline drain

/**
 * The cookie stack for synthetic SteamIDs (bots have no cookies, so the real
 * client lifecycle needs a human client). Exercises: (1) the basic
 * cache+DB round-trip; (2) the OFFLINE setAuthId path — persisted only by the
 * clientprefs plugin's OnGameFrame drain, via a SteamID this recipe never
 * flushes itself; (3) an empty-string round-trip; (4) getTime.
 */
export const cookiesRecipe: Recipe = {
  name: "cookies",
  describe: "cookie cache+DB round-trip, offline setAuthId, empty-string, getTime (sm_cookies)",
  register(ctx) {
    ctx.commands.register("sm_cookies", async (cmd) => {
      try {
        const db = await Database.open("clientprefs");
        await db.execute("CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))");

        // (1) basic round-trip: load FAKE's cookies -> cache, get+increment+set, flush dirty -> DB.
        const rows = await db.query("SELECT name, value FROM cookies WHERE steamid = ?", [FAKE]);
        for (const row of rows) __s2_cookie_load(FAKE, String(row.name), String(row.value));
        const fakeClient = { steamId: FAKE } as any;
        const boots = Cookies.register("demo_boots", { default: "0" });
        const n = parseInt(Cookies.get(fakeClient, boots) || "0", 10) + 1;
        Cookies.set(fakeClient, boots, String(n));
        const dirty = __s2_cookie_get_dirty(FAKE);
        for (const name of Object.keys(dirty)) {
          await db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)", [FAKE, name, dirty[name], 0]);
        }

        // (2) OFFLINE setAuthId: read the current value from the DB, increment, and setAuthId — this
        // recipe does NOT flush FAKE_OFFLINE, so if authid_boots climbs across restarts it was the
        // clientprefs plugin's OnGameFrame offline drain that persisted it.
        const offRows = await db.query("SELECT value FROM cookies WHERE steamid = ? AND name = ?", [FAKE_OFFLINE, "authid_boots"]);
        const m = (offRows.length ? parseInt(String(offRows[0].value), 10) : 0) + 1;
        const authid = Cookies.register("authid_boots", { default: "0" });
        Cookies.setAuthId(FAKE_OFFLINE, authid, String(m));

        // (3) empty-string round-trip: a stored "" must read back "" (not the default).
        const es = Cookies.register("empty_test", { default: "DEFAULT" });
        Cookies.set(fakeClient, es, "");
        const esVal = Cookies.get(fakeClient, es);

        // (4) getTime.
        const t = Cookies.getTime(fakeClient, boots);

        console.log("[cookbook] cookies: demo_boots=" + n + " authid_boots=" + m
          + " empty=[" + esVal + "] getTime=" + t);
        cmd.reply(`[cookbook] cookies: demo_boots=${n} authid_boots=${m} empty=[${esVal}] getTime=${t}`);
        await db.close();
      } catch (e) {
        console.log("[cookbook] cookies ERROR: " + String(e));
        cmd.reply(`[cookbook] cookies ERROR: ${String(e)}`);
      }
    });
  },
};
