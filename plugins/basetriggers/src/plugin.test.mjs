import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import { transformSync } from "esbuild";

const source = transformSync(readFileSync(new URL("./plugin.ts", import.meta.url), "utf8"), {
  loader: "ts", format: "cjs",
}).code;

function fixture() {
  const commands = new Map();
  const players = new Map();
  const replies = [];
  const command = (name, handler) => commands.set(name, handler);
  command.admin = () => assert.fail("self-kill must not require admin privileges");
  const sdk = {
    command,
    translations: { load() {} },
    Chat: { toAll() {} },
    Server: { getCvar: () => "", gameTime: 0, mapName: "de_dust2" },
    HookResult: { Continue: 0, Handled: 2 },
    nextFrame: () => Promise.resolve(),
    Translations: { translate: (_slot, key) => key },
  };
  const context = vm.createContext({
    module: { exports: {} },
    require(name) {
      if (name === "@s2script/sdk") return sdk;
      if (name === "@s2script/cs2") return { Player: { fromSlot: slot => players.get(slot) ?? null } };
      throw new Error(`Unexpected import: ${name}`);
    },
  });
  vm.runInContext(source, context);
  const api = context.module.exports;
  api.OnPluginStart();
  function invoke(slot) {
    const handler = commands.get("sm_kill");
    assert.ok(handler, "sm_kill is registered");
    return handler({ callerSlot: slot, replyT: (...args) => replies.push(args) });
  }
  return { players, replies, invoke, api };
}

test("self-kill is public and slays only the invoking player's living pawn", () => {
  const f = fixture();
  let ownSlays = 0;
  let otherSlays = 0;
  f.players.set(3, { pawn: { isValid: true, health: 100, slay: () => ownSlays++ } });
  f.players.set(4, { pawn: { isValid: true, health: 100, slay: () => otherSlays++ } });
  assert.equal(f.invoke(3), 2);
  assert.equal(ownSlays, 1);
  assert.equal(otherSlays, 0);
  assert.deepEqual(f.replies, [["Killed Yourself"]]);
});

test("server console cannot use self-kill", () => {
  const f = fixture();
  assert.equal(f.invoke(-1), 2);
  assert.deepEqual(f.replies, [["Kill Requires Player"]]);
});

test("self-kill skips absent, dead, invalid and unreadable pawns", () => {
  for (const pawn of [null, { isValid: false, health: 100 }, { isValid: true, health: 0 }, { isValid: true, health: null }]) {
    const f = fixture();
    f.players.set(3, { pawn: pawn && { ...pawn, slay: () => assert.fail("unavailable pawn must not be slain") } });
    assert.equal(f.invoke(3), 2);
    assert.deepEqual(f.replies, [["Player no longer available"]]);
  }
  const f = fixture();
  assert.equal(f.invoke(3), 2);
  assert.deepEqual(f.replies, [["Player no longer available"]]);
});

test("unrelated chat continues through the existing basetriggers public", () => {
  const f = fixture();
  assert.equal(f.api.OnClientSayCommand(3, "hello", false), 0);
});
