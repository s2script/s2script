import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { typecheckPlugin } from '../src/typecheck/typecheck.ts';
import { buildPlugin } from '../src/build.ts';

const fn = (extra = {}) => ({ target: { module: 'server', pattern: '55 48', validate: { prologue: '55 48' } }, ...extra });
const functions = { canUse: fn({ parameters: [{ name: 'count', type: 'i32', mutable: 'pre' }, { name: 'locked', type: 'bool' }], returns: 'bool', surfaces: ['call', 'pre', 'post'] }), notify: fn({ requirement: 'required', returns: 'void', surfaces: ['pre'] }) };
function fixture(source, data = functions) {
  const dir = mkdtempSync(join(tmpdir(), 's2-ef-types-'));
  mkdirSync(join(dir, 'src'));
  mkdirSync(join(dir, 'gamedata'));
  writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: '@demo/binding', version: '1.0.0', main: 'src/plugin.ts', s2script: {} }));
  writeFileSync(join(dir, 'src/plugin.ts'), source);
  if (data) writeFileSync(join(dir, 'gamedata/functions.jsonc'), JSON.stringify({ schemaVersion: 2, functions: data }));
  return dir;
}
const imports = 'import { Engine } from "@s2script/sdk/unsafe"; import { HookResult } from "@s2script/sdk/events";\n';
const valid = imports + `
const optional = Engine.function('canUse');
if (optional.available) {
  const result: boolean = optional.call(1, true);
  optional.onPre(view => { view.count = 2; return { action: HookResult.Handled, returnValue: true }; });
  optional.onPre({ observeOnly: true }, view => { const n: number = view.count; void n; });
  optional.onPost(view => { const result: boolean = view.returnValue; void result; });
  void result;
} else { const reason: string | null = optional.status.reason; void reason; }
Engine.function('notify').onPre(() => HookResult.Handled);
export function OnPluginStart(): void {}
`;

test('standalone typecheck sees generated bindings on a clean checkout and does not emit', () => {
  const dir = fixture(valid);
  const result = typecheckPlugin(dir);
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
  assert.equal(existsSync(join(dir, '.s2script', 'engine-functions.d.ts')), false);
});

test('invalid calls, readonly views, bare suppression, unsupported surfaces, and unknown names get TS diagnostics', () => {
  const invalid = imports + `
const f = Engine.function('canUse');
if (f.available) {
  f.call('wrong', true);
  f.onPre(v => { v.locked = false; return HookResult.Handled; });
  f.onPre({ observeOnly: true }, v => { v.count = 3; return HookResult.Changed; });
  f.onPost(v => { v.count = 4; v.returnValue = false; });
}
Engine.function('notify').call();
Engine.function('missing');
export function OnPluginStart(): void {}
`;
  const result = typecheckPlugin(fixture(invalid));
  assert.equal(result.ok, false);
  assert.ok(result.diagnostics.length >= 6, JSON.stringify(result.diagnostics));
  assert.ok(result.diagnostics.every(d => d.code < 90000), JSON.stringify(result.diagnostics));
  for (const code of [2339, 2345, 2540]) assert.ok(result.diagnostics.some(d => d.code === code), `missing TS${code}: ${JSON.stringify(result.diagnostics)}`);
});

test('standalone typecheck overrides stale declarations when a function changes or disappears', async () => {
  const dir = fixture(valid);
  await buildPlugin(dir);
  const generated = join(dir, '.s2script', 'engine-functions.d.ts');
  assert.ok(existsSync(generated));
  assert.match(readFileSync(generated, 'utf8'), /interface CanUsePreView/);
  assert.match(readFileSync(generated, 'utf8'), /interface CanUsePostView/);
  assert.match(readFileSync(generated, 'utf8'), /interface CanUseAvailableBinding/);
  writeFileSync(join(dir, 'gamedata', 'functions.jsonc'), JSON.stringify({ schemaVersion: 2, functions: { notify: functions.notify } }));
  assert.equal(typecheckPlugin(dir).ok, false);
  rmSync(join(dir, 'gamedata', 'functions.jsonc'));
  assert.equal(typecheckPlugin(dir).ok, false);
  assert.ok(readFileSync(generated, 'utf8').includes('canUse'), 'standalone check must not edit the stale editor artifact');
});

test('reserved names are rejected before declaration emission', () => {
  const dir = fixture('export function OnPluginStart(): void {}', { self: fn() });
  assert.throws(() => typecheckPlugin(dir), /reserved/);
  const reservedParameter = fixture('export function OnPluginStart(): void {}', { safe: fn({ parameters: [{ name: 'returnValue', type: 'i32' }] }) });
  assert.throws(() => typecheckPlugin(reservedParameter), /reserved/);
});

test('observe-only PRE rejects an action return without another type error', () => {
  for (const action of ['Handled', 'Stop', 'Changed']) {
    const dir = fixture(imports + `Engine.function('notify').onPre({ observeOnly: true }, () => HookResult.${action});\nexport function OnPluginStart(): void {}`);
    const result = typecheckPlugin(dir);
    assert.equal(result.ok, false, `observe-only accepted ${action}`);
    assert.ok(result.diagnostics.some(d => d.file.endsWith('plugin.ts')), JSON.stringify(result.diagnostics));
  }
});

test('observe-only PRE accepts ordinary void, undefined, and console.log expression callbacks', () => {
  const source = imports + `
const f = Engine.function('notify');
f.onPre({ observeOnly: true }, () => {});
f.onPre({ observeOnly: true }, () => undefined);
f.onPre({ observeOnly: true }, () => console.log('observed'));
export function OnPluginStart(): void {}
`;
  const result = typecheckPlugin(fixture(source));
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
});

test('accepted authored this/default parameter names keep call arity and view property names', () => {
  const special = { named: fn({ requirement: 'required', parameters: [
    { name: 'this', type: 'i32', mutable: 'pre' },
    { name: 'default', type: 'i32' },
  ], returns: 'void', surfaces: ['call', 'pre', 'post'] }) };
  const source = imports + `
const f = Engine.function('named');
f.call(1, 2);
f.onPre(view => { view.this = 3; const fixed: number = view.default; void fixed; });
f.onPost(view => { const a: number = view.this; const b: number = view.default; void a; void b; });
export function OnPluginStart(): void {}
`;
  const result = typecheckPlugin(fixture(source, special));
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
  const wrongArity = typecheckPlugin(fixture(source.replace('f.call(1, 2)', 'f.call(1)'), special));
  assert.ok(wrongArity.diagnostics.some(d => d.code === 2554 && d.file.endsWith('plugin.ts')), JSON.stringify(wrongArity.diagnostics));
});

test('suppression:none PRE types reject Handled, Stop, and typed suppression while allowing edits', () => {
  const data = { quiet: fn({ requirement: 'required', parameters: [{ name: 'count', type: 'i32', mutable: 'pre' }], returns: 'i32', surfaces: ['pre', 'post'], suppression: 'none' }) };
  const source = imports + `
const f = Engine.function('quiet');
f.onPre(v => { v.count = 4; return HookResult.Changed; });
f.onPre(() => HookResult.Handled);
f.onPre(() => HookResult.Stop);
f.onPre(() => ({ action: HookResult.Handled, returnValue: 5 }));
f.onPost(v => { const n: number = v.returnValue; void n; });
export function OnPluginStart(): void {}
`;
  const result = typecheckPlugin(fixture(source, data));
  assert.equal(result.ok, false);
  assert.equal(result.diagnostics.filter(d => d.file.endsWith('plugin.ts')).length, 3, JSON.stringify(result.diagnostics));
});

test('borrowed copied return retains typed PRE suppression', () => {
  const data = { borrowed: fn({ requirement: 'required', returns: { type: 'string', ownership: 'caller-borrowed' }, surfaces: ['pre', 'post'] }) };
  const source = imports + `
Engine.function('borrowed').onPre(() => ({ action: HookResult.Handled, returnValue: 'held' }));
Engine.function('borrowed').onPost(v => { const s: string = v.returnValue; void s; });
export function OnPluginStart(): void {}
`;
  const result = typecheckPlugin(fixture(source, data));
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
});
