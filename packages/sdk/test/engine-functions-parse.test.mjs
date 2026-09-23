import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseFunctionFile } from '../src/engine-functions/parse.ts';

test('missing file means no capability', () => {
  assert.equal(parseFunctionFile('gamedata/functions.jsonc', undefined), undefined);
});

test('JSONC parses comments and retains a complete declaration', () => {
  const parsed = parseFunctionFile('functions.jsonc', `{
    // audited target
    "schemaVersion": 2, "functions": {"commitSuicide": {
      "target": {"module":"libserver.so","pattern":"55 48","validate":{"prologue":"55 48"}},
      "receiver":{"type":"entity"},
      "parameters":[{"name":"explode","type":"bool"},{"name":"force","type":"bool","mutable":"pre"}],
      "surfaces":["call","pre","post"]
    }}
  }`);
  assert.equal(parsed.functions.commitSuicide.parameters[1].mutable, 'pre');
});

test('rejects duplicate keys at every object depth', () => {
  for (const src of [
    '{"schemaVersion":2,"schemaVersion":2,"functions":{}}',
    '{"schemaVersion":2,"functions":{"x":{},"x":{}}}',
    '{"schemaVersion":2,"functions":{"x":{"target":{"module":"a","module":"b"}}}}',
    '{"schemaVersion":2,"functions":{"x":{"parameters":[{"name":"a","name":"b"}]}}}',
  ]) assert.throws(() => parseFunctionFile('functions.jsonc', src), /duplicate.*(key|property)/i);
});

test('rejects trailing commas and syntax errors', () => {
  assert.throws(() => parseFunctionFile('functions.jsonc', '{"schemaVersion":2,"functions":{},}'), /invalid|trailing/i);
  assert.throws(() => parseFunctionFile('functions.jsonc', '{bad'), /invalid/i);
});
