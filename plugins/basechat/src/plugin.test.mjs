import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import { transformSync } from "esbuild";

const source = transformSync(
  readFileSync(new URL("./plugin.ts", import.meta.url), "utf8"),
  { loader: "ts", format: "cjs" },
).code;

const CHAT = 32;

function fixture({ chatMode = true, targetSlots = [1] } = {}) {
  const players = new Map();
  const admins = new Set();
  const sent = [];
  const targetCalls = [];
  const sdk = {
    command: { admin() {} },
    translations: { load() {} },
    config: { getBool: key => { assert.equal(key, "chat_mode"); return chatMode; } },
    Chat: { toSlot: (slot, msg) => sent.push([slot, msg]) },
    Admin: { forSlot: slot => admins.has(slot) ? { hasFlags: f => (f & CHAT) === f } : null },
    ADMFLAG: { CHAT },
    HookResult: { Continue: 0, Handled: 2 },
    Translations: { translate: (_slot, key, ...args) => `${key}|${args.join("|")}` },
  };
  const cs2 = {
    Player: {
      fromSlot: slot => players.get(slot) ?? null,
      allConnected: () => [...players.values()],
      target: (pattern, callerSlot) => {
        targetCalls.push([pattern, callerSlot]);
        return targetSlots.map(slot => players.get(slot));
      },
    },
    Activity: { formatSource: (actor) => ({ show: true, name: `player-${actor}` }) },
  };
  const context = vm.createContext({
    module: { exports: {} },
    console,
    require(name) {
      if (name === "@s2script/sdk") return sdk;
      if (name === "@s2script/cs2") return cs2;
      throw new Error(`unexpected import ${name}`);
    },
  });
  vm.runInContext(source, context);
  const api = context.module.exports;
  api.OnPluginStart();
  for (const slot of [0, 1, 2]) players.set(slot, { slot, playerName: `player-${slot}` });
  admins.add(0); // slot 0 is the chat admin; 1 and 2 are players
  return { api, sent, targetCalls };
}

test("non-admin @ in team chat reaches admins and echoes to the sender as (TO ADMINS)", () => {
  const { api, sent } = fixture();
  assert.equal(api.OnClientSayCommand(1, "@help please", true), 2);
  assert.deepEqual(sent, [
    [0, "Say To Admins|player-1|help please"],
    [1, "Say To Admins|player-1|help please"],
  ]);
});

test("admin @ in team chat reads (ADMINS) and only admins see it", () => {
  const { api, sent } = fixture();
  assert.equal(api.OnClientSayCommand(0, "@heads up", true), 2);
  assert.deepEqual(sent, [[0, "Say Admins|player-0|heads up"]]);
});

test("chat_mode off: a non-admin @ in team chat is ordinary chat", () => {
  const { api, sent } = fixture({ chatMode: false });
  assert.equal(api.OnClientSayCommand(1, "@help", true), 0);
  assert.deepEqual(sent, []);
});

test("non-admin @ in all chat is ordinary chat", () => {
  const { api, sent } = fixture();
  assert.equal(api.OnClientSayCommand(1, "@everyone", false), 0);
  assert.deepEqual(sent, []);
});

test("admin @ in all chat is sm_say to everyone", () => {
  const { api, sent } = fixture();
  assert.equal(api.OnClientSayCommand(0, "@hello", false), 2);
  assert.equal(sent.length, 3);
  assert.ok(sent.every(([, m]) => m === "Say All|player-0|hello"));
});

test("bare @ in team chat is consumed and sends nothing", () => {
  const { api, sent } = fixture();
  assert.equal(api.OnClientSayCommand(1, "@  ", true), 2);
  assert.deepEqual(sent, []);
});

for (const teamonly of [false, true]) {
  const channel = teamonly ? "team" : "all";
  for (const chatMode of [false, true]) {
    test(`admin @@ in ${channel} chat sends a private message with chat_mode ${chatMode}`, () => {
      const { api, sent, targetCalls } = fixture({ chatMode });
      assert.equal(api.OnClientSayCommand(0, "@@player-1 hello there", teamonly), 2);
      assert.deepEqual(targetCalls, [["player-1", 0]]);
      assert.deepEqual(sent, [
        [1, "Say Private To|player-1|player-0|hello there"],
        [0, "Say Private Echo|player-1|hello there"],
      ]);
    });

    test(`non-admin @@ in ${channel} chat stays ordinary chat with chat_mode ${chatMode}`, () => {
      const { api, sent, targetCalls } = fixture({ chatMode });
      assert.equal(api.OnClientSayCommand(2, "@@player-1 hello there", teamonly), 0);
      assert.deepEqual(targetCalls, []);
      assert.deepEqual(sent, []);
    });
  }

  test(`admin @@ without a message in ${channel} chat replies with usage`, () => {
    const { api, sent, targetCalls } = fixture();
    assert.equal(api.OnClientSayCommand(0, "@@player-1", teamonly), 2);
    assert.deepEqual(targetCalls, []);
    assert.deepEqual(sent, [[0, "Usage Psay Trigger|"]]);
  });

  for (const [targetSlots, expected] of [
    [[], "No matching players|"],
    [[1, 2], "More than one client matched|player"],
  ]) {
    test(`admin @@ target error in ${channel} chat: ${expected}`, () => {
      const { api, sent, targetCalls } = fixture({ targetSlots });
      assert.equal(api.OnClientSayCommand(0, "@@player hello", teamonly), 2);
      assert.deepEqual(targetCalls, [["player", 0]]);
      assert.deepEqual(sent, [[0, expected]]);
    });
  }
}
