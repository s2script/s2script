import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';
import { DatabaseSync } from 'node:sqlite';
const code = transformSync(readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'cjs' }).code;
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
function fixture() {
  const queries = [], mutations = [], writes = [], ready = [], acks = [], pending = new Map();
  let blocked = false;
  const db = {
    execute: async (sql, args) => { if (!args) return; await new Promise((resolve,reject) => writes.push({sql,args,resolve,reject})); },
    query: (sql, args) => sql.startsWith('PRAGMA') ? Promise.resolve([{name:'writer_epoch'},{name:'revision'}]) : new Promise(resolve => queries.push({args,resolve})),
  };
  const context = vm.createContext({ module: { exports: {} }, exports: {}, console: {log(){}}, require: () => ({ Database: { open: async () => db }, Clients: { all: () => [] } }),
    __s2pkg_clients: { _token: c => c.token },
    __s2_cookie_take_retired: () => "[]", __s2_cookie_take_offline_writes: () => [],
    __s2_cookie_session: (...args) => { mutations.push(args); return 'true'; },
    __s2_cookie_account_fence: () => '1', __s2_cookie_fence_done: () => !blocked,
    __s2_cookie_lease: n => { const batch = ready.splice(0,n); for(const r of batch) pending.set(r.leaseId,r); return JSON.stringify(batch); },
    __s2_cookie_ack: (id,rev,success) => { acks.push([id,rev,success]); pending.delete(id); return true; },
  });
  vm.runInContext(code, context);
  return { api: context.module.exports, queries, mutations, writes, ready, acks, pending, block: v => {blocked=v;} };
}
const client = (slot=3,token='99',isValid=()=>true) => ({slot,token,isValid,steamId:'account'});
const row = (n) => ({leaseId:String(n),revision:String(n),writerEpoch:'epoch',steamId:'account'+n,name:'color',value:'v'+n,updated:1});
test('global four-operation cap and first/middle database failures are acknowledged for host retry', async () => {
  const f=fixture(); await f.api.OnPluginStart(); f.ready.push(...Array.from({length:8},(_,i)=>row(i+1)));
  f.api.OnGameFrame(); await settle(); assert.equal(f.writes.length,4); assert.equal(f.ready.length,4);
  f.api.OnGameFrame(); assert.equal(f.writes.length,4);
  f.writes[0].reject(new Error('busy')); f.writes[2].reject(new Error('busy')); await settle();
  assert.deepEqual(f.acks.map(a=>a[2]),[false,false]);
  f.api.OnGameFrame(); await settle(); assert.equal(f.writes.length,6);
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
