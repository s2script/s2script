import {test} from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import vm from 'node:vm';
import {resolve} from 'node:path';

async function evaluate(name, sdk, globals = {}) {
  const out = await build({entryPoints: [resolve(`tools/interop-acceptance/plugins/${name}/src/plugin.ts`)], bundle: true, platform: 'node', format: 'cjs', write: false, external: ['@s2script/*']});
  const module = {exports: {}};
  const context = vm.createContext({module, exports: module.exports, require: () => sdk, console, ...globals});
  vm.runInContext(out.outputFiles[0].text, context);
  context.module.exports.OnPluginStart?.();
  return context;
}
test('both providers preserve original inputs, select qualified forward names and reject malformed emit probes', async () => {
  for (const [name, identity, input, expected] of [
    ['numeric', '@interop/numeric', {value: 1, mode: 'normal'}, {value: 12, mode: 'normal'}],
    ['text', '@interop/text', {text: 'seed', mode: 'normal'}, {text: 'seed!!', mode: 'normal'}],
  ]) {
    let methods; const emitted = [];
    await evaluate(name, {publish(id, implementation) {
      assert.equal(id, identity); methods = implementation;
      return {emit(event, payload) { emitted.push({event, payload: structuredClone(payload)}); payload.mode = 'mutated'; },
        dispatch(event, payload) { assert.deepEqual(JSON.parse(JSON.stringify(payload)), input); return event === 'OnRequest' ? 2 : {result: 1, payload: expected}; }};
    }}, {__s2_iface_emit() {throw Error('InterfaceWireError');}});
    const report = methods.probe('normal');
    assert.equal(report.action, 2); assert.equal(report.result, 1);
    assert.deepEqual(JSON.parse(report.final), expected);
    assert.deepEqual(JSON.parse(report.original), input);
    assert.equal(emitted[0].event, 'OnSignal');
    assert.equal(methods.malformed(), 3);
  }
});
test('named bindings retain separate payloads and dispose both whole maps idempotently', async () => {
  const bindings = new Map(); const commands = new Map(); let disposed = 0;
  await evaluate('named', {HookResult: {Continue: 0, Changed: 1, Handled: 2, Stop: 3},
    bindForwards(id, handlers) {bindings.set(id, handlers); let done = false; return {dispose() {if (!done) disposed++; done = true;}};},
    command: {server(name, fn) {commands.set(name, fn);}},
  });
  const number = bindings.get('@interop/numeric'); const text = bindings.get('@interop/text');
  assert.deepEqual(JSON.parse(JSON.stringify(number.OnFormat({value: 1, mode: 'normal'}))), {result: 1, patch: {value: 2}});
  assert.deepEqual(JSON.parse(JSON.stringify(text.OnFormat({text: 'seed', mode: 'normal'}))), {result: 1, patch: {text: 'seed!'}});
  assert.equal(number.OnRequest({value: 1, mode: 'stop'}), 3);
  commands.get('s2_interop_dispose')({reply() {}}); commands.get('s2_interop_dispose')({reply() {}});
  assert.equal(disposed, 2);
});
test('controller service probe restores policy and bans even when a provider throws', async () => {
  for (const throwing of [false, true]) {
    let muted = false, gagged = false, banned = false;
    const commands = new Map(), listeners = new Map();
    const on = (name, fn) => {listeners.set(name, fn); return {dispose() {}};};
    const comm = {on, isMuted: () => muted, isGagged: () => gagged,
      setMuted(id, state) {if (id === '0') return false; muted = state; listeners.get('OnClientMuteChanged')({steamId:id,state}); return true;},
      setGagged(id, state) {gagged = state; listeners.get('OnClientGagChanged')({steamId:id,state}); return true;}};
    const bans = {on, ban(request) {
      if (request.steamId === '0') return {recorded:false,result:0};
      if (throwing) throw Error('provider failure');
      listeners.get('OnBanRequested')(request); banned = true;
      listeners.get('OnBanRecorded')({request,until:0}); return {recorded:true,result:0};
    }, unban(request) {banned = false; listeners.get('OnBanRemoved')(request); return true;}};
    await evaluate('controller', {command: {server(name, fn) {commands.set(name, fn);}},
      HookResult: {Continue:0,Changed:1,Handled:2}, Bans: {get: () => banned ? {} : null}, Clients: {all: () => []},
      watchOptional(name, attach) {if (name === '@s2script/basecomm') attach(comm, {own: x => x}); if (name === '@s2script/basebans') attach(bans, {own: x => x});},
    });
    let reply; commands.get('s2_interop_services')({reply(value) {reply = JSON.parse(value);}});
    assert.equal(muted, false); assert.equal(gagged, false); assert.equal(banned, false);
    if (throwing) assert.match(reply.error, /provider failure/);
    else {assert.equal(reply.cleaned,true); assert.deepEqual(reply.events,{mute:2,gag:2,request:1,recorded:1,removed:1});}
  }
});
