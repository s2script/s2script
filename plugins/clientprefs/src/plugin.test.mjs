import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
function fixture({ blockWrites = false } = {}) {
  const queries = [], mutations = [], events = [], retired = [], writes = [];
  const db = {
    execute: async (sql, args) => { events.push(['write', args]); if (blockWrites && args) await new Promise(resolve => writes.push(resolve)); },
    query: (sql, args) => new Promise(resolve => { queries.push({ args, resolve }); events.push(['read', args]); }),
  };
  const context = vm.createContext({ module: { exports: {} }, exports: {}, console, require: () => ({ Database: { open: async () => db } }),
    __s2pkg_clients: { _token: c => c.token },
    __s2_cookie_load: (...args) => mutations.push(args),
    __s2_cookie_mark_cached: (...args) => mutations.push(args),
    __s2_cookie_dispatch_cached: (...args) => mutations.push(args),
    __s2_cookie_take_retired: () => JSON.stringify(retired.splice(0)),
    __s2_cookie_session: (...args) => { mutations.push(args); return 'true'; },
    __s2_cookie_get_dirty: () => ({}), __s2_cookie_clear: (...args) => mutations.push(args),
    __s2_cookie_take_offline_writes: () => [],
  });
  vm.runInContext(code, context);
  return { api: context.module.exports, queries, mutations, events, retired, writes };
}
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };

test('a query completed after same-account reconnect cannot mutate or notify the replacement cache', async () => {
  const f = fixture(); await f.api.OnPluginStart();
  let live = true;
  const old = { slot: 3, token: '1', steamId: 'same-account', isValid: () => live };
  const pending = f.api.OnClientPutInServer(old);
  await settle();
  assert.equal(f.queries.length, 1);
  live = false;
  f.queries[0].resolve([{ name: 'color', value: 'old', updated: 12 }]);
  await pending;
  assert.equal(f.mutations.length, 0);
});


test('a live client commits its loaded rows and original token', async () => {
  const f = fixture(); await f.api.OnPluginStart();
  const current = { slot: 3, token: '99', steamId: 'account', isValid: () => true };
  const pending = f.api.OnClientPutInServer(current); await settle();
  f.queries[0].resolve([{ name: 'color', value: 'blue', updated: 12 }]); await pending;
  assert.equal(f.mutations.length, 1);
  assert.deepEqual(f.mutations[0].slice(0, 3), [3, '99', 'load']);
  assert.deepEqual(JSON.parse(f.mutations[0][3]), { steamId: 'account', rows: [{ name: 'color', value: 'blue', updated: 12 }] });
});

test('replacement SELECT waits for detached same-account writes even without a disconnect callback', async () => {
  const f = fixture({ blockWrites: true }); await f.api.OnPluginStart();
  f.retired.push(['same-account', 'color', 'saved-red', 12]);
  const pending = f.api.OnClientPutInServer({ slot: 3, token: '2', steamId: 'same-account', isValid: () => true });
  await settle();
  assert.equal(f.writes.length, 1);
  assert.equal(f.queries.length, 0, 'SELECT must wait for the old connection save');
  f.writes[0](); await settle();
  assert.equal(f.queries.length, 1);
  f.queries[0].resolve([{ name: 'color', value: 'saved-red', updated: 12 }]); await pending;
  assert.equal(JSON.parse(f.mutations[0][3]).rows[0].value, 'saved-red');
  f.api.OnClientDisconnect({ slot: 3, token: '1', steamId: 'same-account', isValid: () => false });
  assert.equal(f.mutations.length, 1, 'old disconnect never clears the new cache');
});
