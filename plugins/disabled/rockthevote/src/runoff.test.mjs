import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;

async function loadRtv({ runoff = true, pollFirst = true } = {}) {
  const hooks = new Map(), admins = new Map(), commands = [], chat = [], votes = [];
  let active = null;
  const lock = { refuse: false };   // simulates another vote taking the slot mid-build
  const server = { mapName: 'de_inferno', command: line => commands.push(line) };
  const sdk = {
    Clients: { all: () => [] }, Server: server,
    UserMessages: { onPre() {} },
    Database: { open: async () => ({ execute: async () => {}, query: async () => [] }) },
    config: {
      readFile: () => 'de_nuke\nde_mirage\nde_ancient',
      // Round-end mode, so a winner stays queued and each test can inspect it before it applies.
      getInt: name => name === 'rtv_map_count' ? 3 : name === 'rtv_vote_duration' ? 20 : name === 'rtv_changetime' ? 1 : 0,
      getBool: name => name === 'rtv_runoff' ? runoff : false,
    },
    command: { admin: (name, _flags, handler) => admins.set(name, handler) },
    hook: { on: (name, handler) => hooks.set(name, handler) },
    watchOptional() {},
    translations: { load() {} }, Translations: { translate: (_slot, key, ...args) => [key, ...args].join('|') },
    Chat: { toAll: text => chat.push(text) }, HookResult: { Handled: 2, Continue: 0 }, ADMFLAG: { CHANGEMAP: 8 },
    Activity: { showT: (cmd, key) => cmd.replyT(key), show() {}, log() {}, describe: () => '' },
    Vote: {
      isActive: () => active !== null, registerTallyRenderer() {},
      start: options => { if (active || lock.refuse) return false; active = options; votes.push(options); return true; },
      cancel: () => { active = null; },
    },
  };
  const math = Object.create(Math); math.random = () => 0;
  const ctx = vm.createContext({ module: { exports: {} }, console: { log() {} }, Math: math,
    require: name => name === '@s2script/sdk' ? sdk : { Player: {} } });
  vm.runInContext(code, ctx);
  const api = ctx.module.exports;
  await api.OnPluginStart();
  if (pollFirst) for (let i = 0; i < 64; i++) api.OnGameFrame();
  admins.get('sm_forcertv')({ replyT() {} });
  await new Promise(setImmediate);
  // End the open vote the way the core does: release the lock, then call onEnd.
  const end = counts => {
    const v = active; active = null;
    const total = counts.reduce((a, b) => a + b, 0);
    const best = Math.max(...counts);
    const leaders = counts.filter(c => c === best).length;
    v.onEnd({ winner: total === 0 || leaders > 1 ? null : counts.indexOf(best), counts, total });
  };
  const poll = () => { for (let i = 0; i < 64; i++) api.OnGameFrame(); };
  return { hooks, commands, chat, votes, end, server, poll, lock, admins, get active() { return active; } };
}

test('a tie runs a runoff of only the tied options, repeating until one wins', async () => {
  const host = await loadRtv();
  const [a, b, c] = host.votes[0].options;
  assert.equal(host.votes[0].options.length, 4);
  assert.equal(host.votes[0].options[3], "Don't Change");
  host.end([2, 2, 2, 1]);
  assert.deepEqual([...host.active.options], [a, b, c], 'three-way tie');
  assert.equal(host.active.question, 'RockTheVote Runoff');
  assert.ok(host.chat.includes('Rtv Runoff|1'));
  host.end([3, 3, 1]);
  assert.deepEqual([...host.active.options], [a, b], 'narrowed to the two still tied');
  assert.ok(host.chat.includes('Rtv Runoff|2'));
  host.end([1, 4]);
  assert.equal(host.active, null);
  assert.ok(host.chat.includes('Rtv Winner Round End|' + b));
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, ['changelevel ' + b]);
});

test('Don\'t Change can tie into a runoff and win it', async () => {
  const host = await loadRtv();
  host.end([0, 3, 0, 3]);
  assert.deepEqual([...host.active.options], [host.votes[0].options[1], "Don't Change"]);
  host.end([1, 2]);
  assert.ok(host.chat.includes('Rtv Dont Change Won'));
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, []);
});

test('a runoff nobody votes in keeps the map', async () => {
  const host = await loadRtv();
  host.end([1, 1, 0, 0]);
  host.end([0, 0]);
  assert.equal(host.active, null);
  assert.ok(host.chat.includes('Rtv No Votes'));
  assert.equal(host.votes.length, 2);
});

test('rtv_runoff false keeps the old tie behaviour', async () => {
  const host = await loadRtv({ runoff: false });
  host.end([1, 1, 0, 0]);
  assert.equal(host.active, null);
  assert.ok(host.chat.includes('Rtv Vote Tied'));
});

test('no runoff starts on a map that already changed', async () => {
  const host = await loadRtv();
  host.server.mapName = 'de_mirage';
  host.end([1, 1, 0, 0]);
  assert.equal(host.active, null);
});

test('a winner from a ballot that outlived a map change is not applied', async () => {
  const host = await loadRtv();
  host.server.mapName = 'de_mirage';
  host.end([0, 3, 0, 0]);
  host.hooks.get('round_prestart')();
  assert.deepEqual(host.commands, []);
  assert.ok(!host.chat.some(line => line.startsWith('Rtv Winner')));
});

test('the map-change reset cancels a ballot still open', async () => {
  const host = await loadRtv();
  assert.ok(host.active);
  host.server.mapName = 'de_mirage';
  host.poll();
  assert.equal(host.active, null);
});

test('the first map poll after load does not cancel a forced RTV', async () => {
  const host = await loadRtv({ pollFirst: false });
  assert.ok(host.active);
  host.poll();
  assert.ok(host.active, 'still open');
});

test('an RTV forced on the new map before the poll notices the change is kept', async () => {
  const host = await loadRtv();
  host.end([0, 0, 0, 3]);   // Don't Change: the first ballot is over
  host.server.mapName = 'de_mirage';
  host.admins.get('sm_forcertv')({ replyT() {} });
  await new Promise(setImmediate);
  assert.ok(host.active);
  host.poll();   // the reset for de_mirage
  assert.ok(host.active, 'the new map\'s ballot is not cancelled');
});

test('a ballot the vote slot refuses releases the RTV lock', async () => {
  const host = await loadRtv();
  host.end([0, 0, 0, 3]);
  host.server.mapName = 'de_mirage';
  host.poll();
  host.lock.refuse = true;
  let reply;
  host.admins.get('sm_forcertv')({ replyT: k => { reply = k; } });
  await new Promise(setImmediate);
  assert.equal(host.active, null);
  host.lock.refuse = false;
  host.admins.get('sm_forcertv')({ replyT: k => { reply = k; } });
  await new Promise(setImmediate);
  assert.equal(reply, 'Initiated Vote Map');
  assert.ok(host.active, 'a later RTV can start');
});

test('an RTV requested on the old map does not open after a map change', async () => {
  const host = await loadRtv();
  host.end([0, 0, 0, 3]);
  host.admins.get('sm_forcertv')({ replyT() {} });   // ballot building on de_inferno...
  host.server.mapName = 'de_mirage';                 // ...when the map changes
  await new Promise(setImmediate);
  assert.equal(host.active, null, 'the old map\'s request is dropped');
  host.poll();
  let reply;
  host.admins.get('sm_forcertv')({ replyT: k => { reply = k; } });
  await new Promise(setImmediate);
  assert.equal(reply, 'Initiated Vote Map');
  assert.ok(host.active, 'the new map can still vote');
});
