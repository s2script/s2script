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

function fixture({ chatMode = true } = {}) {
  const players = new Map();
  const admins = new Set();
  const sent = [];
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
      target: () => [],
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
  return { api, sent };
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
