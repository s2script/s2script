import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
const configSchema = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).s2script.config;

function loadPlugin({ settings = {}, installed = [], loggedOn = true, currentMap = [], mapName = 'de_test' } = {}) {
  const values = new Map(Object.entries(configSchema).map(([name, entry]) => [name, entry.default]));
  for (const [k, v] of Object.entries(settings)) values.set(k, v);
  const calls = [], commands = [], logs = [], admins = new Map(), pre = new Map(), waiting = new Map();
  const onConnected = [], onChange = [];
  const state = { installed: new Set(installed), loggedOn, mounted: [], currentMap };
  const sdk = {
    config: {
      getBool: k => values.get(k), getString: k => values.get(k), getFloat: k => values.get(k),
      onChange: h => onChange.push(h),
    },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    hook: { onPre: (name, handler) => pre.set(name, handler) },
    Server: { command: c => commands.push(c), mapName },
    ADMFLAG: { ROOT: 1 << 14 }, HookResult: { Continue: 0, Handled: 2 },
    Steam: { isLoggedOn: () => state.loggedOn, onConnected: h => onConnected.push(h) },
    Workshop: {
      isInstalled: id => state.installed.has(id),
      installInfo: id => (state.installed.has(id) ? { id, folder: `/ws/${id}` } : null),
      download: id => { calls.push(['download', id]); return new Promise(resolve => waiting.set(id, resolve)); },
    },
    Addons: {
      status: () => ({ mount: true, mapAddons: true, clientHandshake: true }),
      configure: o => calls.push(['configure', JSON.stringify(o)]),
      setExtraAddons: ids => calls.push(['extra', ids.join(',')]),
      setClientAddons: ids => calls.push(['client', ids.join(',')]),
      mount: (id, folder) => { calls.push(['mount', id, folder]); if (state.mounted.includes(id)) return 'already'; state.mounted.push(id); return 'mounted'; },
      unmount: id => { calls.push(['unmount', id]); state.mounted = state.mounted.filter(x => x !== id); return true; },
      mounted: () => [...state.mounted],
      currentMap: () => state.currentMap,
      clearClientCache: () => calls.push(['clear']),
    },
  };
  const context = vm.createContext({
    module: { exports: {} }, require: name => { assert.equal(name, '@s2script/sdk'); return sdk; },
    console: { log: m => logs.push(m) },
  });
  vm.runInContext(code, context);
  const api = context.module.exports;
  const settle = () => new Promise(setImmediate);
  const reply = [];
  const cmd = (...args) => ({ arg: n => args[n] ?? '', reply: m => reply.push(m) });
  return {
    api, calls, commands, logs, admins, pre, state, reply, values,
    start() { api.OnPluginStart(); return settle(); },
    async finish(id, ok = true) {
      const resolve = waiting.get(id); waiting.delete(id);
      if (ok) state.installed.add(id);
      resolve({ id, result: ok ? 1 : 2, ok }); await settle();
    },
    run(name, ...args) { return admins.get(name)(cmd(...args)); },
    async connect() { state.loggedOn = true; for (const h of onConnected) h(); await settle(); },
    async changeConfig(k, v) { values.set(k, v); for (const h of onChange) h({}); await settle(); },
  };
}

test('with no addons configured it never touches the addon state', async () => {
  const p = loadPlugin();
  await p.start();
  assert.deepEqual(p.calls, []);
  assert.equal(p.pre.get('player_disconnect')({ getInt: () => 54 }), 0);
});

test('mounts installed addons, downloads the rest, and reloads the map when they land', async () => {
  const p = loadPlugin({ settings: { addons_extra: '111, 222 111', addons_client_extra: '333' }, installed: ['111'] });
  await p.start();
  assert.deepEqual(p.calls.map(c => c.join(' ')), [
    'configure {"extraAddonTimeout":10,"connectTimeout":30,"cacheClients":false,"cacheSeconds":0}',
    'extra 111,222', 'client 333', 'mount 111 /ws/111', 'download 222',
  ]);
  p.calls.length = 0;
  await p.finish('222');
  assert.deepEqual(p.calls.map(c => c.join(' ')), ['mount 111 /ws/111', 'mount 222 /ws/222']);
  assert.deepEqual(p.commands, ['changelevel de_test']);
});

test('a workshop map reloads through host_workshop_map, and the map\'s own addon is not mounted', async () => {
  const p = loadPlugin({ settings: { addons_extra: '900,222' }, currentMap: ['900'] });
  await p.start();
  assert.ok(!p.calls.some(c => c[0] === 'mount' && c[1] === '900'));
  await p.finish('222');
  assert.deepEqual(p.commands, ['host_workshop_map 900']);
});

test('downloads asked for before Steam logs on start once it does', async () => {
  const p = loadPlugin({ settings: { addons_extra: '222' }, loggedOn: false });
  await p.start();
  assert.ok(!p.calls.some(c => c[0] === 'download'));
  await p.connect();
  assert.ok(p.calls.some(c => c[0] === 'download' && c[1] === '222'));
});

test('commands change the lists, and an addon taken off the list is unmounted', async () => {
  const p = loadPlugin({ settings: { addons_extra: '111' }, installed: ['111', '444'] });
  await p.start();
  p.run('sm_addons_add', '444');
  assert.deepEqual(p.state.mounted, ['111', '444']);
  p.run('sm_addons_remove', '111');
  assert.deepEqual(p.state.mounted, ['444']);
  assert.ok(p.calls.some(c => c.join(' ') === 'extra 444'));
  p.run('sm_addons_add', 'abc');
  assert.match(p.reply.at(-1), /usage/);
});

test('loop-shutdown disconnect messages are hidden only when asked', async () => {
  const p = loadPlugin({ settings: { addons_extra: '111', addons_block_disconnect_messages: true }, installed: ['111'] });
  await p.start();
  const onPre = p.pre.get('player_disconnect');
  assert.equal(onPre({ getInt: () => 54 }), 2);
  assert.equal(onPre({ getInt: () => 2 }), 0);
  await p.changeConfig('addons_block_disconnect_messages', false);
  assert.equal(onPre({ getInt: () => 54 }), 0);
});
