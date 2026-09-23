// Control-flow tests of actual fixture sources, not native/provider/live evidence.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
const token='b'.repeat(64);
function world() {
  const connections=new Map(), commands=new Map(), queued=[], kicked=[], records=[], subscriptions=[];
  let serial=0, now=1000, items=0;
  const cvars=new Map(Object.entries({bot_quota:'1',bot_quota_mode:'normal',bot_join_after_player:'1',mp_limitteams:'2'}));
  class Client {
    constructor(slot) { this.slot=slot; this.serial=connections.get(slot)?.serial; }
    isValid() { return this.serial!==undefined && connections.get(this.slot)?.serial===this.serial; }
    get userId(){return this.isValid()?connections.get(this.slot).userId:-1;}
    get isBot(){return this.isValid() && connections.get(this.slot).bot;}
    get ip(){return this.isValid()?connections.get(this.slot).ip:'';}
    get signonState(){return this.isValid()?6:-1;}
    get name(){return this.isValid()?connections.get(this.slot).name:'';}
    kick(){if(this.isValid()){kicked.push(this.userId);connections.delete(this.slot);}}
  }
  const Clients={all:()=>[...connections.keys()].map(slot=>new Client(slot)),fromSlot:slot=>connections.has(slot)?new Client(slot):null};
  const Player={fromSlot:slot=>{const client=Clients.fromSlot(slot);return client?{userId:client.userId,pawn:{isValid:true,ref:{index:slot+100},giveNamedItem(){items++;return {};},removeWeapon(){}}}:null;}};
  const sdk={Clients,Server:{getCvar:n=>cvars.get(n)??'',setCvar(n,v){cvars.set(n,v);return true;},registerCvar(n,o){if(!cvars.has(n))cvars.set(n,String(o.default));return true;},command:c=>queued.push(c)},command:(name,fn)=>commands.set(name,fn)};
  function load(name){
    const source=fs.readFileSync(new URL(`../../examples/engine-function-acceptance/${name==='B'?'witness/':''}src/plugin.ts`,import.meta.url),'utf8');
    const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
    const exports={}; vm.runInNewContext(code,{exports,Date:{now:()=>now},console:{log:s=>records.push(JSON.parse(s))},require:n=>n==='@s2script/sdk'?sdk:n==='@s2script/cs2'?{Player,items:{onCanAcquire:f=>subscriptions.push(f),onCanAcquirePost:f=>subscriptions.push(f)}}:{REVISION:'a'.repeat(40),TOKEN:token}});exports.OnPluginStart();return exports;
  }
  return {connections,cvars,queued,kicked,records,load,subscriptions,get items(){return items;},tick:()=>now+=31000,
    add(slot,userId,bot=true){connections.set(slot,{serial:++serial,userId,bot,ip:bot?'':'127.0.0.1',name:bot?'Slingshot':'Human'});},
    cmd(name,args){commands.get(name)({callerSlot:-1,arg:i=>args[i]??'',reply(){}});}};
}
function arm(w,g=1){w.cmd('s2_engine_accept',['arm','run']);w.cmd('s2_engine_witness',['arm','run',String(g),token]);}
function begin(){const w=world();w.add(0,10);const a=w.load('A'),b=w.load('B');arm(w);w.cmd('s2_engine_witness',['create','run','1',token]);return {w,a,b};}
function poll(w,g=1){w.cmd('s2_engine_witness',['poll','run',String(g),token]);}
function cleanup(w,g=1){w.cmd('s2_engine_witness',['cleanup','run',String(g),token]);}
test('failed creation never kicks a baseline bot or uses a name selector',()=>{const {w}=begin();assert.equal(w.queued.filter(c=>c==='bot_add_ct').length,1);poll(w);w.tick();poll(w);cleanup(w);assert.deepEqual(w.kicked,[]);assert.ok(!w.queued.some(c=>c.startsWith('bot_kick')));assert.equal(w.cvars.get('bot_join_after_player'),'1');});
test('one guarded bot survives A reload and alone is cleaned up',()=>{const {w,a,b}=begin();w.add(1,11,false); // concurrent human makes adoption ambiguous
 poll(w);cleanup(w);assert.deepEqual(w.kicked,[]);
 const happy=begin(),x=happy.w;x.add(1,12);poll(x);x.cmd('s2_engine_accept',['capture','run']);poll(x);x.cmd('s2_engine_accept',['acquire','run']);assert.equal(x.items,1);happy.a.OnPluginEnd();x.load('A');arm(x,2);poll(x,2);x.cmd('s2_engine_accept',['capture','run']);poll(x,2);x.cmd('s2_engine_accept',['acquire','run']);assert.equal(x.items,2);happy.b.OnPluginEnd();assert.deepEqual(x.kicked,[12]);assert.equal(x.connections.get(0).userId,10);assert.equal(x.cvars.get('mp_limitteams'),'2');});
test('ambiguous bots, replaced baseline, and stale owned handles cannot mutate clients',()=>{for(const scenario of ['two','baseline','owned']){const {w,b}=begin();w.add(1,12);if(scenario==='two')w.add(2,13);if(scenario==='baseline')w.add(0,99,false);poll(w);if(scenario==='owned')w.add(1,100,false);b.OnPluginEnd();assert.deepEqual(w.kicked,[]);}});

test('A never acquires on a reused slot after capture even with reused userid',()=>{const {w}=begin();w.add(1,12);poll(w);w.cmd('s2_engine_accept',['capture','run']);poll(w);w.add(1,12);w.cmd('s2_engine_accept',['acquire','run']);assert.equal(w.items,0);cleanup(w);assert.deepEqual(w.kicked,[]);});

test('existing humans remain baseline and an unauthenticated network client is never adopted',()=>{
 const w=world();w.add(0,10);w.add(2,20,false);w.load('A');w.load('B');arm(w);w.cmd('s2_engine_witness',['create','run','1',token]);w.add(1,12);poll(w);cleanup(w);assert.deepEqual(w.kicked,[12]);assert.equal(w.connections.get(2).userId,20);
 const bad=begin().w;bad.add(1,30);bad.connections.get(1).ip='127.0.0.1';poll(bad);cleanup(bad);assert.deepEqual(bad.kicked,[]);
});
test('cleanup never adopts a client if creation is still queued or was never claimed',()=>{const {w,b}=begin();b.OnPluginEnd();assert.deepEqual(w.kicked,[]);assert.ok(w.records.some(r=>r.event==='bot-cleanup-refused'));const later=begin().w;later.add(1,12);cleanup(later);assert.deepEqual(later.kicked,[]);});
