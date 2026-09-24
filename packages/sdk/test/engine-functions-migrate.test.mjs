import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { analyzeV1Migration, migrateV1Package } from '../src/engine-functions/migrate-v1.ts';
import { normalizeFunctions, summarizeFunctions } from '../src/engine-functions/normalize.ts';
import { parseFunctionFile } from '../src/engine-functions/parse.ts';

const signature = { linuxsteamrt64: { module: 'libserver.so', pattern: '55 48', resolve: 'direct', validate: { prologue: '55 48' } } };
const call = { receiver: { kind: 'entity' }, target: { kind: 'signature', name: 'Target' }, args: ['bool', 'int', 'float', 'entity'], argNames: ['enabled', 'count', 'scale', 'client'], returns: 'bool' };
function fixture(gamedata = { signatures: { Target: signature }, calls: { run: call } }, source = 'export function OnPluginStart() { Engine.call("run"); }') {
  const dir = mkdtempSync(join(tmpdir(), 's2-migrate-'));
  mkdirSync(join(dir, 'gamedata')); mkdirSync(join(dir, 'src'));
  writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: '@demo/migrate', version: '1.0.0', main: 'src/plugin.ts', s2script: { gamedata: 'gamedata/migrate.gamedata.jsonc', permissions: ['engine:calls', ...(gamedata.hooks ? ['engine:hooks'] : [])] } }));
  writeFileSync(join(dir, 'gamedata/migrate.gamedata.jsonc'), JSON.stringify(gamedata));
  writeFileSync(join(dir, 'src/plugin.ts'), source);
  return dir;
}
function output(dir) { return join(dir, 'gamedata/functions.jsonc'); }
function cleanup(dir) { rmSync(dir, { recursive: true, force: true }); }

test('converts declared scalar/member/bool contract, preserving validator and deriving permissions', () => {
  const dir = fixture();
  try {
    const report = migrateV1Package(dir);
    assert.deepEqual(report.ambiguities, []);
    assert.equal(report.scope, 'structurally lossless conversion of declared v1 contract; native ABI completeness is not verified');
    assert.deepEqual(report.filesChanged, [output(dir)]);
    assert.equal(report.recommendedEdits.some(x => x.includes('package.json')), true);
    const parsed = parseFunctionFile(output(dir), readFileSync(output(dir), 'utf8'));
    const bundle = normalizeFunctions('@demo/migrate', parsed);
    assert.deepEqual(summarizeFunctions(bundle).permissions, ['engine:calls']);
    assert.equal(bundle.functions[0].abi.fingerprint, 'linux-x86_64-sysv:entity:u8(u8,i32,f32,ptr)');
    assert.deepEqual(bundle.functions[0].abi.parameters.map(p => p.name), call.argNames);
    assert.deepEqual(bundle.functions[0].abi.parameters[0].projection, { id: 'bool', version: 1 });
    assert.deepEqual(bundle.functions[0].abi.returns.projection, { id: 'bool', version: 1 });
    assert.deepEqual(bundle.functions[0].target.targetValidate, { prologue: '55 48' });
    assert.equal(report.entries[0].canonicalId, '@demo/migrate::run');
    assert.equal(report.sourceReferences[0].name, 'run');
    assert.match(readFileSync(join(dir, 'src/plugin.ts'), 'utf8'), /Engine.call/);
    assert.ok(JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')).s2script.gamedata);
  } finally { cleanup(dir); }
});

test('ordinary named Engine import and direct static call remain analyzable', () => {
  const dir = fixture(undefined, 'import { Engine } from "@s2script/sdk/unsafe"; Engine.call("run");');
  try {
    const report = migrateV1Package(dir);
    assert.deepEqual(report.ambiguities, []);
    assert.equal(report.sourceReferences[0].name, 'run');
    assert.equal(existsSync(output(dir)), true);
  } finally { cleanup(dir); }
});

test('analyzes identical bypass pair but refuses nullable v1 hook receiver', () => {
  const gd = { signatures: { Target: signature }, calls: { run: { ...call, args: [], argNames: [], returns: 'void' } }, hooks: { onRun: { target: { kind: 'signature', name: 'Target' }, shape: 'this_void', params: [], mutable: [], receiver: { kind: 'entity', as: 'receiver' }, bypassWith: 'run', expose: { ctx: 'things' } } } };
  const dir = fixture(gd, 'export function OnPluginStart(ctx) { ctx.things.onRun(() => {}); Engine.hook("onRun")?.(() => {}); }');
  try {
    const report = migrateV1Package(dir);
    assert.match(report.ambiguities.join(' '), /nullable.*receiver.*v2/i);
    assert.equal(existsSync(output(dir)), false);
    assert.deepEqual(report.filesChanged, []);
    assert.equal(report.entries.length, 1);
    assert.equal(report.entries[0].oldHook, 'onRun');
    assert.equal(report.entries[0].oldCall, 'run');
    assert.equal(report.entries[0].oldExposeCtx, 'things');
    assert.equal(report.entries[0].oldReceiverAs, 'receiver');
    const bundle = normalizeFunctions('@demo/migrate', report.output);
    assert.deepEqual(bundle.functions[0].policy.surfaces, ['call', 'pre']);
    assert.equal(bundle.functions[0].policy.selfCall, 'bypass-own-hooks');
    assert.equal(report.sourceReferences.length, 2);
  } finally { cleanup(dir); }
});

test('preserves candidate and target validator stages for validated-call', () => {
  const gd = { signatures: { Target: { linuxsteamrt64: { module: 'libserver.so', pattern: 'E8 ??', resolve: 'validated-call', validate: { 'string-xref': { at: 0, dispOff: 1, instrLen: 5, expect: 'needle' } }, targetValidate: { prologue: '55 48' } } } }, calls: { run: { ...call, args: [], argNames: [], returns: 'void' } } };
  const dir = fixture(gd);
  try {
    const report = migrateV1Package(dir);
    assert.deepEqual(report.ambiguities, []);
    const bundle = normalizeFunctions('@demo/migrate', parseFunctionFile(output(dir), readFileSync(output(dir), 'utf8')));
    assert.deepEqual(bundle.functions[0].target.candidateValidate, gd.signatures.Target.linuxsteamrt64.validate);
    assert.deepEqual(bundle.functions[0].target.targetValidate, { prologue: '55 48' });
  } finally { cleanup(dir); }
});

test('standalone entity hook is refused even when shape and mutation are representable', () => {
  const gd = { signatures: { Target: signature }, hooks: { onRun: { target: { kind: 'signature', name: 'Target' }, shape: 'this_f32_i32_i32_i32', params: ['scale', 'a', 'b', 'c'], mutable: ['scale'], receiver: { kind: 'entity', as: 'receiver' }, expose: { ctx: 'things' } } } };
  const dir = fixture(gd, 'export function OnPluginStart(ctx) { ctx.things.onRun(view => { view.scale = 2; }); }');
  try {
    const report = migrateV1Package(dir);
    assert.match(report.ambiguities.join(' '), /nullable.*receiver.*v2/i);
    assert.equal(existsSync(output(dir)), false);
    assert.deepEqual(report.permissions, ['engine:hooks']);
    assert.equal(report.entries[0].mutates, true);
    const bundle = normalizeFunctions('@demo/migrate', report.output);
    assert.deepEqual(bundle.functions[0].policy.surfaces, ['pre']);
    assert.deepEqual(bundle.functions[0].abi.parameters.map(p => p.mutable), [['pre'], [], [], []]);
  } finally { cleanup(dir); }
});

test('virtual target carries the proven v1 module default or an explicit module', () => {
  for (const module of [undefined, 'libother.so']) {
    const target = { kind: 'vtable', class: 'CExample', linuxsteamrt64: { index: 24, validate: { prologue: '55 48' } }, ...(module && { module }) };
    const gd = { calls: { run: { ...call, target, args: [], argNames: [], returns: 'void' } } };
    const dir = fixture(gd);
    try {
      const report = migrateV1Package(dir);
      assert.deepEqual(report.ambiguities, []);
      const bundle = normalizeFunctions('@demo/migrate', parseFunctionFile(output(dir), readFileSync(output(dir), 'utf8')));
      assert.equal(bundle.functions[0].target.kind, 'virtual');
      assert.equal(bundle.functions[0].target.module, module ?? 'libserver.so');
      assert.equal(bundle.functions[0].target.index, 24);
      assert.deepEqual(bundle.functions[0].target.targetValidate, { prologue: '55 48' });
    } finally { cleanup(dir); }
  }
});

test('four declared scalars stay four and report does not certify the native ABI', () => {
  const dir = fixture();
  try {
    const report = migrateV1Package(dir);
    const bundle = normalizeFunctions('@demo/migrate', parseFunctionFile(output(dir), readFileSync(output(dir), 'utf8')));
    assert.equal(bundle.functions[0].abi.parameters.length, 4);
    assert.match(report.scope, /native ABI completeness is not verified/);
  } finally { cleanup(dir); }
});

test('scans local source imports outside src and refuses dynamic references', () => {
  const dir = fixture(undefined, "import '../helpers.ts'; export function OnPluginStart() {}");
  try {
    writeFileSync(join(dir, 'helpers.ts'), 'Engine.call(name);');
    const report = migrateV1Package(dir);
    assert.match(report.ambiguities.join(' '), /helpers\.ts.*dynamic Engine\.call/);
    assert.equal(existsSync(output(dir)), false);
  } finally { cleanup(dir); }
});

test('refuses Engine and ctx aliases and escapes rather than silently omitting source edits', () => {
  for (const source of [
    'const { call } = Engine; call(name);',
    'const lookup = Engine.call; lookup(name);',
    'const E = Engine; E.call(name);',
    'import { Engine as E } from "@s2script/sdk/unsafe"; E.call(name);',
    'const E = globalThis.Engine; E.call(name);',
    'import * as unsafe from "@s2script/sdk/unsafe"; const E = unsafe.Engine; E.call(name);',
    'const E = require("@s2script/sdk/unsafe").Engine; E.call(name);',
  ]) {
    const dir = fixture(undefined, source);
    try {
      const report = migrateV1Package(dir);
      assert.match(report.ambiguities.join(' '), /indirect.*Engine|Engine.*alias|Engine.*escape/i, source);
      assert.equal(existsSync(output(dir)), false);
    } finally { cleanup(dir); }
  }
  const gd = { signatures: { Target: signature }, hooks: { onRun: { target: { kind: 'signature', name: 'Target' }, shape: 'this_void', params: [], receiver: { kind: 'entity', as: 'receiver' }, expose: { ctx: 'things' } } } };
  for (const source of [
    'const subscribe = ctx.things.onRun; subscribe(handler);',
    'const { onRun } = ctx.things; onRun(handler);',
    'const alias = ctx; alias.things.onRun(handler);',
    'export function OnPluginStart({ things }) { things.onRun(handler); }',
  ]) {
    const dir = fixture(gd, source);
    try {
      const report = migrateV1Package(dir);
      assert.match(report.ambiguities.join(' '), /indirect.*ctx|ctx.*alias|ctx.*escape/i, source);
      assert.equal(existsSync(output(dir)), false);
    } finally { cleanup(dir); }
  }
});

test('follows bounded local require and dynamic import edges outside src', () => {
  for (const edge of ['require("../helpers.js")', 'import("../helpers.js")']) {
    const dir = fixture(undefined, `export function OnPluginStart() { ${edge}; }`);
    try {
      writeFileSync(join(dir, 'helpers.js'), 'Engine.call(dynamicName);');
      const report = migrateV1Package(dir);
      assert.match(report.ambiguities.join(' '), /helpers\.js.*dynamic Engine\.call/, edge);
      assert.equal(existsSync(output(dir)), false);
    } finally { cleanup(dir); }
  }
});

test('rejects unresolved local require and dynamic import edges', () => {
  for (const edge of ['require("../missing.js")', 'import("../missing.js")', 'require(moduleName)']) {
    const dir = fixture(undefined, `export function OnPluginStart() { ${edge}; }`);
    try {
      const report = migrateV1Package(dir);
      assert.match(report.ambiguities.join(' '), /unresolved local module|dynamic module edge/i, edge);
      assert.equal(existsSync(output(dir)), false);
    } finally { cleanup(dir); }
  }
});

test('aggregates ambiguities and writes nothing for incomplete or unsupported declarations', () => {
  const cases = [
    ['missing names', gd => { delete gd.calls.run.argNames; }, /argNames/],
    ['empty validator', gd => { gd.signatures.Target.linuxsteamrt64.validate = {}; }, /validator/],
    ['receiver hop', gd => { gd.calls.run.receiver.via = { class: 'Pawn', field: 'item' }; }, /receiver\.via/],
    ['copied ownership', gd => { gd.calls.run.args[0] = 'string'; }, /ownership/],
    ['unsupported ABI', gd => { gd.calls.run.args[0] = 'utlstring'; }, /utlstring/],
    ['dynamic source', (_gd, src) => 'export function OnPluginStart() { Engine.call(name); }', /dynamic/],
  ];
  for (const [label, mutate, expected] of cases) {
    const gd = structuredClone({ signatures: { Target: signature }, calls: { run: call } });
    const dir = fixture(gd);
    try {
      const source = mutate(gd);
      writeFileSync(join(dir, 'gamedata/migrate.gamedata.jsonc'), JSON.stringify(gd));
      if (typeof source === 'string') writeFileSync(join(dir, 'src/plugin.ts'), source);
      const report = migrateV1Package(dir);
      assert.equal(report.filesChanged.length, 0, label);
      assert.equal(existsSync(output(dir)), false, label);
      assert.match(report.ambiguities.join(' '), expected, label);
    } finally { cleanup(dir); }
  }
});

test('disagreeing paired targets and ABIs are refused without output', () => {
  for (const mismatch of ['target', 'abi']) {
    const gd = { signatures: { Target: signature, Other: { linuxsteamrt64: { ...signature.linuxsteamrt64, pattern: '55 49' } } }, calls: { run: { ...call, args: [], argNames: [], returns: 'void' } }, hooks: { onRun: { target: { kind: 'signature', name: mismatch === 'target' ? 'Other' : 'Target' }, shape: mismatch === 'abi' ? 'this_f32_i32_i32_i32' : 'this_void', params: mismatch === 'abi' ? ['a', 'b', 'c', 'd'] : [], receiver: { kind: 'entity', as: 'receiver' }, bypassWith: 'run', expose: { ctx: 'things' } } } };
    const dir = fixture(gd);
    try { const report = migrateV1Package(dir); assert.match(report.ambiguities.join(' '), new RegExp(mismatch, 'i')); assert.equal(existsSync(output(dir)), false); }
    finally { cleanup(dir); }
  }
});

test('same target without explicit bypass pairing and multiple hooks per call are ambiguous', () => {
  const base = { target: { kind: 'signature', name: 'Target' }, shape: 'this_void', params: [], receiver: { kind: 'entity', as: 'receiver' }, expose: { ctx: 'things' } };
  for (const hooks of [{ onRun: base }, { onRun: { ...base, bypassWith: 'run' }, onRunAgain: { ...base, bypassWith: 'run' } }]) {
    const gd = { signatures: { Target: signature }, calls: { run: { ...call, args: [], argNames: [], returns: 'void' } }, hooks };
    const dir = fixture(gd);
    try {
      const report = migrateV1Package(dir);
      assert.match(report.ambiguities.join(' '), /without explicit bypassWith|already paired/);
      assert.equal(existsSync(output(dir)), false);
    } finally { cleanup(dir); }
  }
});

test('existing destination is refused; force reports replacement and retains old file on analysis failure', () => {
  const dir = fixture();
  try {
    writeFileSync(output(dir), 'old');
    assert.match(analyzeV1Migration(dir).ambiguities.join(' '), /already exists/);
    const refused = migrateV1Package(dir);
    assert.equal(refused.filesChanged.length, 0);
    assert.equal(readFileSync(output(dir), 'utf8'), 'old');
    const report = migrateV1Package(dir, { force: true });
    assert.deepEqual(report.publication, { action: 'create-or-replace', path: output(dir), priorExistence: 'cannot-be-established-under-concurrency' });
    assert.equal(report.replacedFile, undefined);
    assert.notEqual(readFileSync(output(dir), 'utf8'), 'old');
  } finally { cleanup(dir); }
});

test('force preserves destination and removes its temp file on write failure', () => {
  const dir = fixture();
  try {
    mkdirSync(output(dir));
    const report = migrateV1Package(dir, { force: true });
    assert.match(report.ambiguities.join(' '), /publication failed/);
    assert.equal(existsSync(output(dir)), true);
    assert.deepEqual(readdirSync(join(dir, 'gamedata')).filter(x => x.endsWith('.tmp')), []);
    assert.deepEqual(report.filesChanged, []);
  } finally { cleanup(dir); }
});

test('force preserves existing file when analysis reports an ambiguity', () => {
  const gd = { signatures: { Target: signature }, calls: { run: { ...call, argNames: [] } } };
  const dir = fixture(gd);
  try {
    writeFileSync(output(dir), 'keep me');
    const report = migrateV1Package(dir, { force: true });
    assert.match(report.ambiguities.join(' '), /argNames/);
    assert.equal(report.replacedFile, undefined);
    assert.equal(report.publication, undefined);
    assert.equal(readFileSync(output(dir), 'utf8'), 'keep me');
  } finally { cleanup(dir); }
});

test('real CLI emits report and exit codes', () => {
  const dir = fixture();
  try {
    const cli = join(import.meta.dirname, '../dist/cli.js');
    const ok = spawnSync(process.execPath, [cli, 'migrate', 'engine-functions', dir], { encoding: 'utf8' });
    assert.equal(ok.status, 0, ok.stderr);
    assert.deepEqual(JSON.parse(ok.stdout).filesChanged, [output(dir)]);
    const refused = spawnSync(process.execPath, [cli, 'migrate', 'engine-functions', dir], { encoding: 'utf8' });
    assert.equal(refused.status, 1);
    assert.match(JSON.parse(refused.stdout).ambiguities.join(' '), /already exists/);
    const forced = spawnSync(process.execPath, [cli, 'migrate', 'engine-functions', dir, '--force'], { encoding: 'utf8' });
    assert.equal(forced.status, 0, forced.stderr);
    assert.deepEqual(JSON.parse(forced.stdout).publication, { action: 'create-or-replace', path: output(dir), priorExistence: 'cannot-be-established-under-concurrency' });
  } finally { cleanup(dir); }
});
