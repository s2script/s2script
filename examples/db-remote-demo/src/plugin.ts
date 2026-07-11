// db-remote-demo — opens the operator-configured "stats" (mysql) + "prefs" (postgres) connections,
// round-trips CREATE/INSERT/SELECT against each, checks a BIGINT reads back as a decimal string, and
// proves the game frame advances WHILE a query is in flight (async, off-thread).
import { Database } from "@s2script/db";
import { OnGameFrame } from "@s2script/frame";

let frames = 0;
OnGameFrame.subscribe(() => { frames++; });

async function exercise(name: string, autoInc: string): Promise<void> {
  try {
    const db = await Database.open(name);
    await db.execute(`CREATE TABLE IF NOT EXISTS demo (id ${autoInc}, sid BIGINT, note TEXT)`);
    const before = frames;
    await db.execute("INSERT INTO demo (sid, note) VALUES (?, ?)", ["76561199000000001", "hello from " + name]);
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
