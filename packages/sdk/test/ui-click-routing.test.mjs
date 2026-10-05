import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {fileURLToPath, pathToFileURL} from 'node:url';
const framework=process.env.S2SCRIPT_FRAMEWORK_TREE??path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const {cs2AddonBundle}=await import(pathToFileURL(path.join(framework,'packages/sdk/test/cs2-addon.mjs')));
const prelude=fs.readFileSync(path.join(framework,'core/js/prelude.js'),'utf8');
const entityStart=prelude.indexOf('  var K = { I32:');
const entityEnd=prelude.indexOf('  // Entity-I/O slice: hook an entity output');
const clientStart=prelude.indexOf('  var __s2_client_tokens = new WeakMap();');
const clientEnd=prelude.indexOf('  // --- @s2script/sound',clientStart);
assert.ok(entityStart>=0&&entityEnd>entityStart&&clientStart>=0&&clientEnd>clientStart);
const dir=path.join(framework,'games/cs2/js');
const stockFixturePath=path.join(dir,'hudkit-prelude.test.js');
const stock=fs.readFileSync(stockFixturePath,'utf8');
const start=stock.indexOf('function pluginWorld(options = {})');
const end=stock.indexOf('test("legacy and explicit singleton ownership');
assert.ok(start>=0&&end>start);
const uiSource=process.env.S2S_CLICK_BASELINE==='1'
 ?execFileSync('git',['-C',framework,'show','HEAD:games/cs2/js/ui.js'],{encoding:'utf8'})
 :fs.readFileSync(process.env.S2S_UI_SOURCE??path.join(dir,'ui.js'),'utf8');
const pluginWorld=new Function('require','assert','readFileSync','join','__dirname',stock.slice(start,end)+'\nreturn pluginWorld;')(
 createRequire(stockFixturePath),assert,(filename,...args)=>path.basename(filename)==='ui.js'?uiSource:fs.readFileSync(filename,...args),path.join,dir);

// Actual SDK EntityRef + Player + Client code. Only current native state is recorded/faked.
function roster(occupied=8){
 const c={console};vm.createContext(c);const state={occupied,connected:true,controllerLive:true,pawnLive:true,word:true,flags:0,version:1,beforeId:null,beforeHandle:null};
 const calls=[];
 const record=(name,fn)=>(...args)=>{calls.push(name);return fn(...args);};
 const controllerId=index=>1000+index;
 const pawnId=index=>2000+index;
 const live=(index,id)=>index>=1&&index<=64?state.controllerLive&&id===controllerId(index):index>=70&&index<70+occupied&&state.pawnLive&&id===pawnId(index);
 c.__s2_ent_id_for_index=record('entityId',index=>{if(state.beforeId)state.beforeId();return state.controllerLive&&index>=1&&index<=64?controllerId(index):0;});
 c.__s2_ent_ref_valid=record('entityValid',live);
 c.__s2_schema_offset=record('schemaOffset',(_cls,field)=>field==='m_hPlayerPawn'?4:-1);
 c.__s2_ent_ref_read=record('entityRead',(index,id,offset,kind)=>{
  if(state.beforeHandle)state.beforeHandle();
  if(!live(index,id)||index>64||offset!==4)return null;
  const word=index<=occupied&&state.word?100+index-1:0xffffffff;
  return kind===1?word|0:kind===8?word>>>0:null;
 });
 c.__s2_handle_adopt=record('handleAdopt',word=>word>=100&&word<100+occupied&&state.pawnLive?[70+word-100,pawnId(70+word-100)]:null);
 c.__s2_ent_identity_flags=record('identityFlags',(index,id)=>live(index,id)?state.flags:null);
 c.__s2_client_generation=record('clientGeneration',slot=>state.connected&&slot>=0&&slot<occupied?String(state.version):'0');
 c.__s2_client_matches=record('clientMatches',(slot,token)=>state.connected&&slot>=0&&slot<occupied&&token===String(state.version));
 c.__s2_client_steamid=()=> '0';c.__s2_client_subscribe=()=>0;
 c.__s2require=name=>name==='@s2script/sdk/entity'?c.__s2pkg_entity:{};
 vm.runInContext(prelude.slice(entityStart,entityEnd)+';globalThis.__s2pkg_entity={EntityRef};\n'+prelude.slice(clientStart,clientEnd),c);
 vm.runInContext(cs2AddonBundle,c);
 vm.runInContext(`globalThis.guardDefinitions=0;globalThis.guardProperties=0;
 const define=Object.defineProperties;Object.defineProperties=function(obj,descriptors){
  if(Object.prototype.hasOwnProperty.call(descriptors,'isValid')&&Object.prototype.hasOwnProperty.call(descriptors,'readInt32')){guardDefinitions++;guardProperties+=Object.keys(descriptors).length;}
  return define(obj,descriptors);
 };globalThis.playerFactories=0;const originalFromSlot=__s2pkg_cs2.Player.fromSlot;
 __s2pkg_cs2.Player.fromSlot=function(slot){playerFactories++;return originalFromSlot(slot);};`,c);
 function clear(){calls.length=0;c.guardDefinitions=0;c.guardProperties=0;c.playerFactories=0;}
 function summary(){const native={};for(const name of calls)native[name]=(native[name]??0)+1;
  return {factoryCalls:c.playerFactories,players:c.guardDefinitions,guardProperties:c.guardProperties,nativeCalls:calls.length,native};}
 return {state,clear,summary,Player:c.__s2pkg_cs2.Player};
}
function clickCase({contexts=1,occupied=8,index=1,id=1001,configure=()=>{}}={}){
 const w=pluginWorld(),peers=[];
 for(let i=0;i<contexts;i++){
  const p=w.plugin(),r=roster(occupied),events=[];p.ctx.__s2pkg_cs2.Player=r.Player;
  p.rawListeners.push(event=>events.push(event.slot));configure(r,p,w);r.clear();peers.push({p,r,events});
 }
 for(const {p} of peers)for(const fn of p.lifecycle.click)fn({player:index===null?null:{index,id},buttonId:'s2_m0_r1'});
 return {w,peers};
}
test('25 actual SDK context routes resolve only the supplied occupied controller',()=>{
 const {peers}=clickCase({contexts:25});
 assert.equal(peers.reduce((n,x)=>n+x.p.lifecycle.click.length,0),25);
 console.log(JSON.stringify({schema:1,contexts:25,occupied:8,totals:peers.reduce((a,{r})=>{const s=r.summary();for(const k of ['factoryCalls','players','guardProperties','nativeCalls'])a[k]+=s[k];return a;},{factoryCalls:0,players:0,guardProperties:0,nativeCalls:0}),limits:'Actual SDK fixture counts, not native timing. All context subscriptions intentionally unchanged.'}));
 for(const {r,events} of peers){assert.deepEqual(events,[0]);assert.equal(r.summary().factoryCalls,1);assert.equal(r.summary().players,1);assert.equal(r.summary().guardProperties,54);}
});
for(const [name,configure,index,id,slot] of [
 ['same occupied live controller',()=>{},1,1001,0],
 ['last occupied controller',()=>{},8,1008,7],
 ['host identity mismatch',()=>{},1,9999,-1],
 ['pawnless controller',r=>{r.state.word=false;},1,1001,-1],
 ['dangling pawn',r=>{r.state.pawnLive=false;},1,1001,-1],
 ['stale controller',r=>{r.state.controllerLive=false;},1,1001,-1],
 ['staged occupied pawn keeps original occupancy behavior',r=>{r.state.flags=4;},1,1001,0],
 ['null engine clicker',()=>{},null,0,-1],
 ['non-controller index',()=>{},70,2070,-1],
 ['fractional index',()=>{},1.5,1001,-1],
 ['empty preallocated controller',()=>{},64,1064,-1],
 ['connection retired before guarded identity check',r=>{r.state.beforeHandle=()=>{r.state.connected=false;};},1,1001,-1],
])test(name,()=>{const {peers}=clickCase({configure,index,id});assert.deepEqual(peers[0].events,[slot]);});
test('same-SteamID replacement invalidates an already captured guarded controller identity',()=>{
 const r=roster(),p=r.Player.fromSlot(0);assert.ok(p);assert.equal(p.ref.id,1001);
 r.state.version++;assert.equal(p.ref.id,0);
});
test('handler and raw observer order is unchanged for one exact controller',()=>{
 const w=pluginWorld(),p=w.plugin(),r=roster();p.ctx.__s2pkg_cs2.Player=r.Player;
 const order=[];const hud=p.base.kit.layout;
 hud.onClick('local_button',()=>order.push('handler'));
 p.rawListeners.push(event=>order.push('raw:'+event.slot));
 for(const fn of p.lifecycle.click)fn({player:{index:1,id:1001},buttonId:'local_button'});
 assert.deepEqual(order,['handler','raw:0']);
});
