// Validate a single JSON reply captured from the corresponding live server command.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
export function checkProbe(value) {
  for (const [name, original, final] of [
    ['a', {value: 1, mode: 'normal'}, {value: 12, mode: 'normal'}],
    ['b', {text: 'seed', mode: 'normal'}, {text: 'seed!!', mode: 'normal'}],
  ]) {
    assert.equal(value[name].action, 2); assert.equal(value[name].result, 1);
    assert.deepEqual(JSON.parse(value[name].original), original);
    assert.deepEqual(JSON.parse(value[name].final), final);
  }
  assert.deepEqual(value.normal, {numeric: 1, text: 1});
  assert.deepEqual(value.stop, {numeric: 3, text: 3});
  for (const [key, expected] of Object.entries({malformed: 6, malformedDelivered: 0, recursion: 1, recovery: 2, isolationFailures: 0})) assert.equal(value[key], expected, key);
  assert.equal(value.resources.pending, 0);
}
export function checkChurn(value) {
  assert.equal(value.state, 'done'); assert.equal(value.error, '');
  for (const key of ['cycles', 'deliveries', 'staleBlocked']) assert.equal(value[key], 1000, key);
  assert.ok(Object.keys(value.baseline).length > 0);
  assert.deepEqual(value.final, value.baseline);
}
export function checkServices(value) {
  for (const key of ['mute', 'gag', 'unban', 'cleaned']) assert.equal(value[key], true, key);
  assert.equal(value.invalidMute, false);
  assert.deepEqual(value.invalidBan, {recorded: false, result: 0});
  assert.deepEqual(value.ban, {recorded: true, result: 0});
  assert.deepEqual(value.events, {mute: 2, gag: 2, request: 1, recorded: 1, removed: 1});
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const checks = {probe: checkProbe, churn: checkChurn, services: checkServices};
  const check = checks[process.argv[2]];
  if (!check || !process.argv[3]) throw Error('usage: node verify.mjs probe|churn|services reply.json');
  check(JSON.parse(readFileSync(process.argv[3], 'utf8')));
  console.log(`${process.argv[2]}: captured evidence matches expected values`);
}
