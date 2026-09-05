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

// The concatenation-order integration test for the P0-2 contract: components.js, menuhud.js and
// voterail.js evaluate at prelude time WITHOUT a load ctx and register NOTHING — no stand-in ui
// base, no modal claim, no renderer overwriting the core's chat renderer. Everything binds the
// moment the core builds the plugin's ctx-bound ui namespace (simulated here by calling the
// decorated factory the way __s2_make_ctx does), while the load window is still open.
test("CS2 prelude defers hudkit binding to the plugin's ctx-bound ui base", () => {
  const clickHandlers = {};
  const hud = {
    set() { return null; },
    setClass() { return null; },
    show() { return null; },
    hide() { return null; },
    cursor() { return null; },
    forget() {},
    onClick(id, fn) {
      assert.equal(clickHandlers[id], undefined, "duplicate click binding: " + id);
      clickHandlers[id] = fn;
    },
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
  globalThis.__s2pkg_clients = { Clients: { onDisconnect() {}, onActive() {} } };
  globalThis.__s2pkg_hudinput = { HudInput: { arm() {}, disarm() {} } };

  assert.doesNotThrow(() => evalFile("components.js"));
  assert.doesNotThrow(() => evalFile("menuhud.js"));
  assert.doesNotThrow(() => evalFile("voterail.js"));

  const hudkit = globalThis.__s2pkg_cs2.hudkit;
  assert.equal(typeof hudkit.modal, "function");
  assert.equal(typeof hudkit.dashboard, "function");
  assert.equal(typeof hudkit.whenLive, "function");

  // Prelude eval bound NOTHING: there is no load ctx yet, so a kit minted here could only sit on
  // a stand-in registrar (the P0-2 defect — panels that paint but never deliver a click). The
  // core's chat renderer must still be the registered menu renderer at this point.
  assert.deepEqual(registered, {}, "menuhud must not overwrite the core chat renderer at prelude eval");
  assert.equal(voteRenderer, undefined, "voterail must not register a tally renderer at prelude eval");
  assert.equal(hudkit.layout, null, "no layout before the ctx-bound base exists");
  assert.equal(hudkit.modal({ title: "T", rows: [], buttons: [] }), null,
    "a pre-ctx claim is refused, not half-alive");
  // Static descriptor data stays readable pre-live: the documented
  // CustomHudLayout.components(hudkit.spec) pattern must not depend on resolution order.
  assert.equal(hudkit.spec.resource, "panorama/layout/custom_game/s2script_lib.xml");

  // __s2_make_ctx builds the plugin's ui namespace through the decorated factory. That call is
  // what makes the kit live — and it happens with the load window open, so the buffered thunks
  // below stand in for ctxReg registrations replayed at arm.
  const bufferedThunks = [];
  const base = globalThis.__s2pkg_game_ctx.ui(
    (thunk) => { bufferedThunks.push(thunk); },
    (fn) => fn,
  );

  assert.equal(hudkit.layout, hud, "hudkit resolves to the ctx-bound base's layout");
  assert.ok(registered.center, "Menu HUD renderer registers once the kit is live");
  assert.ok(registered.chat, "Chat menus use the same HUD renderer");
  assert.equal(registered.center, registered.chat);
  assert.ok(voteRenderer, "Vote tally renderer registers once the kit is live");
  assert.ok(clickHandlers.s2_vote_o0, "vote option clicks bind at go-live, not at first paint");
  assert.doesNotThrow(() => hudkit.modal({ title: "T", rows: [], buttons: [] }));
  // ctx.ui.kit and hudkit.* are ONE instance — the shared modal pool claims stay coherent.
  assert.equal(base.kit.layout, hudkit.layout);
  assert.equal(base.components(hudkit.spec), base.kit, "explicit library descriptor reuses the live kit");
  assert.equal(base.components({ ...hudkit.spec }), base.kit, "the resource identifies the kit");
});
