import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { cs2AddonBundle } from './cs2-addon.mjs';
import { installClientHost } from './client-host.mjs';

const prelude = readFileSync(new URL('../../../core/js/prelude.js', import.meta.url), 'utf8');
const entitySource = prelude.slice(prelude.indexOf('  var K = { I32:'), prelude.indexOf('  // Entity-I/O slice: hook an entity output'));
function fixture() {
  const calls = [];
  const ctx = { console };
  for (const name of entitySource.match(/__s2_\w+/g)) {
    ctx[name] = (...args) => {
      // Model native argument conversion before recording the engine action. id was already
      // captured by the JS caller, which is precisely the lifetime race under test.
      for (const value of args.slice(2)) {
        if (Array.isArray(value)) value.forEach(item => { if (typeof item === 'object') Number(item); });
        else if (value && typeof value === 'object') Number(value);
      }
      if (args[1] !== 7) return false;
      calls.push({ name, args }); return true;
    };
  }
  ctx.__s2_ent_ref_valid = (_index, id) => id === 7;
  ctx.__s2_ent_id_for_index = () => 7;
  ctx.__s2_schema_offset = () => 8;
  const host = installClientHost(ctx, [3]);
  vm.createContext(ctx);
  vm.runInContext(entitySource + '; globalThis.__s2pkg_entity = { EntityRef };', ctx);
  ctx.__s2require = name => name.endsWith('/entity') ? ctx.__s2pkg_entity : {};
  vm.runInContext(cs2AddonBundle, ctx);
  const player = ctx.__s2pkg_cs2.Player._fromSlotUnchecked(3);
  calls.length = 0;
  return { ctx, host, player, calls };
}

test('the Player facade covers every real EntityRef method and all retained methods fail closed', () => {
  const f = fixture();
  const ref = f.player.ref;
  const special = {
    spawn: [{ targetname: 'controller' }], teleport: [[1, 2, 3]], applyAbsVelocityImpulse: [[1, 2, 3]],
    writeString: [8, 16, 'value'], setModel: ['model'], stopSound: ['sound'],
    setBodyGroupByName: ['group', 1], acceptInput: ['Use', '', null, null, 0],
  };
  const methods = Object.getOwnPropertyNames(f.ctx.EntityRef.prototype)
    .filter(name => name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(f.ctx.EntityRef.prototype, name).value === 'function');
  assert.ok(methods.length > 45, 'inventory uses the full shipped EntityRef prototype');
  for (const method of methods) {
    const args = special[method] || (method.endsWith('Via') || method === 'readFloatsChain' || method === 'readHandleVector' ? [[8], 8, 3] : [8, 3]);
    assert.ok(Object.hasOwn(ref, method), `${method} must have an explicit connection boundary`);
    assert.doesNotThrow(() => ref[method](...args), `${method} needs a supported normalization plan`);
  }
  f.host.replace(3); f.calls.length = 0;
  for (const method of methods) assert.doesNotThrow(() => ref[method]());
  assert.deepEqual(f.calls, [], 'no retained method may reach an engine action');
});

for (const [method, args] of [
  ['setModel', trip => [{ toString() { trip(); return 'model'; } }]],
  ['teleport', trip => [{ get 0() { trip(); return 1; }, 1: 2, 2: 3 }]],
  ['acceptInput', trip => ['Use', '', { get index() { trip(); return 9; }, id: 7 }]],
  ['setGravityScale', trip => [{ valueOf() { trip(); return 1; } }]],
  ['applyAbsVelocityImpulse', trip => [[{ valueOf() { trip(); return 1; } }, 2, 3]]],
  ['stopSound', trip => [{ toString() { trip(); return 'sound'; } }]],
  ['setBodyGroupByName', trip => ['group', { valueOf() { trip(); return 1; } }]],
  ['setModelScale', trip => [{ valueOf() { trip(); return 1; } }]],
  ['writeBool', trip => [{ valueOf() { trip(); return 8; } }, true]],
  ['writeBoolVia', trip => [[{ valueOf() { trip(); return 8; } }], 8, true]],
  ['notifyStateChanged', trip => [{ valueOf() { trip(); return 8; } }]],
  ['clearIdentityFlags', trip => [{ valueOf() { trip(); return 1; } }]],
  ['spawn', trip => [{ get name() { trip(); return 'A'; } }]],
]) {
  test(`Player.ref.${method} finishes input access before the connection check`, () => {
    const f = fixture();
    f.player.ref[method](...args(() => {}));
    assert.equal(f.calls.length, 1, 'current connection must reach the native');
    f.calls.length = 0;
    f.player.ref[method](...args(() => f.host.replace(3)));
    assert.equal(f.calls.length, 0, 'A must not invoke any native on B after coercion');
  });
}
