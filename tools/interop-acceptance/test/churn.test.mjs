import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Churn } from '../shared/churn.ts';

function rig(options = {}) {
  let loaded = true, attached = 1, detached = 0, hits = 0, pending = null;
  const calls = [];
  const io = {
    loaded: () => loaded,
    counts: () => ({ attached, detached, hits }),
    resources: () => ({ subscriptions: loaded ? 3 : 0, ledger: (loaded ? 8 : 2) + (options.leak && detached > 1 ? 1 : 0) }),
    load() { calls.push('load'); pending = true; return true; },
    unload() { calls.push('unload'); pending = false; return true; },
    probe() { hits += options.duplicate ? 2 : 1; },
    stale() { return !loaded; },
  };
  const churn = new Churn(io);
  function frame() {
    if (pending !== null && !options.stall) {
      loaded = pending; if (loaded) attached++; else detached++;
      pending = null;
    }
    churn.tick();
  }
  return { churn, frame, calls };
}
test('1000 cycles require distinct unload/attach transitions, single deliveries and equal resource baselines', () => {
  const { churn, frame, calls } = rig();
  churn.start();
  for (let i = 0; i < 10000 && churn.status.state === 'running'; i++) frame();
  assert.equal(churn.status.state, 'done');
  assert.equal(churn.status.cycles, 1000);
  assert.equal(churn.status.deliveries, 1000);
  assert.equal(churn.status.staleBlocked, 1000);
  assert.equal(calls.filter(x => x === 'load').length, 1000);
  assert.equal(calls.filter(x => x === 'unload').length, 1001);
  assert.deepEqual(churn.status.baseline, { subscriptions: 0, ledger: 2 });
  assert.deepEqual(churn.status.final, { subscriptions: 0, ledger: 2 });
});
for (const [name, options, error] of [
  ['duplicate callback', { duplicate: true }, /delivery/],
  ['surviving resource', { leak: true }, /resource/],
  ['deferred operation stalls', { stall: true }, /timeout/],
]) test(`fails closed when ${name}`, () => {
  const { churn, frame } = rig(options); churn.start();
  for (let i = 0; i < 10000 && churn.status.state === 'running'; i++) frame();
  assert.equal(churn.status.state, 'failed');
  assert.match(churn.status.error, error);
});
test('map interruption aborts an active run and cannot silently count as a completed cycle', () => {
  const { churn, frame } = rig(); churn.start(); frame(); churn.mapChanged();
  assert.equal(churn.status.state, 'failed');
  assert.match(churn.status.error, /map/);
});
