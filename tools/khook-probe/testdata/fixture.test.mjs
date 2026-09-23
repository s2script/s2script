import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
import ts from 'typescript';

const fixturePath = new URL('../../../examples/khook-acceptance/src/plugin.ts', import.meta.url);
const source = fs.readFileSync(fixturePath, 'utf8');
const compiled = ts.transpileModule(source, {
  reportDiagnostics: true,
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
});
const ARTIFACT = 'a'.repeat(64);
const REVISION = 'b'.repeat(40);

// Only the unavailable game boundary is mocked. The real plugin registers its
// callbacks, owns transitions/entities and emits all of the asserted records.
function host({ humans = 3, cvars = new Map(), previous,
  fixtureIdentity = { revision: REVISION, token: 'c'.repeat(64) },
  engine = { hooks: new Map(), entities: [], visibility: new Map(), nextIndex: 10 } } = {}) {
  const handlers = new Map(), listeners = new Map(), ownedHooks = [];
  const { hooks, entities, visibility } = engine;
  cvars.set('s2_khook_source_revision', REVISION);
  const clients = Array.from({ length: humans }, (_, slot) => ({
    slot, userId: slot + 1, steamId: String(76561198000000000n + BigInt(slot)),
    isBot: false, isValid: () => true,
  }));
  const sdk = {
    // B/C controlled engine descriptors are unavailable in this A-only fake host.
    // Null is a named pending boundary, never fabricated native callback evidence.
    Engine: { call: () => null, hook: () => null },
    Server: {
      mapName: 'de_test', getCvar: n => cvars.get(n) ?? '',
      setCvar: (n, v) => { if (!cvars.has(n)) return false; cvars.set(n, v); return true; },
      registerCvar: (n, opts) => { if (!cvars.has(n)) cvars.set(n, String(opts.default)); return true; },
    },
    Entity: { findByClass: cls => entities.filter(e => e.valid && e.classname === cls) },
    SDKHookType: { Touch: 1, TouchPost: 2, SetTransmit: 3 },
    SDKHook: (e, type, fn) => { const key = `${e.id}:${type}`; const a = hooks.get(key) ?? []; a.push(fn); hooks.set(key, a); ownedHooks.push([key, fn]); return true; },
    SDKUnhook: (e, type, fn) => { const key = `${e.id}:${type}`; hooks.set(key, (hooks.get(key) ?? []).filter(f => f !== fn)); },
    Voice: { resetAll() {}, setAudibleTo: () => true },
    Transmit: { resetAll() { visibility.clear(); }, reset(e) { visibility.delete(e.id); }, setVisibleTo(e, slots) { visibility.set(e.id, [...slots]); return true; } },
    HookResult: { Continue: 0, Handled: 2 },
    Clients: { all: () => clients }, Player: { allConnected: () => [] },
    createEntity: (classname, values = {}) => {
      const e = { index: engine.nextIndex++, id: engine.nextIndex + 1000, name: values.targetname, classname, valid: true,
        isValid() { return this.valid; }, remove() { this.valid = false; }, spawn() { return true; }, teleport() {} };
      entities.push(e); return e;
    },
    previous: () => previous,
    Events: { setRecipients() {} }, hook: { onPre() {} },
    command: { onClientCommand: (n, f) => listeners.set(n, f), server: (n, f) => handlers.set(n, f) },
  };
  const ctx = { exports: {}, require: name => name === './build_identity'
    ? { KHOOK_FIXTURE_REVISION: fixtureIdentity.revision, KHOOK_FIXTURE_TOKEN: fixtureIdentity.token }
    : sdk, console: { log() {} } };
  vm.createContext(ctx); vm.runInContext(compiled.outputText, ctx);
  ctx.exports.OnPluginStart();
  const control = (verb, run = 'run-1', digest = ARTIFACT, suite = '') => {
    const rows = [];
    handlers.get('s2_khook_accept')({ arg: i => [verb, run, digest, suite][i] ?? '', reply: s => {
      if (s.startsWith('{')) rows.push(JSON.parse(s));
    } });
    return rows;
  };
  const frame = () => ctx.exports.OnGameFrame();
  const invoke = ({ copies = 1, omitPost = false, original = 1, ackRun = 'run-1', stage } = {}) => {
    const index = Number(cvars.get('s2_khook_accept_phase_ent'));
    const e = entities.find(e => e.valid && e.index === index);
    const current = Number(cvars.get('s2_khook_accept_phase_stage'));
    for (let i = 0; i < copies; i++) for (const f of [...(hooks.get(`${e.id}:1`) ?? [])]) f(e, null);
    if (!omitPost) for (let i = 0; i < copies; i++) for (const f of [...(hooks.get(`${e.id}:2`) ?? [])]) f(e, null);
    cvars.set('s2_khook_accept_phase_ack', JSON.stringify({ run_id: ackRun, entity_index: index, stage: stage ?? current, original }));
    frame();
  };
  const reload = ({ keepStale = false } = {}) => {
    const state = ctx.exports.OnPluginState(); ctx.exports.OnPluginEnd();
    if (!keepStale) for (const [key, fn] of ownedHooks) hooks.set(key, (hooks.get(key) ?? []).filter(f => f !== fn));
    return host({ humans, cvars, previous: state, fixtureIdentity, engine });
  };
  const nativeTarget = () => sdk.createEntity('trigger_push', { targetname: 'native-reload-target' });
  const touchReload = (target, { copies = 1, omitPost = false, ack = true, original = 1 } = {}) => {
    cvars.set('s2_khook_accept_reload_trace', ''); cvars.set('s2_khook_accept_reload_active', 'run-1');
    for (let i = 0; i < copies; i++) for (const f of [...(hooks.get(`${target.id}:1`) ?? [])]) f(target, null);
    if (!omitPost) for (let i = 0; i < copies; i++) for (const f of [...(hooks.get(`${target.id}:2`) ?? [])]) f(target, null);
    cvars.set('s2_khook_accept_reload_active', '');
    if (ack) cvars.set('s2_khook_accept_reload_ack', JSON.stringify({ run_id: 'run-1', artifact_identity: ARTIFACT,
      target_index: target.index, generation: Number(cvars.get('s2_khook_accept_live')), stage: previous ? 'after' : 'before', original }));
  };
  const runtime = () => {
    const replies = [];
    handlers.get('s2_khook_accept')({
      arg: i => ['runtime', '', ''][i] ?? '',
      reply: value => { if (value.startsWith('{')) replies.push(JSON.parse(value)); },
    });
    assert.equal(replies.length, 1);
    return replies[0];
  };
  return { control, frame, invoke, cvars, entities, ctx, clients, listeners, visibility, reload, nativeTarget, touchReload, runtime, sdk };
}
const row = (h, sub) => h.control('collect').find(r => r.subcheck === sub);

test('fixture parses without an in-memory source repair', () => {
  assert.deepEqual(compiled.diagnostics?.map(d => d.code), []);
});

test('runtime identity command reports the embedded build witness', () => {
  const h = host();
  const before = [...h.cvars.entries()];
  assert.deepEqual(h.runtime(), {
    schema: 1, kind: 'khook-fixture-runtime', result: 'ready',
    fixture_revision: REVISION, fixture_token: 'c'.repeat(64), generation: 1,
  });
  assert.deepEqual([...h.cvars.entries()], before);
});

test('ordinary unstamped fixture reports pending identity', () => {
  const h = host({ fixtureIdentity: { revision: 'unknown', token: 'unknown' } });
  assert.deepEqual(h.runtime(), {
    schema: 1, kind: 'khook-fixture-runtime', result: 'pending',
    fixture_revision: 'unknown', fixture_token: 'unknown', generation: 1,
  });
  h.control('prepare');
  assert.equal(row(h, 'js_no_handled_on_unsuppressed_event').result, 'pending');
});

test('final removal requires the final native invocation acknowledgement', () => {
  const h = host(); h.control('prepare');
  for (let i = 0; i < 5; i++) h.invoke();
  h.frame(); h.frame();
  assert.equal(row(h, 'js_phase_final_unsubscribe').result, 'pending');
  h.invoke();
  assert.equal(row(h, 'js_phase_final_unsubscribe').result, 'pass');
});

test('duplicate callbacks are emitted as observed and fail exact phase counts', () => {
  const h = host(); h.control('prepare'); h.invoke({ copies: 2 });
  const r = row(h, 'js_phase_subscribe_pre_post');
  assert.equal(r.result, 'fail'); assert.deepEqual(r.actual, { pre: 2, post: 2 });
});

test('self-unsubscribe retains the first POST assertion', () => {
  const h = host(); h.control('prepare');
  for (let i = 0; i < 3; i++) h.invoke();
  h.invoke({ omitPost: true }); h.invoke();
  const r = row(h, 'js_phase_self_unsubscribe');
  assert.equal(r.result, 'fail'); assert.equal(r.actual.first_post, 0);
});

test('stale run and phase acknowledgements cannot advance the fixture', () => {
  const h = host(); h.control('prepare'); h.invoke({ ackRun: 'older-run' });
  assert.equal(row(h, 'js_phase_subscribe_pre_post').result, 'pending');
  h.cvars.set('s2_khook_accept_phase_ack', JSON.stringify({ run_id: 'run-1', stage: 5, entity_index: Number(h.cvars.get('s2_khook_accept_phase_ent')), original: 1 }));
  h.frame(); assert.equal(row(h, 'js_phase_subscribe_pre_post').result, 'pending');
});

test('restore keeps the visibility subject alive until explicit teardown', () => {
  const h = host(); h.control('prepare');
  const tx = h.entities.find(e => e.classname === 'point_worldtext');
  assert.deepEqual(h.visibility.get(tx.id), [0]);
  h.control('restore'); assert.equal(tx.valid, true); assert.equal(h.visibility.has(tx.id), false);
  h.control('teardown'); assert.equal(tx.valid, false);
});

test('wrong-run teardown does not delete the active visibility subject', () => {
  const h = host(); h.control('prepare');
  h.control('teardown', 'wrong-run');
  assert.equal(h.entities.find(e => e.classname === 'point_worldtext').valid, true);
});

test('run binding and source identity are frozen, later binding cannot overwrite', () => {
  const h = host(); h.control('prepare', 'run-1', ''); h.control('bind');
  h.cvars.set('s2_khook_source_revision', 'c'.repeat(40));
  const r = h.control('collect')[0];
  assert.equal(r.artifact_identity, ARTIFACT); assert.equal(r.source_revision, REVISION);
  h.control('bind', 'run-1', 'd'.repeat(64));
  assert.equal(h.control('collect')[0].artifact_identity, ARTIFACT);
});

test('resume requires actual script handoff', () => {
  const first = host(); first.control('prepare'); first.ctx.exports.OnPluginEnd();
  const second = host({ cvars: first.cvars }); second.control('resume');
  assert.equal(second.entities.length, 0);
});

function armedReload() {
  const h = host(); h.control('prepare');
  const target = h.nativeTarget(); h.cvars.set('s2_khook_accept_reload_target', String(target.index));
  h.control('reload-arm'); h.touchReload(target);
  return { h, target };
}

test('script replacement uses the same resident target and preserves completed phase evidence', () => {
  const { h, target } = armedReload(); h.invoke();
  const before = row(h, 'js_phase_subscribe_pre_post');
  assert.equal(before.result, 'pass');
  const marker = Number(h.cvars.get('s2_khook_accept_reload_marker'));
  const next = h.reload(); next.control('resume');
  assert.equal(next.entities.find(e => e.index === marker).valid, false);
  next.touchReload(target);
  assert.equal(next.cvars.get('s2_khook_accept_reload_trace'), '2:pre,2:post,');
  assert.equal(row(next, 'js_fresh_subscription_after_reload').result, 'pass');
  assert.deepEqual(row(next, 'js_phase_subscribe_pre_post'), before);
});

test('old callbacks remain visible when the ledger fails to retire their owner', () => {
  const { h, target } = armedReload(); const next = h.reload({ keepStale: true });
  next.control('resume'); next.touchReload(target);
  assert.equal(next.cvars.get('s2_khook_accept_reload_trace'), '1:pre,2:pre,1:post,2:post,');
});

test('a failed explicit marker removal remains visible after otherwise successful reload', () => {
  const { h, target } = armedReload();
  const marker = h.entities.find(e => e.index === Number(h.cvars.get('s2_khook_accept_reload_marker')));
  // createEntity entities are world-owned. Suppress the engine remove boundary
  // called by actual OnPluginEnd cleanup; there is no entity-ledger fallback.
  marker.remove = () => {};
  const next = h.reload(); next.control('resume'); next.touchReload(target);
  assert.equal(marker.valid, true);
  assert.equal(next.cvars.get('s2_khook_accept_reload_trace'), '2:pre,2:post,');
  // Native collector coverage rejects this surviving index/serial even when
  // replacement callbacks and original counts are all correct.
});

for (const [name, options] of [['duplicates', { copies: 2 }], ['missing POST', { omitPost: true }], ['suppressed original', { original: 0 }]]) {
  test(`replacement ${name} cannot pass`, () => {
    const { h, target } = armedReload(); const next = h.reload(); next.control('resume'); next.touchReload(target, options);
    assert.equal(row(next, 'js_fresh_subscription_after_reload').result, 'fail');
  });
}

test('replacement without native invocation remains pending', () => {
  const { h } = armedReload(); const next = h.reload(); next.control('resume');
  assert.equal(row(next, 'js_fresh_subscription_after_reload').result, 'pending');
});

test('partially armed reload subscriptions are cleaned up before retry', () => {
  const h = host(); h.control('prepare'); const target = h.nativeTarget();
  h.cvars.set('s2_khook_accept_reload_target', String(target.index));
  const hook = h.sdk.SDKHook;
  h.sdk.SDKHook = (entity, type, fn) => type === 2 ? false : hook(entity, type, fn);
  h.control('reload-arm');
  assert.notEqual(h.cvars.get('s2_khook_accept_reload_ready'), '1');
  h.sdk.SDKHook = hook; h.control('reload-arm'); h.touchReload(target);
  assert.equal(h.cvars.get('s2_khook_accept_reload_ready'), '1');
  assert.equal(h.cvars.get('s2_khook_accept_reload_trace'), '1:pre,1:post,');
});

test('failed marker creation does not leave subscriptions that duplicate a retry', () => {
  const h = host(); h.control('prepare'); const target = h.nativeTarget();
  h.cvars.set('s2_khook_accept_reload_target', String(target.index));
  const create = h.sdk.createEntity; h.sdk.createEntity = () => null;
  h.control('reload-arm');
  assert.notEqual(h.cvars.get('s2_khook_accept_reload_ready'), '1');
  h.sdk.createEntity = create; h.control('reload-arm'); h.touchReload(target);
  assert.equal(h.cvars.get('s2_khook_accept_reload_trace'), '1:pre,1:post,');
});

test('reload handoff cannot complete an earlier pending phase', () => {
  const { h, target } = armedReload();
  assert.equal(row(h, 'js_phase_final_unsubscribe').result, 'pending');
  const next = h.reload(); next.control('resume'); next.touchReload(target);
  assert.equal(row(next, 'js_fresh_subscription_after_reload').result, 'pass');
  assert.equal(row(next, 'js_phase_final_unsubscribe').result, 'pending');
});

test('replacement resume retries an unavailable target without losing prior evidence', () => {
  const { h, target } = armedReload(); h.invoke(); const before = row(h, 'js_phase_subscribe_pre_post');
  const next = h.reload(); target.valid = false; next.control('resume');
  target.valid = true; next.control('resume'); next.touchReload(target);
  assert.equal(row(next, 'js_fresh_subscription_after_reload').result, 'pass');
  assert.deepEqual(row(next, 'js_phase_subscribe_pre_post'), before);
});

test('client commands require the active run and a real connected client', () => {
  const h = host(); h.control('prepare');
  const f = h.listeners.get('s2khook_cc_entry');
  assert.equal(typeof f, 'function');
  f(0, 'other-run s2khook-continue'); f(-1, 'run-1 s2khook-continue');
  assert.equal(row(h, 'js_command_continue_delivery').result, 'pending');
  f(0, 'run-1 s2khook-continue');
  assert.equal(row(h, 'js_command_continue_delivery').result, 'pass');
});


test('B/C missing native descriptors remain pending and never borrow A records', () => {
  for (const suite of ['B', 'C']) {
    const h = host(); h.control('prepare', 'run-1', ARTIFACT, suite);
    const records = h.control('collect', 'run-1', '', suite);
    assert.ok(records.length > 0);
    assert.ok(records.every(r => r.suite === suite && r.result === 'pending'));
    assert.ok(records.every(r => !r.subcheck.includes('js_gameframe')));
    assert.ok(h.control('report', 'run-1', '', suite).every(r => r.result === 'pending'));
  }
});

test('cross-suite report is a controller error without changing bound suite', () => {
  const h = host(); h.control('prepare', 'run-1', ARTIFACT, 'B');
  const rejected = h.control('report', 'run-1', '', 'C');
  assert.match(rejected[0].khook_acceptance_error, /suite\/run mismatch/);
  assert.ok(h.control('report', 'run-1', '', 'B').every(r => r.suite === 'B'));
});

test('archive handoff cannot resume another suite', () => {
  const h = host(); h.control('prepare', 'run-1', ARTIFACT, 'B');
  const next = h.reload();
  const rejected = next.control('resume', 'run-1', ARTIFACT, 'C');
  assert.match(rejected[0].khook_acceptance_error, /handoff mismatch/);
});
