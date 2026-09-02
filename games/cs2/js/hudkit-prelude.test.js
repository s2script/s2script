const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

function throwingNs(name) {
  return new Proxy({}, {
    get(_t, prop) {
      if (typeof prop !== "string") return undefined;
      throw new Error(`s2script: ${name} outside the load window`);
    },
  });
}

function evalFile(name) {
  const src = readFileSync(join(__dirname, name), "utf8");
  new Function(src)();
}

test.afterEach(() => {
  delete globalThis.__s2_game_ns;
  delete globalThis.__s2pkg_cs2;
  delete globalThis.__s2pkg_game_ctx;
  delete globalThis.__s2pkg_timers;
  delete globalThis.__s2pkg_menu;
  delete globalThis.__s2pkg_votes;
  delete globalThis.__s2pkg_clients;
  delete globalThis.__s2pkg_hudinput;
  delete globalThis.__s2pkg_menuhud;
  delete globalThis.__s2pkg_voterail;
  delete globalThis.__s2ui_pool;
});

test("CS2 prelude can claim hudkit and register menu + vote renderers without a load ctx", () => {
  const hud = {
    set() { return null; },
    setClass() { return null; },
    show() { return null; },
    hide() { return null; },
    cursor() { return null; },
    forget() {},
    onClick() {},
    forSlot() {
      return {
        setText() {},
        setClass() {},
        show() {},
        hide() {},
        cursor() {},
      };
    },
  };

  globalThis.__s2_game_ns = throwingNs;
  globalThis.__s2pkg_cs2 = { CustomHudLayout: throwingNs("ui") };
  globalThis.__s2pkg_game_ctx = { ui: () => ({ create: () => hud, hud: () => hud }) };
  globalThis.__s2pkg_timers = {
    nextFrame: () => Promise.resolve(),
    delay: () => Promise.resolve(),
    after() {},
  };
  const registered = {};
  globalThis.__s2pkg_menu = {
    Menu: { registerRenderer: (name, renderer) => { registered[name] = renderer; } },
    MenuStyle: { Chat: "chat", Center: "center" },
  };
  let voteRenderer;
  globalThis.__s2pkg_votes = {
    Vote: { registerTallyRenderer: (renderer) => { voteRenderer = renderer; } },
  };
  globalThis.__s2pkg_clients = { Clients: { onDisconnect() {} } };
  globalThis.__s2pkg_hudinput = { HudInput: { arm() {}, disarm() {} } };

  assert.doesNotThrow(() => evalFile("components.js"));
  assert.doesNotThrow(() => evalFile("menuhud.js"));
  assert.doesNotThrow(() => evalFile("voterail.js"));

  assert.equal(typeof globalThis.__s2pkg_cs2.hudkit.modal, "function");
  assert.equal(globalThis.__s2pkg_cs2.hudkit.layout, hud);
  assert.ok(registered.center, "Menu HUD renderer must register during prelude");
  assert.ok(registered.chat, "Chat menus must use the same HUD renderer");
  assert.ok(voteRenderer, "Vote.registerTallyRenderer must run during prelude");
  assert.doesNotThrow(() => globalThis.__s2pkg_cs2.hudkit.modal({
    title: "T",
    rows: [],
    buttons: [],
  }));
});
