// Prints "<buildid> <manifest> <timeupdated>" for an app branch's depot, read straight from Steam's
// PICS (anonymous login), so the cs2-update cron does not depend on api.steamcmd.net's cache, which
// can lag a new build by an hour or more.
//   node steam-build.cjs <steam-user module dir> [app] [depot] [branch]
'use strict';
const [modDir, app = '730', depot = '2347773', branch = 'public'] = process.argv.slice(2);
const SteamUser = require(require('path').join(modDir, 'steam-user'));
const client = new SteamUser({ enablePicsCache: false, webCompatibilityMode: true, autoRelogin: false });
const fail = (msg) => { console.error(`steam-build: ${msg}`); process.exit(1); };
setTimeout(() => fail('timed out'), 60000).unref();
client.on('error', (e) => fail(e.message));
client.on('loggedOn', async () => {
  try {
    const info = await client.getProductInfo([Number(app)], [], true);
    const depots = info.apps?.[app]?.appinfo?.depots ?? {};
    const b = depots.branches?.[branch];
    const m = depots[depot]?.manifests?.[branch];
    const manifest = typeof m === 'string' ? m : m?.gid;
    if (!b?.buildid || !manifest) fail(`no ${branch} build or depot ${depot} manifest in appinfo`);
    console.log(`${b.buildid} ${manifest} ${b.timeupdated ?? ''}`.trim());
    client.logOff();
    setTimeout(() => process.exit(0), 200);
  } catch (e) {
    fail(e.message);
  }
});
client.logOn({ anonymous: true });
