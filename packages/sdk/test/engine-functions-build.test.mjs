import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { unzipSync } from 'fflate';
import { buildPlugin } from '../src/build.ts';
import { inspectArchive } from '../src/commands/inspect.ts';

const functions = { ping: { target: { module: 'server', pattern: '55 48', validate: { prologue: '55 48' } }, parameters: [{ name: 'count', type: 'i32', mutable: 'pre' }], returns: 'bool', surfaces: ['call', 'pre', 'post'] } };
function fixture(permissions) {
  const dir = mkdtempSync(join(tmpdir(), 's2-ef-build-'));
  mkdirSync(join(dir, 'src')); mkdirSync(join(dir, 'gamedata'));
  writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: '@demo/build', version: '1.0.0', main: 'src/plugin.ts', s2script: permissions ? { permissions } : {} }));
  writeFileSync(join(dir, 'src/plugin.ts'), 'export function OnPluginStart(): void {}');
  writeFileSync(join(dir, 'gamedata/functions.jsonc'), JSON.stringify({ schemaVersion: 2, functions }));
  return dir;
}
const unpack = path => Object.fromEntries(Object.entries(unzipSync(readFileSync(path))).map(([k, v]) => [k, Buffer.from(v).toString('utf8')]));

test('auto-discovers the exact file, emits declarations and packs normalized contract with derived manifest', async () => {
  const dir = fixture();
  const first = unpack(await buildPlugin(dir));
  const manifest = JSON.parse(first['manifest.json']);
  const archive = JSON.parse(first['engine-functions.json']);
  assert.deepEqual(manifest.permissions, ['engine:calls', 'engine:hooks']);
  assert.equal(manifest.engineFunctions.bundleHash, archive.bundleHash);
  assert.deepEqual(manifest.engineFunctions.functions.map(f => f.canonicalId), ['@demo/build::ping']);
  assert.equal(archive.functions[0].contractHash.length, 64);
  assert.equal(archive.functions[0].policy.surfaces.join(','), 'call,pre,post');
  assert.deepEqual(Object.keys(archive), ['bundleHash', 'functions', 'ownerId', 'schemaVersion']);
  assert.ok(existsSync(join(dir, '.s2script', 'engine-functions.d.ts')));
  assert.equal(unpack(await buildPlugin(dir))['engine-functions.json'], first['engine-functions.json']);
  assert.match(inspectArchive(join(dir, 'dist', '_demo_build.s2sp')), /@demo\/build::ping/);
});

test('duplicate authored engine permission gets an actionable removal error', async () => {
  await assert.rejects(() => buildPlugin(fixture(['engine:calls'])), /remove.*engine:calls/i);
});

test('v1 permissions remain accepted when a v1 descriptor also needs them', async () => {
  const dir = fixture(['engine:calls']);
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
  pkg.s2script.gamedata = 'gamedata/build.gamedata.jsonc';
  writeFileSync(join(dir, 'package.json'), JSON.stringify(pkg));
  writeFileSync(join(dir, 'gamedata', 'build.gamedata.jsonc'), JSON.stringify({
    signatures: { Old: { linuxsteamrt64: { module: 'libserver.so', pattern: '55 48', resolve: 'direct' } } },
    calls: { old: { receiver: { kind: 'none' }, target: { kind: 'signature', name: 'Old' }, args: [], returns: 'void' } },
  }));
  const members = unpack(await buildPlugin(dir));
  assert.deepEqual(JSON.parse(members['manifest.json']).permissions, ['engine:calls', 'engine:hooks']);
  assert.ok(members['gamedata.json']);
  assert.ok(members['engine-functions.json']);
});

test('only gamedata/functions.jsonc is discovered', async () => {
  const dir = fixture();
  rmSync(join(dir, 'gamedata', 'functions.jsonc'));
  writeFileSync(join(dir, 'gamedata', 'build.functions.jsonc'), JSON.stringify({ schemaVersion: 2, functions }));
  assert.equal(unpack(await buildPlugin(dir))['engine-functions.json'], undefined);
});

test('removing functions deletes editor declarations and archive capability', async () => {
  const dir = fixture();
  await buildPlugin(dir);
  rmSync(join(dir, 'gamedata', 'functions.jsonc'));
  const members = unpack(await buildPlugin(dir));
  assert.equal(members['engine-functions.json'], undefined);
  assert.equal(JSON.parse(members['manifest.json']).engineFunctions, undefined);
  assert.equal(existsSync(join(dir, '.s2script', 'engine-functions.d.ts')), false);
});

test('changing a function refreshes the editor declaration and packed hash', async () => {
  const dir = fixture();
  const first = JSON.parse(unpack(await buildPlugin(dir))['engine-functions.json']);
  const changed = structuredClone(functions);
  changed.ping.parameters[0].name = 'score';
  writeFileSync(join(dir, 'gamedata', 'functions.jsonc'), JSON.stringify({ schemaVersion: 2, functions: changed }));
  const second = JSON.parse(unpack(await buildPlugin(dir))['engine-functions.json']);
  assert.notEqual(second.bundleHash, first.bundleHash);
  assert.match(readFileSync(join(dir, '.s2script', 'engine-functions.d.ts'), 'utf8'), /score: number/);
});
