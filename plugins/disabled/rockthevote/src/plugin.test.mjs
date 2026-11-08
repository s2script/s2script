import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';
import { installClientHost } from '../../../../packages/sdk/test/client-host.mjs';

test('deferred A disconnect preserves B RTV request and A request never counts as B', () => {
  const ctx = { module: { exports: {} }, console };
  const host = installClientHost(ctx, [3, 4]);
  const messages = [];
  const sdk = {
    Activity: { show() {}, showT() {}, log() {}, describe: () => '', targetName: () => '' },
    Clients: ctx.__s2pkg_clients.Clients, HookResult: { Handled: 2, Continue: 0 },
    UserMessages: { onPre() {} },
    config: { getInt: () => 0, getFloat: () => 1 },
    Chat: { toSlot: (_slot, text) => messages.push(text), toAll: text => messages.push(text) },
    Translations: { translate: (_slot, key) => key },
  };
  ctx.require = name => name === '@s2script/sdk' ? sdk : { Player: { fromSlot: () => ({ playerName: 'current' }) } };
  vm.createContext(ctx);
  const source = readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8');
  vm.runInContext(transformSync(source, { loader: 'ts', format: 'cjs' }).code, ctx);
  const api = ctx.module.exports;
  const a = sdk.Clients.fromSlot(3);
  api.OnClientSayCommand(3, 'rtv', false);
  host.replace(3);
  api.OnClientSayCommand(3, 'rtv', false);
  assert.equal(messages.at(-1), 'Rtv Wants To Vote', 'B has not voted yet');
  api.OnClientDisconnect(a);
  api.OnClientSayCommand(3, 'rtv', false);
  assert.equal(messages.at(-1), 'Rtv Already Voted', 'B request survives A disconnect');
});

async function loadVotingPlugin(maplist = 'de_nuke') {
  const hooks = new Map(), admins = new Map(), commands = [];
  let vote;
  const server = { mapName: 'de_inferno', command: line => commands.push(line) };
  const db = { execute: async () => {}, query: async () => [] };
  const sdk = {
    Activity: { show() {}, showT() {}, log() {}, describe: () => '', targetName: () => '' },
    Clients: { all: () => [] }, Server: server,
    UserMessages: { onPre() {} }, Database: { open: async () => db },
    config: { readFile: () => maplist, getInt: name => name === 'rtv_map_count' ? 1 : 0, getBool: () => false },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    hook: { on: (name, handler) => hooks.set(name, handler) },
    watchOptional() {},
    translations: { load() {} }, Translations: { translate: (_slot, key) => key },
    Chat: { toAll() {} }, HookResult: { Handled: 2 }, ADMFLAG: { CHANGEMAP: 8 },
    Vote: { isActive: () => false, registerTallyRenderer() {}, start: options => { vote = options; } },
  };
  const ctx = vm.createContext({ module: { exports: {} }, console: { log() {} }, require: name => name === '@s2script/sdk' ? sdk : { Player: {} } });
  const source = readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8');
  vm.runInContext(transformSync(source, { loader: 'ts', format: 'cjs' }).code, ctx);
  const api = ctx.module.exports;
  await api.OnPluginStart();
  const poll = () => { for (let i = 0; i < 64; i++) api.OnGameFrame(); };
  poll();
  admins.get('sm_forcertv')({ replyT() {} });
  await new Promise(setImmediate);
  assert.ok(vote, 'the real ballot path starts a vote');
  vote.onEnd({ winner: 0, counts: [1, 0], total: 1 });
  return { hooks, commands, server, poll };
}

for (const [maplist, command] of [['de_nuke', 'changelevel de_nuke'], ['awp_lego_2:3070284539', 'host_workshop_map 3070284539']]) {
  test(`RTV preserves round-end results and applies ${command} at the next prestart once`, async () => {
    const host = await loadVotingPlugin(maplist);
    assert.deepEqual(host.commands, [], 'vote completion must not change maps');
    host.hooks.get('round_end')?.();
    assert.deepEqual(host.commands, [], 'round end must not immediately change maps');
    host.hooks.get('round_prestart')();
    assert.deepEqual(host.commands, [command]);
    host.hooks.get('round_prestart')();
    assert.deepEqual(host.commands, [command]);
  });
}

test('RTV discards a queued winner if another map became live before the map poll', async () => {
  const host = await loadVotingPlugin();
  host.server.mapName = 'de_mirage';
  host.hooks.get('round_end')?.();
  host.hooks.get('round_prestart')?.();
  assert.deepEqual(host.commands, []);
});

test('RTV can apply after match end when no following round will start', async () => {
  const host = await loadVotingPlugin();
  assert.equal(typeof host.hooks.get('cs_win_panel_match'), 'function');
  host.hooks.get('cs_win_panel_match')();
  assert.deepEqual(host.commands, ['changelevel de_nuke']);
});
