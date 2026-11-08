import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
const configSchema = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).s2script.config;

function loadPlugin({ maplist = null, loggedOn = true, installed = [], settings = {} } = {}) {
  const values = new Map(Object.entries(configSchema).map(([name, entry]) => [name, entry.default]));
  for (const [k, v] of Object.entries(settings)) values.set(k, v);
  const files = new Map(maplist === null ? [] : [['maplist.txt', maplist]]);
  const logs = [], requests = [], asks = [], waiting = new Map(), admins = new Map(), timers = [];
  const onConnected = [];
  const steam = { loggedOn, installed: new Set(installed) };
  const sdk = {
    config: {
      readFile: name => files.get(name) ?? null,
      getBool: name => values.get(name), getString: name => values.get(name),
    },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    after: (ms, fn) => { const t = { ms, fn, alive: true, kill() { t.alive = false; return true; } }; timers.push(t); return t; },
    ADMFLAG: { CHANGEMAP: 8 }, HookResult: { Handled: 2 },
    Steam: { isLoggedOn: () => steam.loggedOn, onConnected: h => onConnected.push(h) },
    Workshop: {
      isInstalled: id => steam.installed.has(id),
      request: id => { asks.push(id); return true; },
      download: id => { requests.push(id); return new Promise(resolve => waiting.set(id, resolve)); },
    },
  };
  const context = vm.createContext({
    module: { exports: {} }, require: name => { assert.equal(name, '@s2script/sdk'); return sdk; },
    console: { log: message => logs.push(message) },
  });
  vm.runInContext(code, context);
  const api = context.module.exports;
  const settle = () => new Promise(setImmediate);
  return {
    api, logs, requests, asks, admins, timers, steam, files, values,
    start() { api.OnPluginStart(); return settle(); },
    async connect() { steam.loggedOn = true; for (const h of onConnected) h(); await settle(); },
    async finish(id, result = 1) {
      const resolve = waiting.get(id); waiting.delete(id);
      if (result === 1) steam.installed.add(id);
      resolve({ id, result, ok: result === 1 }); await settle();
    },
    async mapStart() { api.OnMapStart('de_test'); await settle(); },
  };
}

const MAPLIST = 'de_dust2\n// awp_old:111\nawp_lego_2:3070284539\n# x:222\nfy_pool:3121217565\nbad:abc\n:333\naim:3070284539\n';

test('downloads the maplist workshop ids that are not installed, one at a time', async () => {
  const host = loadPlugin({ maplist: MAPLIST, installed: ['3121217565'], settings: { workshopmaps_items: '444, 555 3070284539' } });
  await host.start();
  assert.deepEqual(host.requests, ['3070284539']);
  await host.finish('3070284539');
  assert.deepEqual(host.requests, ['3070284539', '444']);
  await host.finish('444', 9);
  await host.finish('555');
  assert.deepEqual(host.requests, ['3070284539', '444', '555']);
  assert.deepEqual(host.logs, [
    '[workshopmaps] plugin start: downloading 3 workshop map(s)',
    '[workshopmaps] 444: download failed (EResult 9)',
    '[workshopmaps] 2/3 ready',
  ]);
  assert.ok(host.timers.every(t => !t.alive), 'each timeout is cancelled once its result arrives');
});

test('waits for Steam to log on, then runs', async () => {
  const host = loadPlugin({ maplist: 'awp_lego_2:3070284539', loggedOn: false });
  await host.start();
  assert.deepEqual(host.requests, []);
  await host.connect();
  assert.deepEqual(host.requests, ['3070284539']);
});

test('map start checks again; a request during a pass runs one more pass after it', async () => {
  const host = loadPlugin({ maplist: 'a:1' });
  await host.start();
  host.files.set('maplist.txt', 'a:1\nb:2');
  await host.mapStart();                         // pass in progress: queued
  assert.deepEqual(host.requests, ['1']);
  await host.finish('1');
  assert.deepEqual(host.requests, ['1', '2']);
  await host.finish('2');
  await host.mapStart();                         // everything installed: nothing to do
  assert.deepEqual(host.requests, ['1', '2']);
});

test('map start re-check and the maplist source can be turned off', async () => {
  const host = loadPlugin({ maplist: 'a:1', settings: { workshopmaps_from_maplist: false, workshopmaps_check_on_map_start: false } });
  await host.start();
  assert.deepEqual(host.requests, []);
  host.values.set('workshopmaps_items', '7');
  await host.mapStart();
  assert.deepEqual(host.requests, []);
});

test('a download with no result times out and the list moves on', async () => {
  const host = loadPlugin({ maplist: 'a:1\nb:2' });
  await host.start();
  assert.equal(host.timers[0].ms, 15 * 60 * 1000);
  host.timers[0].fn();
  await new Promise(setImmediate);
  assert.deepEqual(host.requests, ['1', '2']);
  assert.equal(host.logs[1], '[workshopmaps] 1: no result from Steam; will retry');
});

test('a timed-out item is asked for again without adding another waiter', async () => {
  const host = loadPlugin({ maplist: 'a:1' });
  await host.start();
  host.timers[0].fn();
  await new Promise(setImmediate);
  await host.mapStart();
  assert.deepEqual(host.requests, ['1'], 'one waiting download per item');
  assert.deepEqual(host.asks, ['1'], 'Steam is asked again');
  await host.finish('1');
  assert.equal(host.logs.at(-1), '[workshopmaps] 1/1 ready');
  await host.mapStart();
  assert.deepEqual(host.requests, ['1']);
});

test('sm_workshop_sync reports the list and starts a pass', async () => {
  const host = loadPlugin({ maplist: 'a:1\nb:2', installed: ['1'], loggedOn: false });
  await host.start();
  const replies = [];
  const cmd = { reply: text => replies.push(text) };
  host.admins.get('sm_workshop_sync')(cmd);
  host.steam.loggedOn = true;
  host.admins.get('sm_workshop_sync')(cmd);
  await new Promise(setImmediate);
  assert.deepEqual(replies, [
    '[workshopmaps] the server is not logged on to Steam',
    '[workshopmaps] 2 workshop map(s) listed, 1 to download',
  ]);
  assert.deepEqual(host.requests, ['2']);
});
