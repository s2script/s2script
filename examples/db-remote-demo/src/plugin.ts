// db-remote-demo — opens the operator-configured "stats" (mysql) + "prefs" (postgres) connections,
// round-trips CREATE/INSERT/SELECT against each, checks a BIGINT reads back as a decimal string, and
// proves the game frame advances WHILE a query is in flight (async, off-thread).
import { Database } from "@s2script/sdk/db";
import { OnGameFrame } from "@s2script/sdk/frame";

let frames = 0;
OnGameFrame.subscribe(() => { frames++; });

async function exercise(name: string, autoInc: string): Promise<void> {
  try {
    const db = await Database.open(name);
    await db.execute(`CREATE TABLE IF NOT EXISTS demo (id ${autoInc}, sid BIGINT, note TEXT)`);
    const before = frames;
    // sid is a BIGINT literal (not a bound string): Postgres is strictly typed and rejects a
    // text-bound param into a BIGINT column (MySQL coerces; PG does not). Binding a big value into a
    // PG bigint needs a number (precision-limited), a `?::bigint` cast, or — as here — a SQL literal.
    await db.execute("INSERT INTO demo (sid, note) VALUES (76561199000000001, ?)", ["hello from " + name]);
    const rows = await db.query("SELECT sid, note FROM demo ORDER BY id DESC LIMIT 1");
    const sid = rows.length ? rows[0].sid : null;
    console.log(`[db-remote-demo] ${name}: rows=${rows.length} sid=${JSON.stringify(sid)} typeof=${typeof sid} frames+=${frames - before}`);
    await db.close();
  } catch (e) {
    console.log(`[db-remote-demo] ${name}: ERROR ${e}`);
  }
}

export function onLoad(): void {
  console.log("[db-remote-demo] onLoad — exercising mysql + postgres");
  exercise("stats", "INT AUTO_INCREMENT PRIMARY KEY");        // mysql
  exercise("prefs", "SERIAL PRIMARY KEY");                    // postgres
}
