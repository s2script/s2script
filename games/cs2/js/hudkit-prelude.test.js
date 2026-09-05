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
  for (const member of ["modal", "dashboard", "badge", "toast", "callout", "banner",
    "motd", "forSlot", "hideAll", "forget", "ensure", "budget"]) {
    assert.throws(() => hudkit[member](),
      new RegExp("hudkit\\." + member + " requires plugin context.*OnPluginStart"), member);
  }
  for (const member of ["layout", "hud"]) {
    assert.throws(() => hudkit[member],
      new RegExp("hudkit\\." + member + " requires plugin context.*OnPluginStart"), member);
  }
  // Static descriptor data stays readable pre-live: the documented
  // CustomHudLayout.components(hudkit.spec) pattern must not depend on resolution order.
  assert.equal(hudkit.spec.resource, "panorama/layout/custom_game/s2script_lib.xml");
  assert.equal(hudkit.descriptor, hudkit.spec);

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

// Each VM is a real plugin prelude; only the engine and native pool boundary are simulated.
function pluginWorld() {
  const vm = require("node:vm");
  const owners = Array(6).fill(null);
  const retired = Array(6).fill(null);
  let epoch = 0, activeEpoch = null;
  const entities = [];
  const writes = [];
  const plugins = [];
  function plugin() {
    const owner = plugins.length;
    const lifecycle = { active: [], disconnect: [], map: [] };
    const fallbackCalls = [];
    const fallback = {
      open(s) { fallbackCalls.push(["open", s.slot]); },
      update(s) { fallbackCalls.push(["update", s.slot]); },
      close(slot) { fallbackCalls.push(["close", slot]); },
    };
    const renderers = { chat: fallback };
    let sealed = false;
    const ctx = vm.createContext({
      console: { log() {} }, __s2pkg_cs2: {},
      __s2_ui_pool_claim(_kind, capacity) {
        const index = owners.findIndex((v, i) => i < capacity && v === null &&
          (activeEpoch === null || retired[i] !== activeEpoch));
        if (index >= 0) owners[index] = owner;
        return index;
      },
      __s2_ui_pool_release(_kind, index) {
        assert.equal(owners[index], owner);
        owners[index] = null;
        retired[index] = activeEpoch;
        return true;
      },
      __s2pkg_entity: {
        Entity: { findByClass: () => entities },
        createEntity(_cls, kv) {
          const e = { id: entities.length + 1, name: kv.targetname, isValid: () => true };
          entities.push(e); return e;
        },
      },
      __s2pkg_cs2_calls: { call: (name) => (...args) => writes.push({ owner, name, args }), status: () => "" },
      __s2pkg_server: { Server: { onMapStart: (f) => lifecycle.map.push(f), getCvar: () => "3790153369" } },
      __s2pkg_clients: { Clients: {
        all: () => [{ signonState: 6 }],
        onActive: (f) => lifecycle.active.push(f), onDisconnect: (f) => lifecycle.disconnect.push(f),
      } },
      __s2pkg_menu: { Menu: { registerRenderer(name, renderer) {
        const prev = renderers[name]; renderers[name] = renderer; return prev;
      } }, MenuStyle: { Chat: "chat", Center: "center" } },
      __s2_hook_on: () => 1,
    });
    for (const file of ["ui.js", "components.js", "menuhud.js"]) {
      vm.runInContext(readFileSync(join(__dirname, file), "utf8"), ctx);
    }
    const base = ctx.__s2pkg_game_ctx.ui((fn) => {
      assert.equal(sealed, false, "click registration must happen in the load window");
      return fn();
    }, (fn) => fn);
    sealed = true;
    const p = { base, hudkit: ctx.__s2pkg_cs2.hudkit, renderers, lifecycle, fallbackCalls,
      click(slot, id) { base.kit.layout.dispatchClick(slot, id); } };
    plugins.push(p); return p;
  }
  function session(slot) {
    const picks = [];
    return { slot, picks, menu: { title: "T" },
      view: () => ({ lines: [{ text: "Pick", key: "1", selectable: true, index: 0 }], page: 0, pageCount: 1, exit: true }),
      pickNumber: (n) => picks.push(n), cancel() {},
    };
  }
  function dispatchClick(slot, id) {
    const previous = activeEpoch;
    if (activeEpoch === null) activeEpoch = ++epoch;
    try { for (const p of plugins) p.click(slot, id); }
    finally { activeEpoch = previous; }
  }
  return { owners, writes, plugins, plugin, session, dispatchClick };
}

test("14 idle plugins reserve no panels and every plugin can open a clickable menu after load", () => {
  const w = pluginWorld();
  for (let i = 0; i < 14; i++) w.plugin();
  assert.equal(w.owners.filter((v) => v !== null).length, 0);
  for (const p of w.plugins) {
    const s = w.session(1);
    p.renderers.center.open(s);
    for (const other of w.plugins) other.click(1, "s2_m0_r0");
    assert.deepEqual(s.picks, [1], "only the current owner's handler fires");
    p.renderers.center.close(1);
    assert.ok(w.owners.every((v) => v === null));
  }
});

test("menu claims live until the final viewer closes and recover from actual exhaustion", () => {
  const w = pluginWorld();
  for (let i = 0; i < 7; i++) w.plugin();
  for (let i = 0; i < 6; i++) w.plugins[i].renderers.center.open(w.session(i));
  const seventh = w.plugins[6];
  seventh.renderers.center.open(w.session(10));
  seventh.renderers.center.update(w.session(10));
  seventh.renderers.center.close(10);
  assert.deepEqual(seventh.fallbackCalls, [["open", 10], ["update", 10], ["close", 10]]);
  w.plugins[0].renderers.center.open(w.session(20));
  w.plugins[0].renderers.center.close(0);
  assert.equal(w.owners[0], 0, "another viewer still owns the sheet");
  w.plugins[0].renderers.center.close(20);
  const s = w.session(10);
  seventh.renderers.center.open(s);
  seventh.click(10, "s2_m0_r0");
  assert.deepEqual(s.picks, [1]);
});

test("disconnect and map transition release idle menu claims", () => {
  const w = pluginWorld(), p = w.plugin();
  p.renderers.center.open(w.session(1));
  p.lifecycle.disconnect.forEach((fn) => fn({ slot: 1 }));
  assert.ok(w.owners.every((v) => v === null));
  p.renderers.center.open(w.session(2));
  p.lifecycle.map.forEach((fn) => fn());
  assert.ok(w.owners.every((v) => v === null));
});

test("released modal handles cannot steal a reused panel or dispatch stale clicks", () => {
  const w = pluginWorld(), p = w.plugin();
  let oldPicks = 0, newPicks = 0;
  const old = p.base.kit.modal({ rows: [{ a: "old" }], onPick: () => oldPicks++ });
  old.open(1); old.release();
  const next = p.base.kit.modal({ rows: [{ a: "new" }], onPick: () => newPicks++ });
  next.open(1);
  const beforeStale = w.writes.length;
  old.release(); old.close(1); old.setCursor(1, false);
  assert.equal(w.writes.length, beforeStale, "stale handles cannot repaint or clear capture");
  assert.equal(w.owners[0], 0);
  assert.throws(() => old.open(1), /released/);
  p.click(1, "s2_m0_r0");
  assert.equal(oldPicks, 0); assert.equal(newPicks, 1);
});


test("panel handoff forces repaint when a previous owner reacquires the same tree", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  a.renderers.center.open(w.session(1));
  a.renderers.center.close(1);
  const other = w.session(1); other.menu.title = "Other plugin";
  b.renderers.center.open(other);
  b.renderers.center.close(1);
  const before = w.writes.length;
  a.renderers.center.open(w.session(1));
  assert.ok(w.writes.slice(before).some((v) => v.owner === 0 &&
    v.name === "setDialogVariableStringForPlayer" && v.args[3] === "s2_m0_title" && v.args[4] === "T"),
  "the first owner's cached title must not suppress restoring it after a handoff");
});


test("a cross-plugin menu transition does not forward the triggering click to the new menu", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const first = w.session(1), next = w.session(1);
  first.pickNumber = (n) => {
    first.picks.push(n);
    a.renderers.center.close(1);
    b.renderers.center.open(next); // Synchronous interface handoff during A's click callback.
  };
  a.renderers.center.open(first);
  w.dispatchClick(1, "s2_m0_r0");
  assert.deepEqual(first.picks, [1]);
  assert.deepEqual(next.picks, [], "the new menu needs a fresh user click");
  w.dispatchClick(1, "s2_m1_r0");
  assert.deepEqual(next.picks, [1]);
});

test("built-in menus keep Next and Back bound to the player who sees them", () => {
  const w = pluginWorld(), p = w.plugin();
  const first = w.session(1), second = w.session(2);
  const firstView = first.view(), secondView = second.view();
  first.view = () => ({ ...firstView, page: 0, pageCount: 2 });
  second.view = () => ({ ...secondView, page: 1, pageCount: 2 });
  p.renderers.center.open(first); p.renderers.center.open(second);
  p.click(1, "s2_m0_f0"); p.click(2, "s2_m0_f0");
  assert.deepEqual(first.picks, [9], "Next must stay Next after another player paints Back");
  assert.deepEqual(second.picks, [8]);
});


test("ambient hudkit remains usable in callbacks after initialization has settled", () => {
  const w = pluginWorld(), p = w.plugin(); // The ctx registrar is sealed before returning.
  const kit = p.hudkit;
  assert.equal(kit.layout, p.base.kit.layout);
  assert.equal(kit.hud, kit.layout);
  assert.equal(kit.forSlot(1).slot, 1);
  assert.equal(typeof kit.dashboard({ title: "Dashboard", tabs: [], rows: () => [] }).open, "function");
  assert.equal(typeof kit.motd(1, { title: "Rules", sections: [] }).close, "function");
  const modals = Array.from({ length: 6 }, () => kit.modal({ rows: [] }));
  assert.ok(modals.every(Boolean));
  assert.equal(kit.modal({ rows: [] }), null, "null denotes actual pool exhaustion");
  modals[0].release();
  assert.ok(kit.modal({ rows: [] }), "a later callback can claim a released slot");
  assert.equal(kit.ensure(), null, "successful HudResult is null");
  assert.equal(typeof kit.budget().cap, "number");
});
