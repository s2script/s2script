import test from 'node:test';
import assert from 'node:assert/strict';
import { installClientHost } from './client-host.mjs';

test('the game host fixture requires an explicit connection and never revives retained clients', () => {
  const ctx = {};
  const host = installClientHost(ctx);
  const { Client, Clients } = ctx.__s2pkg_clients;
  const absent = new Client(3);
  assert.equal(absent.isValid(), false);
  assert.equal(Clients.fromSlot(3), null);
  assert.deepEqual(Clients.all(), []);
  host.connect(3);
  const a = Clients.fromSlot(3);
  assert.equal(a.isValid(), true);
  assert.equal(absent.isValid(), false);
  host.retire(3);
  assert.equal(a.isValid(), false);
  assert.equal(Clients.fromSlot(3), null);
  host.connect(3);
  const b = Clients.fromSlot(3);
  assert.notEqual(b.token, a.token);
  host.replace(3);
  assert.equal(b.isValid(), false);
  assert.equal(Clients.all().length, 1);
});
