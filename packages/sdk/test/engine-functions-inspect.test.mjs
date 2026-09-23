import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { zipSync } from 'fflate';
import { inspectArchive } from '../src/commands/inspect.ts';
import { find } from '../src/commands/index.ts';

test('inspect renders contract identity, requirement, surfaces, risks, and permissions without loading', () => {
  const dir = mkdtempSync(join(tmpdir(), 's2-ef-inspect-'));
  const archive = join(dir, 'test.s2sp');
  writeFileSync(archive, zipSync({
    'manifest.json': Buffer.from(JSON.stringify({ id: '@demo/x', permissions: ['engine:calls', 'engine:hooks'], engineFunctions: { schemaVersion: 2, bundleHash: 'abc', functions: [{ canonicalId: '@demo/x::ping', contractHash: 'def', requirement: 'optional', surfaces: ['call', 'pre'], mutates: true, suppresses: true }] } })),
    'engine-functions.json': Buffer.from(JSON.stringify({ schemaVersion: 2, bundleHash: 'abc', ownerId: '@demo/x', functions: [{ canonicalId: '@demo/x::ping', contractHash: 'def', requirement: 'optional', policy: { surfaces: ['call', 'pre'], suppression: 'generic' }, abi: { parameters: [{ mutable: ['pre'] }] } }] })),
  }));
  const text = inspectArchive(archive);
  for (const value of ['@demo/x::ping', 'def', 'optional', 'call', 'pre', 'mutation', 'suppression', 'engine:calls', 'engine:hooks']) assert.match(text, new RegExp(value));
  assert.equal(find('inspect')?.name, 'inspect');
});

test('inspect rejects a summary that disagrees with the packed contract', () => {
  const dir = mkdtempSync(join(tmpdir(), 's2-ef-inspect-'));
  const archive = join(dir, 'tampered.s2sp');
  writeFileSync(archive, zipSync({
    'manifest.json': Buffer.from(JSON.stringify({ id: '@demo/x', engineFunctions: { schemaVersion: 2, bundleHash: 'abc', functions: [{ canonicalId: '@demo/x::fake', contractHash: 'def', requirement: 'required', surfaces: ['call'], mutates: false, suppresses: false }] } })),
    'engine-functions.json': Buffer.from(JSON.stringify({ schemaVersion: 2, bundleHash: 'abc', ownerId: '@demo/x', functions: [] })),
  }));
  assert.throws(() => inspectArchive(archive), /summary mismatch/);
});
