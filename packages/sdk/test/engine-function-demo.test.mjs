import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { unzipSync } from 'fflate';
import { transformSync } from 'esbuild';
import { runInNewContext } from 'node:vm';
import { buildPlugin } from '../src/build.ts';
import { typecheckPlugin } from '../src/typecheck/typecheck.ts';
import { parseFunctionFile } from '../src/engine-functions/parse.ts';

const root = resolve(import.meta.dirname, '../../..');
const example = join(root, 'examples/engine-function-demo');
const read = path => readFileSync(path, 'utf8');
const unpack = path => Object.fromEntries(Object.entries(unzipSync(readFileSync(path))).map(([key, value]) => [key, Buffer.from(value).toString('utf8')]));

test('ordinary example derives its archive, permissions, and generated declarations', async () => {
  const pkg = JSON.parse(read(join(example, 'package.json')));
  assert.equal(pkg.name, '@demo/engine-function');
  assert.equal(pkg.s2script?.gamedata, undefined);
  assert.equal(pkg.s2script?.requiresGamedata, undefined);
  assert.equal(pkg.s2script?.permissions, undefined);
  assert.doesNotMatch(read(join(example, 'tsconfig.json')), /\.s2script/);
  const members = unpack(await buildPlugin(example));
  const manifest = JSON.parse(members['manifest.json']);
  const bundle = JSON.parse(members['engine-functions.json']);
  assert.deepEqual(manifest.permissions, ['engine:calls', 'engine:hooks']);
  assert.equal(manifest.engineFunctions.bundleHash, bundle.bundleHash);
  assert.deepEqual(bundle.functions.map(f => f.canonicalId), [
    '@demo/engine-function::commitSuicide', '@demo/engine-function::staleEntryProbe',
  ]);
  const good = bundle.functions[0];
  assert.deepEqual(good.policy.surfaces, ['call', 'pre', 'post']);
  assert.equal(good.target.targetValidate.prologue, '55 48 89 E5 41 55 41 89 F5 41 54 41 89 D4');
  assert.match(read(join(example, '.s2script/engine-functions.d.ts')), /interface CommitSuicidePreView/);
  assert.match(read(join(example, '.s2script/engine-functions.d.ts')), /readonly explode/);
  assert.ok(members['plugin.js']);
});

test('generated example types reject wrong arguments, readonly PRE edits, and POST overrides', async () => {
  await buildPlugin(example);
  const dir = mkdtempSync(join(tmpdir(), 's2-ef-example-neg-'));
  try {
    mkdirSync(join(dir, 'src'));
    mkdirSync(join(dir, 'gamedata'));
    writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: '@demo/negative', version: '1.0.0', main: 'src/plugin.ts' }));
    const data = parseFunctionFile('functions.jsonc', read(join(example, 'gamedata/functions.jsonc')));
    data.functions.returningFixture = { ...data.functions.commitSuicide, returns: 'bool' };
    writeFileSync(join(dir, 'gamedata/functions.jsonc'), JSON.stringify(data));
    writeFileSync(join(dir, 'src/plugin.ts'), `
import { Engine } from '@s2script/sdk/unsafe';
import { HookResult } from '@s2script/sdk';
const f = Engine.function('commitSuicide');
if (f.available) {
  f.call(null, false);
  f.onPre(v => { v.explode = true; return HookResult.Changed; });
  f.onPost(v => { v.returnValue = true; });
}
const g = Engine.function('returningFixture');
if (g.available) g.onPre(() => HookResult.Handled);
export function OnPluginStart(): void {}
`);
    const result = typecheckPlugin(dir);
    assert.equal(result.ok, false);
    const errors = result.diagnostics.filter(d => d.file.endsWith('plugin.ts'));
    assert.ok(errors.length >= 4, JSON.stringify(result.diagnostics));
    assert.ok(errors.some(d => d.code === 2554), JSON.stringify(errors));
    assert.ok(errors.filter(d => d.code === 2540).length >= 2, JSON.stringify(errors));
    assert.ok(errors.some(d => /Handled|3/.test(d.message)), JSON.stringify(errors));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('example exposes status, subscription lifecycle, and an explicit guarded bot effect path', () => {
  const source = read(join(example, 'src/plugin.ts'));
  assert.match(source, /Engine\.function\("commitSuicide"\)/);
  assert.match(source, /Engine\.function\("staleEntryProbe"\)/);
  assert.doesNotMatch(source, /Engine\.(call|hook)\(/);
  assert.match(source, /\.onPre\(/);
  assert.match(source, /\.onPost\(/);
  assert.match(source, /\.dispose\(/);
  assert.match(source, /command\("sm_ef_suicide_bot"/);
  assert.match(source, /\.isBot/);
  assert.match(source, /\.health/);
  assert.match(source, /\.call\(pawn\.ref, false, true\)/);
  assert.doesNotMatch(source, /pawn\.slay\(/);
  const oldSource = read(join(root, 'examples/engine-call-demo/src/plugin.ts'));
  const oldReadme = read(join(root, 'examples/engine-call-demo/README.md'));
  assert.match(oldSource, /Engine\.call\("ignite"\)/);
  assert.match(oldReadme, /historical/i);
  assert.match(oldReadme, /current.*unavailable/is);
  assert.match(oldReadme, /trailing.*Vector/is);
  assert.match(oldReadme, /replacing.*pattern.*does not/is);
});

test('public binding fixture registers, disposes, reports status, and refuses a human without calling native', () => {
  const commands = new Map();
  const logs = [];
  let nativeCalls = 0;
  let disposed = 0;
  let pre = 0;
  const status = Object.freeze({ canonicalId: '@demo/engine-function::commitSuicide', availability: 'available', reason: null,
    hookObservation: 'pending', provenance: Object.freeze({ archiveHash: 'archive', baseContractHash: 'contract',
      appliedOverrides: Object.freeze([]), finalTargetHash: 'target', resolverReceipt: 'receipt', required: false }) });
  const binding = { available: true, status,
    call() { nativeCalls++; },
    onPre() { pre++; return { status: 'pending', reason: null, dispose() { disposed++; this.status = 'disposed'; return true; } }; },
    onPost() { return { status: 'pending', reason: null, dispose() { return true; } }; } };
  const stale = { available: false, status: Object.freeze({ ...status, canonicalId: '@demo/engine-function::staleEntryProbe', availability: 'unavailable', reason: 'prologue mismatch' }) };
  const imports = {
    '@s2script/sdk/unsafe': { Engine: { function(name) { return name === 'commitSuicide' ? binding : stale; } } },
    '@s2script/sdk': { Clients: { fromSlot() { return { isValid: () => true, isBot: false, signonState: 6, userId: 10 }; } },
      command(name, handler) { commands.set(name, handler); }, HookResult: { Handled: 3, Changed: 1 } },
    '@s2script/cs2': { Player: { fromSlot() { throw Error('human pawn must not be touched'); } } },
  };
  const js = transformSync(read(join(example, 'src/plugin.ts')), { loader: 'ts', format: 'cjs' }).code;
  const module = { exports: {} };
  runInNewContext(js, { module, exports: module.exports, require: name => imports[name], console: { log: text => logs.push(text), error: text => logs.push(text) } });
  module.exports.OnPluginStart();
  assert.equal(nativeCalls, 0);
  assert.equal(pre, 2);
  assert.equal(disposed, 1);
  assert.ok(logs.some(line => line.includes('"archiveHash":"archive"')));
  const replies = [];
  commands.get('sm_ef_suicide_bot')({ callerSlot: -1, arg: () => '2', reply: text => replies.push(text) });
  assert.equal(nativeCalls, 0);
  assert.match(replies[0], /refused/);
});

test('valid-bot command uses exact call arguments and reports API health reads (control flow only)', () => {
  // The health transition is a host/API fixture. It proves command wiring, not native gameplay.
  const commands = new Map();
  const pawnRef = { index: 41, id: 207 };
  let health = 100;
  let healthReads = 0;
  const pawn = { isValid: true, ref: pawnRef, get health() { healthReads++; return health; } };
  const bot = { isValid: () => true, isBot: true, signonState: 6, userId: 27 };
  const calls = [];
  const handle = () => ({ status: 'active', reason: null, dispose: () => true });
  const binding = { available: true, status: { canonicalId: '@demo/engine-function::commitSuicide' },
    onPre: handle, onPost: handle,
    call(...args) { calls.push(args); health = 0; } };
  const imports = {
    '@s2script/sdk/unsafe': { Engine: { function(name) { return name === 'commitSuicide'
      ? binding : { available: false, status: { reason: 'prologue mismatch' } }; } } },
    '@s2script/sdk': { Clients: { fromSlot(slot) { assert.equal(slot, 3); return bot; } },
      command(name, handler) { commands.set(name, handler); }, HookResult: { Handled: 3, Changed: 1 } },
    '@s2script/cs2': { Player: { fromSlot(slot) { assert.equal(slot, 3); return { userId: 27, pawn }; } } },
  };
  const js = transformSync(read(join(example, 'src/plugin.ts')), { loader: 'ts', format: 'cjs' }).code;
  const module = { exports: {} };
  runInNewContext(js, { module, exports: module.exports, require: name => imports[name],
    console: { log() {}, error() {} } });
  module.exports.OnPluginStart();
  assert.equal(calls.length, 0, 'startup must not invoke the lethal binding');
  const replies = [];
  commands.get('sm_ef_suicide_bot')({ callerSlot: -1, arg: () => '3', reply: text => replies.push(text) });
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], pawnRef);
  assert.equal(calls[0][1], false);
  assert.equal(calls[0][2], true);
  assert.equal(healthReads >= 2, true, 'health must be read both before and after call');
  assert.equal(replies.length, 1);
  assert.match(replies[0], /alive true -> false; health 100 -> 0; sameBot=true; samePawn=true/);
});
