import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { unzipSync } from 'fflate';
import { buildPlugin } from '../src/build.ts';
import { extractInlineFunctions } from '../src/engine-functions/inline.ts';

const ping = `{ target: { module: 'server', pattern: '55 48', validate: { prologue: '55 48' } }, parameters: [{ name: 'base', type: 'ptr' }], returns: 'ptr', surfaces: ['call', 'pre'] }`;
function plugin(source, functionsFile) {
  const dir = mkdtempSync(join(tmpdir(), 's2-ef-inline-'));
  mkdirSync(join(dir, 'src')); mkdirSync(join(dir, 'gamedata'));
  writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: '@demo/inline', version: '1.0.0', main: 'src/plugin.ts', s2script: {} }));
  writeFileSync(join(dir, 'src/plugin.ts'), source);
  if (functionsFile) writeFileSync(join(dir, 'gamedata/functions.jsonc'), JSON.stringify(functionsFile));
  return dir;
}
const unpack = path => Object.fromEntries(Object.entries(unzipSync(readFileSync(path))).map(([k, v]) => [k, Buffer.from(v).toString('utf8')]));

test('an inline declaration is lifted into the bundle, the manifest and the generated types', async () => {
  const dir = plugin(`import { Engine, Ptr } from "@s2script/sdk/unsafe";
export function OnPluginStart(): void {
  const ping = Engine.fromSignature("ping", ${ping});
  if (ping.available) { const r: Ptr | null = ping.call(Ptr.from(0x20000)); void r; }
}`);
  const members = unpack(await buildPlugin(dir));
  const manifest = JSON.parse(members['manifest.json']);
  const archive = JSON.parse(members['engine-functions.json']);
  assert.deepEqual(manifest.engineFunctions.functions.map(f => f.canonicalId), ['@demo/inline::ping']);
  assert.deepEqual(manifest.permissions, ['engine:calls', 'engine:hooks']);
  assert.equal(archive.functions[0].abi.parameters[0].projection.id, 'ptr');
  assert.equal(archive.functions[0].abi.returns.native, 'ptr');
  assert.match(readFileSync(join(dir, '.s2script', 'engine-functions.d.ts'), 'utf8'), /call\(arg0_base: Ptr \| null\): Ptr \| null;/);
});

test('inline and file declarations merge; aliases and namespace imports are found', () => {
  const dir = plugin(`import * as unsafe from "@s2script/sdk/unsafe";
import { Engine as E } from "@s2script/sdk/unsafe";
unsafe.Engine.fromVirtual("slot", { target: { module: 'server', class: 'CBaseEntity', index: 12, validate: { prologue: '55' } }, surfaces: ['call'] } as const);
E.fromSignature("ping", ${ping});`);
  const found = extractInlineFunctions(dir);
  assert.deepEqual(Object.keys(found.functions).sort(), ['ping', 'slot']);
  assert.equal(found.functions.slot.target.kind, 'virtual');
  assert.match(found.sites.slot, /^src\/plugin\.ts:3$/);
});

for (const [name, source, error] of [
  ['a computed declaration', `import { Engine } from "@s2script/sdk/unsafe"; const d = ${ping}; Engine.fromSignature("ping", d);`, /src\/plugin\.ts:1: inline engine function declarations must be literals/],
  ['a computed name', `import { Engine } from "@s2script/sdk/unsafe"; const n = "ping"; Engine.fromSignature(n, ${ping});`, /must be literals/],
  ['a spread', `import { Engine } from "@s2script/sdk/unsafe"; const t = {}; Engine.fromSignature("ping", { ...t });`, /no spreads/],
  ['an uncalled reference', `import { Engine } from "@s2script/sdk/unsafe"; const f = Engine.fromSignature;`, /must be called directly/],
  ['a duplicate inline name', `import { Engine } from "@s2script/sdk/unsafe"; Engine.fromSignature("ping", ${ping}); Engine.fromSignature("ping", ${ping});`, /already declared inline at src\/plugin\.ts:1/],
  ['a virtual target via fromSignature', `import { Engine } from "@s2script/sdk/unsafe"; Engine.fromSignature("v", { target: { kind: 'virtual', module: 's', class: 'C', index: 1, validate: {} } });`, /use Engine\.fromVirtual/],
]) {
  test(`${name} fails the build with its location`, async () => {
    await assert.rejects(() => buildPlugin(plugin(source)), error);
  });
}

test('a name declared inline and in gamedata/functions.jsonc fails', async () => {
  const file = { schemaVersion: 2, functions: { ping: { target: { module: 'server', pattern: '55', validate: { prologue: '55' } }, surfaces: ['call'] } } };
  await assert.rejects(() => buildPlugin(plugin(`import { Engine } from "@s2script/sdk/unsafe"; Engine.fromSignature("ping", ${ping});`, file)),
    /declared both inline and in gamedata\/functions\.jsonc/);
});

test('an invalid inline declaration is reported at its call site', async () => {
  await assert.rejects(() => buildPlugin(plugin(`import { Engine } from "@s2script/sdk/unsafe";
Engine.fromSignature("ping", { target: { module: 'server', pattern: '55', validate: { prologue: '55' } }, parameters: [{ name: 'x', type: 'pointer' }] });`)),
    /src\/plugin\.ts:2: function "ping".*unsupported author type.*\(declared inline\)/);
});

test('an unrelated fromSignature is ignored', () => {
  const dir = plugin(`const Engine = { fromSignature: (n: string, d: unknown) => d }; Engine.fromSignature("x", 1);`);
  assert.deepEqual(extractInlineFunctions(dir).functions, {});
});
