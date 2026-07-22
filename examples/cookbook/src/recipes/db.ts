import type { Recipe } from "../recipe.ts";
import { Database } from "@s2script/sdk/db";

/**
 * The same @s2script/db API drives SQLite, MySQL, and Postgres alike — only
 * the connection config (resolved by name from the operator's config) differs.
 * Every call is off-thread behind a Promise, so none of this ever blocks the
 * tick; the frame counter below proves it advances while a query is in flight.
 *
 *   cb_db          round-trip the local SQLite "demo" connection; proves the
 *                  primitive persists across a server restart.
 *   cb_db_remote   round-trip the operator-configured "stats" (mysql) + "prefs"
 *                  (postgres) connections; checks a BIGINT reads back as a
 *                  decimal string.
 */
export const dbRecipe: Recipe = {
  name: "db",
  describe: "round-trip SQLite (cb_db) or operator-configured mysql/postgres (cb_db_remote)",
  register(ctx) {
    let frames = 0;
    ctx.server.onGameFrame(() => { frames += 1; });

    ctx.commands.register("cb_db", (cmd) => {
      cmd.reply("querying SQLite…");
      (async () => {
        try {
          const db = await Database.open("demo");
          await db.execute("CREATE TABLE IF NOT EXISTS boots (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT)");
          const res = await db.execute("INSERT INTO boots (at) VALUES (?)", ["load"]);
          const rows = await db.query("SELECT COUNT(*) AS n FROM boots", []);
          const n = rows.length ? rows[0].n : 0;
          console.log("[cookbook] db: inserted id=" + res.lastInsertId + " total boots=" + n);
          cmd.reply(`sqlite ok — inserted id=${res.lastInsertId} total boots=${n}`);
          await db.close();
        } catch (e) {
          console.log("[cookbook] db ERROR: " + String(e));
          cmd.reply("sqlite ERROR: " + String(e));
        }
      })();
    });

    // sid is a BIGINT literal (not a bound string): Postgres is strictly typed and rejects a
    // text-bound param into a BIGINT column (MySQL coerces; PG does not). Binding a big value into
    // a PG bigint needs a number (precision-limited), a `?::bigint` cast, or — as here — a SQL literal.
    async function exercise(name: string, autoInc: string): Promise<void> {
      try {
        const db = await Database.open(name);
        await db.execute(`CREATE TABLE IF NOT EXISTS demo (id ${autoInc}, sid BIGINT, note TEXT)`);
        const before = frames;
        await db.execute("INSERT INTO demo (sid, note) VALUES (76561199000000001, ?)", ["hello from " + name]);
        const rows = await db.query("SELECT sid, note FROM demo ORDER BY id DESC LIMIT 1");
        const sid = rows.length ? rows[0].sid : null;
        console.log(`[cookbook] db_remote ${name}: rows=${rows.length} sid=${JSON.stringify(sid)} typeof=${typeof sid} frames+=${frames - before}`);
        await db.close();
      } catch (e) {
        console.log(`[cookbook] db_remote ${name}: ERROR ${e}`);
      }
    }

    ctx.commands.register("cb_db_remote", (cmd) => {
      cmd.reply("exercising mysql + postgres — watch the log");
      exercise("stats", "INT AUTO_INCREMENT PRIMARY KEY"); // mysql
      exercise("prefs", "SERIAL PRIMARY KEY");              // postgres
    });
  },
};
