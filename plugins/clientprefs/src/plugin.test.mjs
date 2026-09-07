import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';
import { DatabaseSync } from 'node:sqlite';
const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
function fixture({ host = {}, storage } = {}) {
  const queries = [], mutations = [], writes = [], ready = [], acks = [], pending = new Map();
  let blocked = false;
  const db = {
    execute: async (sql, args) => {
      if (!args) return;
      await new Promise((resolve,reject) => writes.push({sql,args,resolve,reject}));
      storage?.prepare(sql).run(...args);
    },
    query: (sql, args) => sql.startsWith('PRAGMA') ? Promise.resolve([{name:'writer_epoch'},{name:'revision'}]) : new Promise(resolve => queries.push({args,resolve})),
  };
  const context = vm.createContext({ module: { exports: {} }, exports: {}, console: {log(){}}, require: () => ({ Database: { open: async () => db }, Clients: { all: () => [] } }),
    __s2pkg_clients: { _token: c => c.token },
    __s2_cookie_take_retired: () => "[]", __s2_cookie_take_offline_writes: () => [],
    __s2_cookie_session: (...args) => { mutations.push(args); return 'true'; },
    __s2_cookie_account_fence: () => '1', __s2_cookie_fence_done: () => !blocked,
    __s2_cookie_lease: n => { const batch = ready.splice(0,n); for(const r of batch) pending.set(r.leaseId,r); return JSON.stringify(batch); },
    __s2_cookie_ack: (id,rev,success) => { acks.push([id,rev,success]); pending.delete(id); return true; },
    ...host,
  });
  vm.runInContext(code, context);
  return { api: context.module.exports, queries, mutations, writes, ready, acks, pending, block: v => {blocked=v;} };
}
const client = (slot=3,token='99',isValid=()=>true) => ({slot,token,isValid,steamId:'account'});
const row = (n) => ({leaseId:String(n),revision:String(n),writerEpoch:'epoch',steamId:'account'+n,name:'color',value:'v'+n,updated:1});
// Model only the native lease/ACK contract used by this composed pump test. Detailed capacity,
// covers_from, owner-generation and capped-backoff behavior is tested against the Rust outbox.
function retryingHost() {
  let now = 0, nextLease = 0, peakLeased = 0;
  const ready = [], leased = new Map(), acks = [];
  const key = row => `${row.steamId}/${row.name}`;
  function accept(row) {
    const at = ready.findIndex(item => key(item.row) === key(row));
    const pending = { row: { ...row }, due: at < 0 ? now : ready[at].due };
    if (at < 0) ready.push(pending); else ready[at] = pending;
  }
  const natives = {
    __s2_cookie_lease: requested => {
      assert.ok(requested > 0 && requested <= 4 - leased.size, 'pump requests only free operation slots');
      const batch = [], busy = new Set([...leased.values()].map(row => row.steamId));
      for (let i = 0; i < ready.length && batch.length < requested && leased.size < 4;) {
        const item = ready[i];
        if (item.due > now || busy.has(item.row.steamId)) { i++; continue; }
        ready.splice(i, 1);
        const attempt = Object.freeze({ ...item.row, leaseId: String(++nextLease) });
        leased.set(attempt.leaseId, attempt); busy.add(attempt.steamId); batch.push(attempt);
      }
      peakLeased = Math.max(peakLeased, leased.size);
      return JSON.stringify(batch);
    },
    __s2_cookie_ack: (id, revision, success) => {
      const attempt = leased.get(id);
      if (!attempt || attempt.revision !== revision) return false;
      acks.push({ id, revision, success }); leased.delete(id);
      if (!success) {
        const newer = ready.find(item => key(item.row) === key(attempt));
        if (newer) newer.due = now + 100;
        else ready.push({ row: attempt, due: now + 100 });
      }
      return true;
    },
  };
  return { natives, accept, ready, leased, acks, advanceTo: time => { now = time; }, peakLeased: () => peakLeased };
}

test('first/middle failures retry after their deadline and persist the newest in-flight update within lease bounds', async () => {
  const host = retryingHost(), storage = new DatabaseSync(':memory:');
  storage.exec('CREATE TABLE cookies (steamid TEXT,name TEXT,value TEXT,updated INTEGER,writer_epoch TEXT,revision INTEGER,PRIMARY KEY(steamid,name))');
  try {
    const f = fixture({ host: host.natives, storage }); await f.api.OnPluginStart();
    for (let n = 1; n <= 8; n++) host.accept(row(n));
    f.api.OnGameFrame(); await settle();
    assert.equal(f.writes.length, 4); assert.equal(host.leased.size, 4);
    f.api.OnGameFrame(); assert.equal(f.writes.length, 4, 'no submission beyond four active leases');

    // A newer accepted value waits behind the immutable first attempt. Reject the first and
    // middle DB operations; both must remain core-owned until successful retries are ACKed.
    host.accept({ ...row(9), steamId: 'account1', value: 'newest' });
    f.writes[0].reject(new Error('busy first')); f.writes[2].reject(new Error('busy middle'));
    await settle();
    assert.deepEqual(host.acks.map(ack => ack.success), [false, false]);
    assert.equal(host.ready.length, 6); assert.equal(host.leased.size, 2);
    assert.equal(f.writes[0].args[2], 'v1', 'in-flight attempt is never mutated by coalescing');
    f.api.OnGameFrame(); await settle(); assert.equal(f.writes.length, 6);
    for (const i of [1, 3, 4, 5]) f.writes[i].resolve();
    await settle();

    host.advanceTo(99); f.api.OnGameFrame(); await settle();
    assert.equal(f.writes.length, 8, 'unrelated accounts make progress before retries are due');
    f.writes[6].resolve(); f.writes[7].resolve(); await settle();
    f.api.OnGameFrame(); assert.equal(f.writes.length, 8, 'no failed account retries before 100ms');
    assert.equal(host.ready.length, 2); assert.equal(host.leased.size, 0);
    assert.equal(storage.prepare('SELECT count(*) AS n FROM cookies').get().n, 6);
    assert.equal(storage.prepare("SELECT value FROM cookies WHERE steamid='account1'").get(), undefined);

    host.advanceTo(100); f.api.OnGameFrame(); await settle();
    assert.equal(f.writes.length, 10); assert.equal(host.leased.size, 2);
    const retried = f.writes.slice(8).map(write => [write.args[0], write.args[2], write.args[5]]);
    assert.deepEqual(retried, [['account1', 'newest', '9'], ['account3', 'v3', '3']]);
    f.writes[8].resolve(); f.writes[9].resolve(); await settle();
    const stored = storage.prepare('SELECT steamid,value,revision FROM cookies ORDER BY steamid').all();
    assert.deepEqual(stored.map(row => [row.steamid, row.value, row.revision]),
      Array.from({ length: 8 }, (_, i) => [`account${i + 1}`, i === 0 ? 'newest' : `v${i + 1}`, i === 0 ? 9 : i + 1]));
    assert.equal(host.ready.length, 0); assert.equal(host.leased.size, 0);
    assert.equal(host.acks.length, 10); assert.equal(host.acks.filter(ack => ack.success).length, 8);
    assert.equal(new Set(host.acks.map(ack => ack.id)).size, 10, 'retries use fresh leases and each is ACKed once');
    assert.equal(host.peakLeased(), 4);
    f.api.OnGameFrame(); assert.equal(f.writes.length, 10, 'fully ACKed writes are not replayed');
  } finally { storage.close(); }
});
test('fence prevents reconnect SELECT during retry without retaining a JS write queue', async () => {
  const f=fixture(); await f.api.OnPluginStart(); f.block(true);
  const p=f.api.OnClientPutInServer(client()); await settle(); assert.equal(f.queries.length,0);
  f.api.OnGameFrame(); assert.equal(f.queries.length,0); f.block(false); f.api.OnGameFrame(); await settle();
  assert.equal(f.queries.length,1); f.queries[0].resolve([{name:'color',value:'newest',updated:12}]); await p;
  assert.deepEqual(f.mutations[0].slice(0,3),[3,'99','load']);
});
test('delayed SELECT cannot mutate replacement connection', async () => {
  const f=fixture(); await f.api.OnPluginStart(); let live=true;
  const p=f.api.OnClientPutInServer(client(3,'1',()=>live)); await settle(); live=false;
  f.queries[0].resolve([{name:'color',value:'stale',updated:12}]); await p; assert.equal(f.mutations.length,0);
});
test('reads share the four-operation budget and invalid waiters are retired', async () => {
  const f=fixture(); await f.api.OnPluginStart(); let live=true;
  const ps=Array.from({length:8},(_,i)=>f.api.OnClientPutInServer(client(i,String(i),()=>live)));
  await settle(); assert.equal(f.queries.length,4); live=false; f.api.OnGameFrame();
  for(const q of f.queries) q.resolve([]); await Promise.all(ps); assert.equal(f.mutations.length,0);
});
test('actual SQLite conditional upsert rejects old and equal replay after replacement commit, including int64 revisions', async () => {
  const f=fixture(); await f.api.OnPluginStart(); f.ready.push(row(1)); f.api.OnGameFrame(); await settle();
  const sql=f.writes[0].sql; const db=new DatabaseSync(':memory:');
  db.exec('CREATE TABLE cookies (steamid TEXT,name TEXT,value TEXT,updated INTEGER,writer_epoch TEXT,revision INTEGER,PRIMARY KEY(steamid,name))');
  const put=db.prepare(sql);
  put.run('A','k','new',1,'epoch','9007199254740993');
  put.run('A','k','old',1,'epoch','9007199254740992');
  put.run('A','k','equal-replay',1,'epoch','9007199254740993');
  assert.equal(db.prepare('SELECT value FROM cookies').get().value,'new');
  put.run('A','k','next-process',1,'fresh-epoch','1');
  assert.equal(db.prepare('SELECT value FROM cookies').get().value,'next-process'); db.close();
});
