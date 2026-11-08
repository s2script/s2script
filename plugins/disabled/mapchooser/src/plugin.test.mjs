import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const compile = path => transformSync(readFileSync(new URL(path, import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
const code = compile('./plugin.ts');
const configSchema = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).s2script.config;

function loadPlugin({
  maplist = 'de_dust2\nde_inferno\nde_mirage\nde_nuke\nawp_lego_2:3070284539\n', mapName = 'de_dust2',
  nominations = [], history = [], settings = {}, withNextmap = false, postpone = false,
} = {}) {
  const commands = [], cvars = new Map(), hooks = new Map(), admins = new Map(), chat = [], logs = [], nextmapCalls = [];
  const values = new Map(Object.entries(configSchema).map(([name, entry]) => [name, entry.default]));
  for (const [k, v] of Object.entries(settings)) values.set(k, v);
  let vote = null;
  let published = null;
  const server = {
    mapName, gameTime: 0,
    getCvar: name => cvars.get(name) ?? '0',
    setCvar: (name, value) => cvars.set(name, String(value)),
    command: line => commands.push(line),
  };
  const sdk = {
    Server: server,
    config: {
      readFile: name => (name === 'maplist.txt' ? maplist : null),
      getInt: name => values.get(name), getBool: name => values.get(name), getFloat: name => values.get(name),
    },
    Database: {
      open: async () => ({
        execute: async () => {},
        query: async sql => sql.includes('FROM nominations') ? nominations.map(map => ({ map }))
          : sql.includes('FROM map_history') ? history.map(map => ({ map })) : [],
      }),
    },
    Vote: {
      isActive: () => vote !== null,
      start: options => { vote = options; return true; },
      cancel: () => { vote = null; },
    },
    hook: { on: (name, handler) => hooks.set(name, handler) },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    translations: { load() {} },
    Translations: { translate: (_slot, key, ...args) => [key, ...args].join('|') },
    Chat: { toAll: text => chat.push(text) },
    ADMFLAG: { CHANGEMAP: 8 }, HookResult: { Continue: 0, Handled: 2 },
    publish: (_name, impl) => {
      published = impl;
      return { dispatch: () => (postpone ? 2 : 0) };
    },
    watchOptional: (name, attach) => {
      if (name === '@s2script/nextmap' && withNextmap) {
        attach({ setNextMap: map => { nextmapCalls.push(map); return true; }, on: () => ({ dispose() {} }) }, { own: value => value });
      }
    },
  };
  const math = Object.create(Math); math.random = () => 0;
  const context = vm.createContext({
    module: { exports: {} }, require: name => { assert.equal(name, '@s2script/sdk'); return sdk; },
    console: { log: message => logs.push(message) }, Math: math,
  });
  vm.runInContext(code, context);
  const api = context.module.exports;
  const host = {
    commands, cvars, hooks, admins, chat, logs, nextmapCalls, values, server,
    get vote() { return vote; },
    get iface() { return published; },
    async start() { await api.OnPluginStart(); host.poll(); },
    poll() { for (let i = 0; i < 64; i++) api.OnGameFrame(); },
    async settle() { await new Promise(setImmediate); },
    async end(winner, counts) {
      const v = vote; vote = null;
      const total = counts.reduce((a, b) => a + b, 0);
      v.onEnd({ winner, counts, total });
    },
  };
  return host;
}

test('the vote opens mapvote_start minutes before mp_timelimit, not earlier', async () => {
  const host = loadPlugin();
  await host.start();
  host.cvars.set('mp_timelimit', '30');
  host.server.gameTime = 30 * 60 - 121;
  host.poll(); await host.settle();
  assert.equal(host.vote, null);
  host.server.gameTime = 30 * 60 - 119;
  host.poll(); await host.settle();
  assert.ok(host.vote, 'vote started at two minutes left');
  assert.equal(host.vote.duration, 20);
  assert.equal(host.vote.options.at(-1), 'Extend Map');
});

test('the start window never falls inside the vote duration', async () => {
  const host = loadPlugin({ settings: { mapvote_start: 0 } });
  await host.start();
  host.cvars.set('mp_timelimit', '10');
  host.server.gameTime = 600 - 31;
  host.poll(); await host.settle();
  assert.equal(host.vote, null);
  host.server.gameTime = 600 - 30;
  host.poll(); await host.settle();
  assert.ok(host.vote);
});

test('ballot: nominations first, then fill without the current map or cooldown, then Extend', async () => {
  const host = loadPlugin({ nominations: ['de_nuke', 'de_dust2'], history: ['de_mirage'], settings: { mapvote_include: 3 } });
  await host.start();
  host.cvars.set('mp_timelimit', '30');
  host.admins.get('sm_mapvote')({ replyT() {} });
  await host.settle();
  // de_dust2 is the current map (nominated anyway), de_mirage is on cooldown.
  assert.deepEqual([...host.vote.options], ['de_nuke', 'awp_lego_2', 'de_inferno', 'Extend Map']);
});

test('a nominated cooldown map is still listed', async () => {
  const host = loadPlugin({ nominations: ['de_mirage'], history: ['de_mirage'] });
  await host.start();
  host.admins.get('sm_mapvote')({ replyT() {} });
  await host.settle();
  assert.equal(host.vote.options[0], 'de_mirage');
});

test('no Extend when mapvote_extend is 0 or there is no limit to extend', async () => {
  const off = loadPlugin({ settings: { mapvote_extend: 0 } });
  await off.start();
  off.cvars.set('mp_timelimit', '30');
  off.admins.get('sm_mapvote')({ replyT() {} });
  await off.settle();
  assert.ok(!off.vote.options.includes('Extend Map'));

  const unlimited = loadPlugin();
  await unlimited.start();
  unlimited.admins.get('sm_mapvote')({ replyT() {} });
  await unlimited.settle();
  assert.ok(!unlimited.vote.options.includes('Extend Map'));
});

test('a map winner becomes nextmap\'s next map when nextmap is loaded', async () => {
  const host = loadPlugin({ nominations: ['de_nuke'], withNextmap: true });
  await host.start();
  host.cvars.set('mp_timelimit', '30');
  host.server.gameTime = 30 * 60 - 60;
  host.poll(); await host.settle();
  await host.end(0, [3, 1, 0, 0, 0]);
  assert.deepEqual(host.nextmapCalls, ['de_nuke']);
  assert.equal(host.cvars.get('nextlevel'), 'de_nuke');
  assert.equal(host.iface.hasEndOfMapVoteFinished(), true);
  host.hooks.get('cs_win_panel_match')();
  assert.deepEqual(host.commands, [], 'nextmap owns the change');
  host.poll(); await host.settle();
  assert.equal(host.vote, null, 'no second vote after a map won');
});

test('without nextmap a workshop winner changes at match end', async () => {
  const host = loadPlugin({ nominations: ['awp_lego_2'] });
  await host.start();
  host.admins.get('sm_mapvote')({ replyT() {} });
  await host.settle();
  await host.end(0, [2, 0, 0, 0, 0]);
  host.hooks.get('cs_win_panel_match')();
  assert.deepEqual(host.commands, ['host_workshop_map 3070284539']);
});

test('Extend adds time, allows exactly mapvote_extend extensions, and votes again', async () => {
  const host = loadPlugin();
  await host.start();
  host.cvars.set('mp_timelimit', '30');
  host.server.gameTime = 30 * 60 - 60;
  host.poll(); await host.settle();
  const extendAt = host.vote.options.indexOf('Extend Map');
  const counts = host.vote.options.map((_, i) => (i === extendAt ? 4 : 0));
  await host.end(extendAt, counts);
  assert.equal(host.cvars.get('mp_timelimit'), '45');
  assert.equal(host.iface.hasEndOfMapVoteFinished(), false);
  host.poll(); await host.settle();
  assert.equal(host.vote, null, 'not yet: 15 minutes were added');
  host.server.gameTime = 45 * 60 - 60;
  host.poll(); await host.settle();
  assert.ok(host.vote, 'the vote reopens against the new limit');
  assert.ok(!host.vote.options.includes('Extend Map'), 'the one allowed extension is used');
});

test('Extend with mp_maxrounds adds rounds', async () => {
  const host = loadPlugin();
  await host.start();
  host.cvars.set('mp_maxrounds', '10');
  for (let i = 0; i < 7; i++) host.hooks.get('round_end')();
  await host.settle();
  assert.equal(host.vote, null);
  host.hooks.get('round_end')();
  await host.settle();
  assert.ok(host.vote, 'vote at two rounds to go');
  const extendAt = host.vote.options.indexOf('Extend Map');
  await host.end(extendAt, host.vote.options.map((_, i) => (i === extendAt ? 1 : 0)));
  assert.equal(host.cvars.get('mp_maxrounds'), '15');
});

test('a tie picks one of the leaders; no votes leaves the rotation alone', async () => {
  const tie = loadPlugin({ nominations: ['de_nuke', 'de_inferno'], withNextmap: true });
  await tie.start();
  tie.admins.get('sm_mapvote')({ replyT() {} });
  await tie.settle();
  await tie.end(null, [2, 2, 0, 0, 0]);
  assert.deepEqual(tie.nextmapCalls, ['de_nuke']);

  const none = loadPlugin({ withNextmap: true });
  await none.start();
  none.admins.get('sm_mapvote')({ replyT() {} });
  await none.settle();
  await none.end(null, [0, 0, 0, 0, 0]);
  assert.deepEqual(none.nextmapCalls, []);
  assert.equal(none.iface.hasEndOfMapVoteFinished(), true);
});

test('a consumer returning Handled from OnMapVoteStart postpones the vote', async () => {
  const host = loadPlugin({ postpone: true });
  await host.start();
  host.cvars.set('mp_timelimit', '30');
  host.server.gameTime = 30 * 60 - 60;
  host.poll(); await host.settle();
  assert.equal(host.vote, null);
  assert.equal(host.iface.isVoteInProgress(), false);
});

test('a map change resets the vote', async () => {
  const host = loadPlugin({ withNextmap: true, nominations: ['de_nuke'] });
  await host.start();
  host.admins.get('sm_mapvote')({ replyT() {} });
  await host.settle();
  await host.end(0, [1, 0, 0, 0, 0]);
  assert.equal(host.iface.hasEndOfMapVoteFinished(), true);
  host.server.mapName = 'de_nuke';
  host.poll();
  assert.equal(host.iface.hasEndOfMapVoteFinished(), false);
});

test('a map change cancels a ballot still open from the old map', async () => {
  const host = loadPlugin();
  await host.start();
  host.admins.get('sm_mapvote')({ replyT() {} });
  await host.settle();
  assert.ok(host.vote);
  host.server.mapName = 'de_nuke';
  host.poll();
  assert.equal(host.vote, null);
  assert.equal(host.iface.isVoteInProgress(), false);
});
