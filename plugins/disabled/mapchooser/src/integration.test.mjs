// mapchooser running next to the real nextmap and rockthevote sources. Only the inter-plugin
// transport and the engine are faked; every map policy runs unchanged.
import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const schema = name => JSON.parse(readFileSync(new URL(`../../${name}/package.json`, import.meta.url), 'utf8')).s2script.config;

async function loadAll(names, { nominations = [] } = {}) {
  const hooks = new Map(), admins = new Map(), sent = [], chat = [], providers = new Map(), watchers = new Map();
  const cvars = new Map([['mp_timelimit', '30']]);
  const values = new Map();
  for (const name of names) for (const [k, v] of Object.entries(schema(name))) values.set(k, v.default);
  const server = {
    mapName: 'de_inferno', gameTime: 0, isMapValid: () => true,
    getCvar: name => cvars.get(name) ?? '0', setCvar: (name, value) => cvars.set(name, String(value)),
    command: line => sent.push(line),
  };
  const pending = [];
  const votes = {};
  const base = {
    Server: server,
    config: {
      readFile: () => 'de_inferno\nde_nuke\nde_mirage\nde_ancient',
      writeFile() {},
      getInt: k => values.get(k), getBool: k => values.get(k), getFloat: k => values.get(k),
    },
    delay: () => new Promise(resolve => pending.push(resolve)),
    Database: { open: async () => ({ execute: async () => {}, query: async sql => sql.includes('FROM nominations') ? nominations.map(map => ({ map })) : [] }) },
    Clients: { all: () => [{ slot: 1, isBot: false, isValid: () => true }], fromSlot: slot => ({ slot, isBot: false, isValid: () => true }) },
    UserMessages: { onPre() {} },
    hook: { on: (event, handler) => { const list = hooks.get(event) ?? []; list.push(handler); hooks.set(event, list); } },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    translations: { load() {} }, Translations: { translate: (_slot, key) => key },
    Chat: { toAll: text => chat.push(text), toSlot: (_slot, text) => chat.push(text) },
    ADMFLAG: { CHANGEMAP: 8 }, HookResult: { Continue: 0, Handled: 2 },
    publish(name, impl) {
      const handlers = new Map();
      const provider = {
        ...impl,
        on: (event, handler) => {
          const list = handlers.get(event) ?? []; list.push(handler); handlers.set(event, list);
          return { dispose() { list.splice(list.indexOf(handler), 1); } };
        },
      };
      providers.set(name, provider);
      for (const attach of watchers.get(name) ?? []) attach(provider, { own: value => value });
      return { dispatch: (event, payload) => Math.max(0, ...(handlers.get(event) ?? []).map(h => h(structuredClone(payload)))) };
    },
    watchOptional(name, attach) {
      const list = watchers.get(name) ?? []; list.push(attach); watchers.set(name, list);
      if (providers.has(name)) attach(providers.get(name), { own: value => value });
    },
  };
  const apis = {};
  for (const name of names) {
    // Each plugin context has its own vote lock, as in the host.
    const vote = { current: null };
    votes[name] = vote;
    const sdk = { ...base, Vote: { isActive: () => vote.current !== null, registerTallyRenderer() {}, cancel: () => { vote.current = null; }, start: o => { if (vote.current) return false; vote.current = o; return true; } } };
    const source = readFileSync(new URL(`../../${name}/src/plugin.ts`, import.meta.url), 'utf8');
    const math = Object.create(Math); math.random = () => 0;
    const ctx = vm.createContext({ module: { exports: {} }, console: { log() {} }, Math: math,
      require: n => n === '@s2script/sdk' ? sdk : { Player: { fromSlot: () => null } } });
    vm.runInContext(transformSync(source, { loader: 'ts', format: 'cjs' }).code, ctx);
    apis[name] = ctx.module.exports;
    await apis[name].OnPluginStart();
  }
  const poll = () => { for (let i = 0; i < 64; i++) for (const api of Object.values(apis)) api.OnGameFrame(); };
  poll();
  const settle = () => new Promise(setImmediate);
  const end = (name, index) => {
    const v = votes[name].current; votes[name].current = null;
    v.onEnd({ winner: index, counts: v.options.map((_, i) => (i === index ? 1 : 0)), total: 1 });
  };
  const fire = event => { for (const h of hooks.get(event) ?? []) h(); };
  const elapse = async () => { for (const r of pending.splice(0)) r(); await settle(); };
  return { apis, votes, server, cvars, sent, chat, admins, poll, settle, end, fire, elapse };
}

for (const order of [['nextmap', 'mapchooser'], ['mapchooser', 'nextmap']]) {
  test(`the vote winner, not the rotation, is played at the time limit: ${order.join(' then ')}`, async () => {
    const host = await loadAll(order, { nominations: ['de_ancient'] });
    host.server.gameTime = 30 * 60 - 100;
    host.poll(); await host.settle();
    const vote = host.votes.mapchooser.current;
    assert.equal(vote.options[0], 'de_ancient');
    host.end('mapchooser', 0);
    host.server.gameTime = 30 * 60;
    host.poll();
    await host.elapse();
    host.fire('round_prestart');
    assert.deepEqual(host.sent, ['changelevel de_ancient']);
  });

  test(`Extend keeps the map past the old limit: ${order.join(' then ')}`, async () => {
    const host = await loadAll(order);
    host.server.gameTime = 30 * 60 - 100;
    host.poll(); await host.settle();
    host.end('mapchooser', host.votes.mapchooser.current.options.indexOf('Extend Map'));
    host.server.gameTime = 30 * 60 + 1;
    host.poll(); await host.elapse();
    host.fire('round_prestart');
    assert.deepEqual(host.sent, []);
    assert.equal(host.cvars.get('mp_timelimit'), '45');
  });
}

test('RTV is refused once the end-of-map vote has chosen, and while it runs', async () => {
  const host = await loadAll(['nextmap', 'mapchooser', 'rockthevote']);
  host.server.gameTime = 30 * 60 - 100;
  host.poll(); await host.settle();
  assert.ok(host.votes.mapchooser.current);
  host.apis.rockthevote.OnClientSayCommand(1, 'rtv', false);
  assert.equal(host.chat.at(-1), 'Rtv Vote Already Running');
  let reply;
  host.admins.get('sm_forcertv')({ replyT: k => { reply = k; } });
  assert.equal(reply, 'Rtv Forced Already Running', 'a forced RTV cannot open a second ballot');
  host.end('mapchooser', 0);
  host.apis.rockthevote.OnClientSayCommand(1, 'rtv', false);
  assert.equal(host.chat.at(-1), 'Rtv Map Vote Finished');
  host.admins.get('sm_forcertv')({ replyT: k => { reply = k; } });
  await host.settle();
  assert.equal(reply, 'Rtv Forced Already Running', 'a forced RTV cannot veto the chosen map');
  assert.equal(host.votes.rockthevote.current, null);
});

test('the end-of-map vote waits while an RTV vote runs', async () => {
  const host = await loadAll(['mapchooser', 'rockthevote']);
  let reply;
  host.admins.get('sm_forcertv')({ replyT: k => { reply = k; } });
  await host.settle();
  assert.equal(reply, 'Rtv Forced');
  assert.ok(host.votes.rockthevote.current);
  host.server.gameTime = 30 * 60 - 100;
  host.poll(); await host.settle();
  assert.equal(host.votes.mapchooser.current, null, 'postponed while RTV votes');
  host.end('rockthevote', host.votes.rockthevote.current.options.length - 1); // Don't Change
  host.poll(); await host.settle();
  assert.ok(host.votes.mapchooser.current, 'opens once RTV is done');
});

for (const order of [['nextmap', 'mapchooser'], ['mapchooser', 'nextmap']]) {
  test(`a vote still open at the final round holds the rotation, then its winner is played: ${order.join(' then ')}`, async () => {
    const host = await loadAll(order, { nominations: ['de_ancient'] });
    host.cvars.set('mp_timelimit', '0');
    host.cvars.set('mp_maxrounds', '3');
    host.fire('round_end');
    host.fire('round_end');   // two to go -> vote opens
    await host.settle();
    assert.ok(host.votes.mapchooser.current);
    host.fire('round_end');   // the limit, while the vote is still open
    await host.elapse();
    host.fire('round_prestart');
    assert.deepEqual(host.sent, [], 'rotation held while the vote is open');
    host.end('mapchooser', 0);
    host.poll();              // nextmap retries the deferred change
    await host.elapse();
    host.fire('cs_win_panel_match');
    assert.deepEqual(host.sent, ['changelevel de_ancient']);
  });
}
