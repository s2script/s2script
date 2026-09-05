import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { transformSync } from "esbuild";

const source = transformSync(readFileSync(new URL("../src/plugin.ts", import.meta.url), "utf8"),
  { loader: "ts", format: "cjs", target: "es2022" }).code;

function harness() {
  const replies = [], rules = new Map();
  const state = { reject: null, removeWorks: true, resetAllCount: 0, writes: 0 };
  let handler, entity;
  const sdk = {
    HookResult: { Handled: 1 },
    Clients: { all: () => [{ isBot: false, signonState: 6 }],
      fromSlot: () => ({ isBot: false, signonState: 6 }) },
    command: { server: (_name, fn) => { handler = fn; } },
    createEntity: () => {
      entity = { index: 99, id: 7, isValid: () => entity.live,
        live: true, remove: () => { if (state.removeWorks) entity.live = false; return state.removeWorks; } };
      return entity;
    },
    Transmit: {
      setVisibleTo: (e, slots) => { rules.set(e.id, slots); return true; },
      reset: (e) => rules.delete(e.id),
      resetAll: () => { rules.clear(); state.resetAllCount++; }, stats: () => ({}),
    },
  };
  const ctx = vm.createContext({ console, exports: {}, module: { exports: {} },
    require: (name) => name === "@s2script/sdk" ? sdk : { Player: { fromSlot: () => null } },
    __s2pkg_cs2_calls: { status: () => "available", call: (name) => () => {
      state.writes++;
      return state.reject === name ? null : undefined;
    } },
  });
  vm.runInContext(source, ctx);
  ctx.module.exports.OnPluginStart();
  function run(text) {
    const args = text.split(" ");
    replies.length = 0;
    handler({ arg: (i) => args[i] ?? "", reply: (s) => replies.push(s) });
    return replies.join("\n");
  }
  return { run, state, rules };
}

test("rejected void HUD call stops painting and does not report marker written", () => {
  const h = harness();
  h.run("create A");
  h.state.reject = "setDialogVariableStringForPlayer";
  const reply = h.run("paint A all CHECK");
  assert.match(reply, /REFUSED:.*rejected/i);
  assert.doesNotMatch(reply, /^wrote/);
  assert.equal(h.state.writes, 1);
  h.state.reject = null;
  assert.match(h.run("paint A 0 RETRY"), /^wrote/);
});

test("failed cleanup preserves the live entity and recipient rule for retry", () => {
  const h = harness();
  h.run("create A");
  h.run("audience A 0");
  h.state.removeWorks = false;
  assert.match(h.run("clean"), /FAILED.*A/);
  assert.equal(h.state.resetAllCount, 0);
  assert.equal(h.rules.get(7)[0], 0);
  assert.equal(JSON.parse(h.run("status")).entities.length, 1);
  h.state.removeWorks = true;
  assert.match(h.run("clean"), /removed/);
  assert.equal(h.rules.size, 0);
  assert.equal(JSON.parse(h.run("status")).entities.length, 0);
});
