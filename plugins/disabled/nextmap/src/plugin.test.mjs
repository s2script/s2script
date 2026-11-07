import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
const configSchema = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).s2script.config;

function loadPlugin({ maplist = 'de_inferno\nde_nuke\n', mapName = 'de_default', random = 0, randomStartup, waitForRound } = {}) {
  const commands = [], cvars = new Map(), hooks = new Map(), admins = new Map(), pending = [], logs = [], waits = [], messages = [];
  const values = new Map(Object.entries(configSchema).map(([name, entry]) => [name, entry.default]));
  if (randomStartup !== undefined) values.set('nextmap_random_startup', randomStartup);
  if (waitForRound !== undefined) values.set('nextmap_wait_for_round', waitForRound);
  let randomCalls = 0;
  const files = new Map(maplist === null ? [] : [['maplist.txt', maplist]]);
  const server = {
    mapName, gameTime: 0,
    isMapValid: name => ['de_dust2', 'de_inferno', 'de_mirage', 'de_nuke', 'de_ancient', 'de_anubis'].includes(name.toLowerCase()),
    getCvar: name => cvars.get(name) ?? '0',
    setCvar: (name, value) => cvars.set(name, value),
    command: line => commands.push(line),
  };
  const sdk = {
    Server: server,
    config: {
      readFile: name => files.get(name) ?? null,
      writeFile: (name, value) => files.set(name, value),
      getInt: name => values.get(name), getBool: name => values.get(name),
    },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    hook: { on: (name, handler) => hooks.set(name, handler) },
    publish: () => ({ dispatch: () => 0 }),
    translations: { load() {} }, Translations: { translate: (_slot, key) => key },
    Chat: { toAll: text => messages.push(text) }, ADMFLAG: { CHANGEMAP: 8 }, HookResult: { Handled: 2 },
    delay: ms => { waits.push(ms); return new Promise(resolve => pending.push(resolve)); },
  };
  const math = Object.create(Math);
  math.random = () => { randomCalls++; return random; };
  const context = vm.createContext({
    module: { exports: {} }, require: name => { assert.equal(name, '@s2script/sdk'); return sdk; },
    console: { log: message => logs.push(message) }, Math: math,
  });
  vm.runInContext(code, context);
  const api = context.module.exports;
  api.OnPluginStart();
  return {
    commands, cvars, hooks, admins, logs, waits, messages, values, files, server,
    get randomCalls() { return randomCalls; },
    poll() { for (let i = 0; i < 64; i++) api.OnGameFrame(); },
    async elapse() { for (const resolve of pending.splice(0)) resolve(); await new Promise(setImmediate); },
  };
}

for (const [random, target] of [[0, 'de_inferno'], [0.999999, 'de_nuke']]) {
  test(`startup chooses ${target} from maplist and switches only after the configured delay`, async () => {
    const host = loadPlugin({ randomStartup: true, random, maplist: '// comment\n de_inferno \n# ignored\n\nde_nuke\n' });
    assert.deepEqual(host.commands, []);
    host.poll();
    assert.deepEqual(host.waits, [5000]);
    assert.deepEqual(host.commands, []);
    await host.elapse();
    assert.deepEqual(host.commands, ['changelevel ' + target]);
  });
}

test('startup waits for a live map before choosing or scheduling a switch', async () => {
  const host = loadPlugin({ randomStartup: true, mapName: '' });
  host.cvars.set('mp_timelimit', '1'); host.server.gameTime = 120;
  host.poll(); host.poll();
  assert.deepEqual(host.waits, []);
  assert.equal(host.randomCalls, 0);
  host.server.mapName = 'de_default';
  host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
});

test('startup uses workshop IDs even when the workshop map is not installed yet', async () => {
  const host = loadPlugin({ randomStartup: true, maplist: 'awp_lego_2:3070284539' });
  host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['host_workshop_map 3070284539']);
});

test('startup filters malformed entries and unavailable stock maps before choosing', async () => {
  const host = loadPlugin({ randomStartup: true, maplist: 'de_fake\nde_nuke;quit\nawp_lego_2:123;quit\n:123\nde_inferno', random: 0 });
  host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
});

for (const maplist of ['// empty\n# comment', 'de_fake\nawp_lego_2:bad-id\nde_nuke;quit']) {
  test(`startup leaves the server alone and logs once with no usable maps: ${JSON.stringify(maplist)}`, async () => {
    const host = loadPlugin({ randomStartup: true, maplist });
    host.poll(); host.poll(); await host.elapse();
    assert.deepEqual(host.waits, []);
    assert.deepEqual(host.commands, []);
    assert.equal(host.logs.filter(line => line.includes('startup')).length, 1);
    host.files.set('maplist.txt', 'de_nuke');
    host.poll(); await host.elapse();
    assert.deepEqual(host.commands, [], 'later polls do not repeat the startup choice');
  });
}

test('a selected map already running does not restart or switch again on later maps', async () => {
  const host = loadPlugin({ randomStartup: true, mapName: 'DE_INFERNO' });
  host.poll(); await host.elapse();
  assert.deepEqual(host.waits, []);
  assert.equal(host.randomCalls, 1);
  host.server.mapName = 'de_nuke'; host.poll(); await host.elapse();
  assert.deepEqual(host.commands, []);
  assert.equal(host.randomCalls, 1);
});

test('startup runs once, then sequential rotation and admin overrides still work', async () => {
  const host = loadPlugin({ randomStartup: true });
  host.poll(); host.poll();
  assert.deepEqual(host.waits, [5000], 'only one startup switch is scheduled');
  await host.elapse();
  host.server.mapName = 'de_inferno'; host.poll(); host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
  assert.equal(host.cvars.get('nextlevel'), 'de_nuke');
  host.cvars.set('mp_maxrounds', '1');
  host.hooks.get('round_end')(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_inferno', 'changelevel de_nuke']);
  host.server.mapName = 'de_nuke'; host.poll();
  host.admins.get('sm_setnextmap')({ arg: () => 'de_mirage', replyT() {} });
  host.hooks.get('round_end')(); await host.elapse();
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_inferno', 'changelevel de_nuke', 'changelevel de_mirage']);
  assert.equal(host.randomCalls, 1);
});

test('an external map change during the startup delay cancels the stale switch', async () => {
  const host = loadPlugin({ randomStartup: true });
  host.poll();
  host.server.mapName = 'de_nuke'; host.poll();
  await host.elapse();
  assert.deepEqual(host.commands, []);
  assert.equal(host.cvars.get('nextlevel'), 'de_inferno');
  assert.equal(host.randomCalls, 1);
});

test('a missing maplist still generates the template and selects a map from it', async () => {
  const host = loadPlugin({ randomStartup: true, maplist: null });
  assert.match(host.files.get('maplist.txt'), /de_dust2/);
  host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_dust2']);
});

test('default config leaves the initial map alone and keeps normal rotation available', async () => {
  const host = loadPlugin();
  host.poll(); await host.elapse();
  assert.deepEqual(host.waits, []);
  assert.deepEqual(host.commands, []);
  assert.equal(host.randomCalls, 0);
  assert.equal(host.cvars.get('nextlevel'), 'de_inferno');
  host.cvars.set('mp_maxrounds', '1'); host.hooks.get('round_end')();
  await host.elapse();
  assert.deepEqual(host.commands, [], 'round waiting is enabled by default');
  assert.equal(host.messages.at(-1), 'Changing After Round');
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
});

test('disabling random startup still consumes the startup choice before later config changes', async () => {
  const host = loadPlugin({ randomStartup: false });
  host.poll();
  host.values.set('nextmap_random_startup', true);
  host.poll();
  host.server.mapName = 'de_mirage'; host.poll(); await host.elapse();
  assert.equal(host.randomCalls, 0);
  assert.deepEqual(host.commands, []);
  assert.deepEqual(host.waits, []);
});

test('the random startup flag is read when the first live map is observed', async () => {
  const host = loadPlugin({ randomStartup: false, mapName: '' });
  host.poll();
  host.values.set('nextmap_random_startup', true);
  host.server.mapName = 'de_default'; host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
});

test('enabled random startup stays prompt when round waiting is disabled', async () => {
  const host = loadPlugin({ randomStartup: true, waitForRound: false });
  host.poll(); await host.elapse();
  assert.deepEqual(host.commands, ['changelevel de_inferno']);
  assert.equal(host.messages.at(-1), 'Changing To');
});

for (const trigger of ['timelimit', 'maxrounds']) {
  test(`round waiting disabled applies ${trigger} rotation as soon as the delay expires`, async () => {
    const host = loadPlugin({ randomStartup: false, waitForRound: false, mapName: 'de_inferno' });
    host.poll();
    if (trigger === 'timelimit') {
      host.cvars.set('mp_timelimit', '1'); host.server.gameTime = 61; host.poll();
    } else {
      host.cvars.set('mp_maxrounds', '1'); host.hooks.get('round_end')();
    }
    assert.deepEqual(host.waits, [5000]);
    assert.deepEqual(host.commands, []);
    assert.equal(host.messages.at(-1), 'Changing To');
    await host.elapse();
    assert.deepEqual(host.commands, ['changelevel de_nuke']);
    host.hooks.get('round_prestart')();
    assert.deepEqual(host.commands, ['changelevel de_nuke']);
  });
}

test('timelimit expiry during play waits for the following round prestart', async () => {
  const host = loadPlugin({ mapName: 'de_inferno' });
  host.poll();
  host.cvars.set('mp_timelimit', '1'); host.server.gameTime = 61;
  host.poll(); await host.elapse();
  assert.equal(host.messages.at(-1), 'Changing After Round');
  assert.deepEqual(host.commands, [], 'the elapsed countdown must not interrupt the round');
  host.hooks.get('round_end')();
  assert.deepEqual(host.commands, [], 'the round-end results remain visible');
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_nuke']);
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_nuke'], 'apply the queued change once');
});

test('a round limit leaves the round-end results up until the next prestart', async () => {
  const host = loadPlugin({ mapName: 'de_inferno' });
  host.poll(); host.cvars.set('mp_maxrounds', '1');
  host.hooks.get('round_end')(); await host.elapse();
  assert.deepEqual(host.commands, []);
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_nuke']);
});

test('a countdown that finishes after prestart waits for another prestart', async () => {
  const host = loadPlugin({ mapName: 'de_inferno' });
  host.poll(); host.cvars.set('mp_maxrounds', '1');
  host.hooks.get('round_end')();
  host.hooks.get('round_prestart')?.();
  await host.elapse();
  assert.deepEqual(host.commands, [], 'a long countdown must not switch during a new round');
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel de_nuke']);
});

test('an external map change cancels a ready rotation even before the next map poll', async () => {
  const host = loadPlugin({ mapName: 'de_inferno' });
  host.poll(); host.cvars.set('mp_maxrounds', '1');
  host.hooks.get('round_end')(); await host.elapse();
  host.server.mapName = 'de_mirage';
  host.hooks.get('round_prestart')?.();
  assert.deepEqual(host.commands, []);
});

for (const matchEndsBeforeDelay of [true, false]) {
  test(`the final match panel allows rotation without a following round (panel before delay: ${matchEndsBeforeDelay})`, async () => {
    const host = loadPlugin({ mapName: 'de_inferno' });
    host.poll(); host.cvars.set('mp_maxrounds', '1');
    host.hooks.get('round_end')();
    if (matchEndsBeforeDelay) host.hooks.get('cs_win_panel_match')?.();
    await host.elapse();
    if (!matchEndsBeforeDelay) {
      assert.deepEqual(host.commands, []);
      host.hooks.get('cs_win_panel_match')();
    }
    assert.deepEqual(host.commands, ['changelevel de_nuke']);
  });
}
