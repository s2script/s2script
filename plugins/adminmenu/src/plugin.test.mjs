import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';
import { installClientHost } from '../../../packages/sdk/test/client-host.mjs';

test('deferred A disconnect preserves B admin sheet and B movement restoration', () => {
  const ctx = { module: { exports: {} }, console };
  const host = installClientHost(ctx, [3]);
  const commands = {}, closes = [];
  let spec;
  let pawn = { moveType: 2 };
  const sdk = {
    Clients: ctx.__s2pkg_clients.Clients,
    command: (name, fn) => { commands[name] = fn; },
    topmenu: { addCategory() {} }, translations: { load() {} },
    TopMenu: { snapshot: () => ({ categories: ['Player Commands'], items: [{ id: 'slap', category: 'Player Commands', flags: 0 }] }) },
    Admin: { forSlot: () => ({ flags: 0 }) }, ADMFLAG: { ROOT: 1 },
    Translations: { translate: (_slot, key) => key }, HookResult: { Handled: 2 },
  };
  ctx.require = name => name === '@s2script/sdk' ? sdk : {
    Player: { fromSlot: () => ({ pawn }) },
    hudkit: { dashboard: opts => { spec = opts; return { open() {}, close: slot => closes.push(slot) }; } },
  };
  vm.createContext(ctx);
  const source = readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8');
  vm.runInContext(transformSync(source, { loader: 'ts', format: 'cjs' }).code, ctx);
  const api = ctx.module.exports;
  api.OnPluginStart();
  const a = sdk.Clients.fromSlot(3);
  const open = () => commands.sm_admin({ callerSlot: 3, replyT() {} });
  open(); assert.equal(pawn.moveType, 0);
  host.replace(3); pawn = { moveType: 3 };
  open(); assert.equal(pawn.moveType, 0, 'B gets its own freeze state');
  api.OnClientDisconnect(a);
  assert.equal(closes.length, 0, 'A must not close B sheet');
  spec.onClose(3);
  assert.equal(pawn.moveType, 3, 'B restores its own movement type');
});
