import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

async function loadBoth(order, winner) {
  const hooks = new Map(), admins = new Map(), pending = [], sent = [], providers = new Map(), watchers = new Map();
  let vote;
  const cvars = new Map();
  const sdk = {
    Server: {
      mapName: 'de_inferno', gameTime: 0, isMapValid: () => true,
      getCvar: name => cvars.get(name) ?? '0', setCvar: (name, value) => cvars.set(name, value),
      command: line => sent.push(line),
    },
    config: {
      readFile: () => 'de_inferno\nde_nuke\nde_mirage',
      getInt: name => name === 'nextmap_change_delay' ? 5 : name === 'rtv_map_count' ? 1 : 0,
      getBool: () => true,
    },
    delay: () => new Promise(resolve => pending.push(resolve)),
    Database: { open: async () => ({ execute: async () => {}, query: async sql => sql.includes('FROM nominations') ? [{ map: winner }] : [] }) },
    Clients: { all: () => [] }, UserMessages: { onPre() {} },
    Vote: { isActive: () => false, registerTallyRenderer() {}, start: options => { vote = options; } },
    hook: { on: (event, handler) => { const list = hooks.get(event) ?? []; list.push(handler); hooks.set(event, list); } },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    translations: { load() {} }, Translations: { translate: (_slot, key) => key }, Chat: { toAll() {} },
    ADMFLAG: { CHANGEMAP: 8 }, HookResult: { Continue: 0, Handled: 2 },
    // Only the external inter-plugin transport is replaced; both map policies run unchanged.
    publish(name) {
      const handlers = new Map();
      const provider = { on: (event, handler) => {
        const list = handlers.get(event) ?? []; list.push(handler); handlers.set(event, list);
        return { dispose() { list.splice(list.indexOf(handler), 1); } };
      } };
      providers.set(name, provider);
      for (const attach of watchers.get(name) ?? []) attach(provider, { own: value => value });
      return { dispatch: (event, payload) => Math.max(0, ...(handlers.get(event) ?? []).map(handler => handler(structuredClone(payload)))) };
    },
    watchOptional(name, attach) {
      const list = watchers.get(name) ?? []; list.push(attach); watchers.set(name, list);
      if (providers.has(name)) attach(providers.get(name), { own: value => value });
    },
  };
  for (const name of order) {
    const source = readFileSync(new URL(`../../${name}/src/plugin.ts`, import.meta.url), 'utf8');
    const math = Object.create(Math); math.random = () => 0;
    const ctx = vm.createContext({ module: { exports: {} }, console: { log() {} }, Math: math,
      require: name => name === '@s2script/sdk' ? sdk : { Player: {} } });
    vm.runInContext(transformSync(source, { loader: 'ts', format: 'cjs' }).code, ctx);
    const api = ctx.module.exports;
    await api.OnPluginStart();
    for (let i = 0; i < 64; i++) api.OnGameFrame();
  }
  const fire = event => { for (const handler of hooks.get(event) ?? []) handler(); };
  cvars.set('mp_maxrounds', '1');
  fire('round_end');
  for (const resolve of pending.splice(0)) resolve();
  await new Promise(setImmediate);
  if (winner !== null) {
    admins.get('sm_forcertv')({ replyT() {} });
    await new Promise(setImmediate);
    vote.onEnd({ winner: 0, counts: [1, 0], total: 1 });
  }
  return { fire, sent };
}

for (const order of [['nextmap', 'rockthevote'], ['rockthevote', 'nextmap']]) {
  test(`RTV without a winner allows normal rotation: ${order.join(' then ')}`, async () => {
    const host = await loadBoth(order, null);
    host.fire('round_prestart');
    assert.deepEqual(host.sent, ['changelevel de_nuke']);
  });
  for (const event of ['round_prestart', 'cs_win_panel_match']) {
    for (const winner of ['de_mirage', 'de_nuke']) {
      test(`RTV wins once over ready rotation: ${order.join(' then ')}, ${event}, ${winner}`, async () => {
        const host = await loadBoth(order, winner);
        host.fire(event);
        assert.deepEqual(host.sent, ['changelevel ' + winner]);
        host.fire(event);
        assert.deepEqual(host.sent, ['changelevel ' + winner]);
      });
    }
  }
}
