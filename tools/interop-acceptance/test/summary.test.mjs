import {test} from 'node:test';
import assert from 'node:assert/strict';
import {checkProbe, checkChurn, checkServices} from '../verify.mjs';
const probe = () => ({ a: {action: 2, result: 1, original: '{"value":1,"mode":"normal"}', final: '{"value":12,"mode":"normal"}'}, b: {action: 2, result: 1, original: '{"text":"seed","mode":"normal"}', final: '{"text":"seed!!","mode":"normal"}'}, normal: {numeric: 1, text: 1}, stop: {numeric: 3, text: 3}, malformed: 6, malformedDelivered: 0, recursion: 1, recovery: 2, isolationFailures: 0, resources: {pending: 0} });
test('probe evidence rejects incorrect transforms, missed rejections and duplicate callbacks', () => {
  assert.doesNotThrow(() => checkProbe(probe()));
  for (const mutate of [x => x.a.final = '{"value":11,"mode":"normal"}', x => x.normal.numeric = 2, x => x.recursion = 0, x => x.malformedDelivered = 1]) {
    const value = probe(); mutate(value); assert.throws(() => checkProbe(value));
  }
});
const resources = () => ({watches: 1, callbacks: 1, attachments: 0, disposers: 0, pending: 0, subscriptions: 4, methods: 2, ledger: 8});
test('churn evidence requires completed cycles and matching actual baselines', () => {
  const value = {state: 'done', cycles: 1000, deliveries: 1000, staleBlocked: 1000, error: '', baseline: resources(), final: resources()};
  assert.doesNotThrow(() => checkChurn(value));
  assert.throws(() => checkChurn({...value, state: 'running'}));
  assert.throws(() => checkChurn({...value, final: {subscriptions: 5}}));
});
test('service evidence requires transition notifications and cleanup', () => {
  const value = {mute: true, gag: true, invalidMute: false, invalidBan: {recorded:false,result:0},ban:{recorded:true,result:0},unban:true,events:{mute:2,gag:2,request:1,recorded:1,removed:1},cleaned:true};
  assert.doesNotThrow(() => checkServices(value));
  assert.throws(() => checkServices({...value,cleaned:false}));
});

test('churn evidence rejects incomplete, unsafe and pending resource snapshots', () => {
  for (const side of ['baseline', 'final']) {
    for (const key of Object.keys(resources())) {
      for (const invalid of [undefined, -1, 0.5, Number.MAX_SAFE_INTEGER + 1, NaN, Infinity, '0']) {
        const snapshot = {...resources(), [key]: invalid};
        const value = {state: 'done', cycles: 1000, deliveries: 1000, staleBlocked: 1000, error: '', baseline: resources(), final: resources(), [side]: snapshot};
        assert.throws(() => checkChurn(value), `${side}.${key}=${invalid}`);
        assert.throws(() => checkChurn({...value, baseline: snapshot, final: snapshot}), `matching invalid ${key}`);
      }
    }
  }
  for (const snapshot of [{pending: 9}, {...resources(), pending: 9}, {}]) {
    assert.throws(() => checkChurn({state: 'done', cycles: 1000, deliveries: 1000, staleBlocked: 1000, error: '', baseline: snapshot, final: snapshot}));
  }
});
