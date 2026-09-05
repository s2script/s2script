import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';
import { installClientHost } from '../../../../packages/sdk/test/client-host.mjs';

test('deferred A disconnect preserves B RTV request and A request never counts as B', () => {
  const ctx = { module: { exports: {} }, console };
  const host = installClientHost(ctx, [3, 4]);
  const messages = [];
  const sdk = {
    Clients: ctx.__s2pkg_clients.Clients, HookResult: { Handled: 2, Continue: 0 },
    UserMessages: { onPre() {} },
    config: { getInt: () => 0, getFloat: () => 1 },
    Chat: { toSlot: (_slot, text) => messages.push(text), toAll: text => messages.push(text) },
    Translations: { translate: (_slot, key) => key },
  };
  ctx.require = name => name === '@s2script/sdk' ? sdk : { Player: { fromSlot: () => ({ playerName: 'current' }) } };
  vm.createContext(ctx);
  const source = readFileSync(new URL('./plugin.ts', import.meta.url), 'utf8');
  vm.runInContext(transformSync(source, { loader: 'ts', format: 'cjs' }).code, ctx);
  const api = ctx.module.exports;
  const a = sdk.Clients.fromSlot(3);
  api.OnClientSayCommand(3, 'rtv', false);
  host.replace(3);
  api.OnClientSayCommand(3, 'rtv', false);
  assert.equal(messages.at(-1), 'Rtv Wants To Vote', 'B has not voted yet');
  api.OnClientDisconnect(a);
  api.OnClientSayCommand(3, 'rtv', false);
  assert.equal(messages.at(-1), 'Rtv Already Voted', 'B request survives A disconnect');
});
