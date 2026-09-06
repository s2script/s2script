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

function plain(value) { return JSON.parse(JSON.stringify(value)); }

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
  assert.equal(typeof hudkit.tryOwnDashboard, "function");
  assert.equal(typeof hudkit.whenLive, "function");

  // Prelude eval bound NOTHING: there is no load ctx yet, so a kit minted here could only sit on
  // a stand-in registrar (the P0-2 defect — panels that paint but never deliver a click). The
  // core's chat renderer must still be the registered menu renderer at this point.
  assert.deepEqual(registered, {}, "menuhud must not overwrite the core chat renderer at prelude eval");
  assert.equal(voteRenderer, undefined, "voterail must not register a tally renderer at prelude eval");
  for (const member of ["modal", "tryModal", "dashboard", "tryOwnDashboard", "badge", "tryBadge", "toast", "callout", "banner",
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
function pluginWorld(options = {}) {
  const vm = require("node:vm");
  const owners = Array(6).fill(null);
  const retired = Array(6).fill(null);
  let epoch = 0, activeEpoch = null;
  const entities = [];
  const writes = [];
  const plugins = [];
  const focus = new Map();
  const owned = new Map();
  const focusCalls = [];
  const activationCalls = [];
  let nextFocus = 1;
  let nextOwned = 1;
  function live(r) {
    return clientGenerations.get(r.slot) === r.generation && entities.some(e =>
      e.index === r.index && e.id === r.id && e.isValid());
  }
  function group(r) { return [...focus.values()].filter(x => x.key === r.key && live(x)); }
  function winner(r) { return group(r).sort((a, b) => b.priority - a.priority || b.order - a.order)[0]; }
  function retire(r) {
    if (r.adapter.suspend) writes.push({ owner: r.owner, name: r.adapter.suspend.call,
      args: [entities.find(e => e.id === r.id), r.slot, ...r.adapter.suspend.args], host: true });
    if (r.adapter.capture) switches.native(r.owner)(r.adapter.capture.call,
      r.index, r.id, r.slot, r.adapter.capture.token, false);
  }
  function releaseFocus(owner, token) {
    const r = focus.get(token);
    if (!r || r.owner !== owner) return false;
    const wasWinner = live(r) && winner(r) === r;
    focus.delete(token);
    if (r.parentToken) {
      const parent = owned.get(r.parentToken);
      if (parent && parent.childToken === token) parent.childToken = null;
    }
    if (wasWinner) { retire(r); const next = winner(r); if (next) next.state = "waiting"; }
    return true;
  }
  function releaseOwned(owner, token) {
    const r = owned.get(token);
    if (!r || r.owner !== owner) return false;
    if (!live(r)) {
      if (r.childToken) focus.delete(r.childToken);
      owned.delete(token);
      return false;
    }
    if (r.childToken) releaseFocus(owner, r.childToken);
    owned.delete(token);
    retire(r);
    return true;
  }
  function focusNatives(owner) {
    function own(token) {
      const r = focus.get(token) || owned.get(token);
      return r && r.owner === owner && live(r) ? r : null;
    }
    function reserveFocus(surface, index, id, slot, priority, json, parentToken) {
      if (parentToken) {
        const parent = owned.get(parentToken);
        if (!parent || parent.owner !== owner || !live(parent)) {
          return { ok: false, error: { code: "Released", message: "parent released" } };
        }
        if (parent.childToken) return { ok: false, error: { code: "Busy", message: "parent already linked" } };
      }
      focusCalls.push({ surface, index, id, slot, priority, adapter: JSON.parse(json), parentToken });
      if (options.focusError) return { ok: false, error: { code: options.focusError, message: "native failure" } };
      const token = "opaque/" + nextFocus++;
      const r = { token, owner, index, id, slot, priority, order: nextFocus,
        generation: clientGenerations.get(slot), adapter: JSON.parse(json), parentToken,
        key: JSON.stringify([surface, index, id, slot, clientGenerations.get(slot)]), state: "covered" };
      const prev = winner(r);
      focus.set(token, r);
      if (parentToken) owned.get(parentToken).childToken = token;
      if (winner(r) === r) { if (prev) { retire(prev); prev.state = "covered"; } r.state = "ready"; }
      return { ok: true, value: token };
    }
    return {
      __s2_surface_reserve(surface, index, id, slot, priority, json) {
        return reserveFocus(surface, index, id, slot, priority, json, null);
      },
      __s2_surface_reserve_linked(surface, index, id, slot, priority, json, parentToken) {
        return reserveFocus(surface, index, id, slot, priority, json, parentToken);
      },
      __s2_surface_reserve_owned(surface, index, id, slot, mode, json) {
        const adapters = JSON.parse(json);
        const key = JSON.stringify([surface, index, id, slot, clientGenerations.get(slot)]);
        const records = [...owned.values()].filter(r => r.key === key && live(r));
        let lane = adapters.findIndex((_adapter, i) => !records.some(r => r.lane === i));
        if (lane < 0 && mode === "legacy") {
          const previous = records.filter(r => r.mode === "legacy").sort((a, b) => a.order - b.order)[0];
          if (previous) { lane = previous.lane; releaseOwned(previous.owner, previous.token); }
        }
        if (lane < 0) return { ok: false, error: { code: "Busy", message: "surface busy" } };
        const token = "owned/" + nextOwned++;
        owned.set(token, { token, owner, index, id, slot, mode, lane, adapter: adapters[lane], key,
          generation: clientGenerations.get(slot), state: "ready", order: nextOwned, childToken: null });
        return { ok: true, value: { token, lane } };
      },
      __s2_surface_clear_legacy(surface, index, id, slot, json) {
        const adapters = JSON.parse(json);
        const key = JSON.stringify([surface, index, id, slot, clientGenerations.get(slot)]);
        const records = [...owned.values()].filter(r => r.key === key && live(r));
        const occupied = new Set(records.map(r => r.lane));
        for (const r of records) if (r.mode === "legacy") releaseOwned(r.owner, r.token);
        adapters.forEach((adapter, lane) => {
          if (!occupied.has(lane)) retire({ owner, index, id, slot, adapter });
        });
        return { ok: true };
      },
      __s2_surface_state(token) { const r = own(token); return r ? r.state : "invalid"; },
      __s2_surface_activate(token) {
        const r = own(token);
        if (!r || r.state !== "ready" || options.activateError) return false;
        if (r.parentToken) {
          const parent = owned.get(r.parentToken);
          if (!parent || parent.state !== "active") return false;
        }
        activationCalls.push(token);
        r.state = "active"; r.activatedEpoch = activeEpoch; return true;
      },
      __s2_surface_active(token) {
        const r = own(token);
        return !!r && r.state === "active" && !(activeEpoch !== null && r.activatedEpoch === activeEpoch);
      },
      __s2_surface_release: token => focus.has(token) ? releaseFocus(owner, token) : releaseOwned(owner, token),
    };
  }
  let nextClientGeneration = 1;
  const clientGenerations = new Map();
  function connect(slot) { clientGenerations.set(slot, nextClientGeneration++); }
  for (let slot = 0; slot < 64; slot++) connect(slot);
  function clientFor(slot) {
    const generation = clientGenerations.get(slot);
    if (!generation) return null;
    return {
      slot, generation, steamId: "76561198000000000",
      isValid: () => clientGenerations.get(slot) === generation,
    };
  }
  const switches = require("./shared-switch-fixture.js").sharedSwitchFixture(
    (index, id) => entities.find(e => e.index === index && e.id === id),
    (name, entity, slot, on) => {
      writes.push({ name, args: [entity, slot, on] });
      return on && options.captureError ? options.captureError : null;
    });
  function plugin() {
    const owner = plugins.length;
    const lifecycle = { active: [], disconnect: [], map: [], frame: [], click: [] };
    const fallbackCalls = [];
    const pendingTimers = [];
    const fallback = {
      open(s) { fallbackCalls.push(["open", s.slot]); },
      update(s) { fallbackCalls.push(["update", s.slot]); },
      close(slot) { fallbackCalls.push(["close", slot]); },
    };
    const renderers = { chat: fallback };
    const logs = [];
    let sealed = false;
    const ctx = vm.createContext({
      console: { log(message) { logs.push(String(message)); } }, __s2pkg_cs2: {},
      ...focusNatives(owner),
      __s2pkg_frame: { OnGameFrame: { subscribe(fn, opts) {
        assert.equal(sealed, false, "frame registration must happen in the load window");
        assert.equal(opts.phase, "pre"); lifecycle.frame.push(fn); return { dispose() {} };
      } } },
      __s2_shared_entity_switch: switches.native(owner),
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
          const e = { index: entities.length + 1, id: entities.length + 1, name: kv.targetname,
            valid: true, isValid() { return this.valid; } };
          entities.push(e); return e;
        },
      },
      __s2pkg_cs2_calls: {
        call: (name) => options.unresolved === name ? null : (...args) => {
          writes.push({ owner, name, args });
          return options.failInvoke === name ? null : undefined;
        },
        status: () => options.unresolved ? "signature unresolved" : "available",
      },
      __s2pkg_server: { Server: { onMapStart: (f) => lifecycle.map.push(f), getCvar: () => "3790153369" } },
      __s2pkg_clients: { _same: (a, b) => !!a && !!b && a.slot === b.slot && a.generation === b.generation, Clients: {
        fromSlot: clientFor,
        all: () => options.notReady ? [] : [{ signonState: 6 }],
        onActive: (f) => lifecycle.active.push(f), onDisconnect: (f) => lifecycle.disconnect.push(f),
      } },
      __s2pkg_timers: { after: (_ms, fn) => pendingTimers.push(fn) },
      __s2pkg_menu: { Menu: { registerRenderer(name, renderer) {
        const prev = renderers[name]; renderers[name] = renderer; return prev;
      } }, MenuStyle: { Chat: "chat", Center: "center" } },
      __s2_hook_on: (_pkg, _name, fn) => { lifecycle.click.push(fn); return 1; },
    });
    for (const file of ["ui.js", "components.js", "menuhud.js"]) {
      vm.runInContext(readFileSync(join(__dirname, file), "utf8"), ctx);
    }
    const base = ctx.__s2pkg_game_ctx.ui((fn) => {
      assert.equal(sealed, false, "click registration must happen in the load window");
      return fn();
    }, (fn) => fn);
    const rawListeners = []; base.onClicked(view => rawListeners.forEach(fn => fn(view)));
    sealed = true;
    ctx.__s2pkg_cs2.Player = { all: () => [...clientGenerations.keys()].map(slot => ({ slot, ref: { index: slot + 1000, id: clientGenerations.get(slot) } })) };
    const p = { ctx, base, rawListeners, hudkit: ctx.__s2pkg_cs2.hudkit, renderers, lifecycle, fallbackCalls, pendingTimers, logs,
      click(slot, id) { for (const fn of lifecycle.click) fn({ player: { index: slot + 1000, id: clientGenerations.get(slot) }, buttonId: id }); },
      runTimer(index = 0) { const fn = pendingTimers.splice(index, 1)[0]; if (fn) fn(); } };
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
  return {
    owners, writes, plugins, plugin, session, dispatchClick, client: clientFor, focus, focusCalls,
    activationCalls, owned,
    frame() {
      for (const r of focus.values()) if (r.state === "waiting" && live(r) && winner(r) === r) r.state = "ready";
      for (const p of plugins) p.lifecycle.frame.forEach(fn => fn());
    },
    unload(p) {
      const owner = plugins.indexOf(p);
      for (const r of [...focus.values()]) if (r.owner === owner) releaseFocus(owner, r.token);
      for (const r of [...owned.values()]) if (r.owner === owner) releaseOwned(owner, r.token);
      p.lifecycle.frame.length = 0; p.lifecycle.click.length = 0;
    },
    replace(slot) { connect(slot); switches.clearSlot(slot); },
    mapChange() {
      focus.clear(); owned.clear(); switches.clear();
      for (const entity of entities) entity.valid = false;
      for (const p of plugins) p.lifecycle.map.forEach(fn => fn());
      for (const p of plugins) p.lifecycle.active.forEach(fn => fn(clientFor(1)));
    },
    replaceLayoutEntity() {
      const old = entities.find(e => e.isValid());
      if (old) old.valid = false;
      const e = { index: entities.length + 1, id: entities.length + 1,
        name: old && old.name, valid: true, isValid() { return this.valid; } };
      entities.push(e);
      return e;
    },
    disconnect(slot) {
      const client = clientFor(slot);
      clientGenerations.delete(slot); switches.clearSlot(slot);
      for (const p of plugins) p.lifecycle.disconnect.forEach(fn => fn(client));
    },
  };
}

test("legacy and explicit singleton ownership arbitrate across plugin contexts", () => {
  const w = pluginWorld();
  const a = w.plugin(), b = w.plugin();
  assert.equal(a.hudkit.banner(1, { text: "legacy", holdSeconds: 0 }), null);
  assert.equal(b.hudkit.forSlot(1).tryOwnBanner({ text: "blocked", holdSeconds: 0 }).error.code, "Busy");
  a.hudkit.hideAll(1);
  const owned = b.hudkit.forSlot(1).tryOwnBanner({ text: "owned", holdSeconds: 0 });
  assert.equal(owned.ok, true);
  assert.match(a.hudkit.banner(1, { text: "blocked legacy", holdSeconds: 0 }), /busy/i);
  assert.equal(owned.value.isValid(), true);
  owned.value.dispose(); owned.value.dispose();
  assert.equal(a.hudkit.banner(1, { text: "after release", holdSeconds: 0 }), null);
});

test("toast ownership has four per-client lanes, reuses holes, and isolates players", () => {
  const w = pluginWorld();
  const a = w.plugin(), b = w.plugin();
  const handles = [];
  for (let i = 0; i < 4; i++) handles.push(a.hudkit.forSlot(1)
    .tryOwnToast({ title: String(i), holdSeconds: 0 }).value);
  assert.equal(b.hudkit.forSlot(1).tryOwnToast({ title: "full", holdSeconds: 0 }).error.code, "Busy");
  assert.equal(b.hudkit.forSlot(2).tryOwnToast({ title: "other", holdSeconds: 0 }).ok, true);
  handles[1].dispose();
  const reused = b.hudkit.forSlot(1).tryOwnToast({ title: "hole", holdSeconds: 0 });
  assert.equal(reused.ok, true);
  assert.equal([...w.owned.values()].filter(r => r.slot === 1).some(r => r.lane === 1 && r.owner === 1), true);
});

test("stale transient timers and disposers cannot retire a replacement owner", () => {
  const w = pluginWorld();
  const a = w.plugin(), b = w.plugin();
  const first = a.hudkit.forSlot(1).tryOwnCallout({ message: "old", holdSeconds: 1 }).value;
  first.dispose();
  const replacement = b.hudkit.forSlot(1).tryOwnCallout({ message: "new", holdSeconds: 0 }).value;
  first.dispose();
  a.runTimer();
  assert.equal(replacement.isValid(), true);
  w.unload(b);
  assert.equal(a.hudkit.forSlot(1).tryOwnCallout({ message: "reclaimed", holdSeconds: 0 }).ok, true);
});

test("focused legacy MOTD replacement cascades its linked child before repaint", () => {
  const w = pluginWorld();
  const a = w.plugin(), b = w.plugin();
  const first = a.hudkit.motd(1, { title: "first", focus: { mode: "exclusive" } });
  assert.equal(first.isValid(), true);
  const second = a.hudkit.motd(1, { title: "second", focus: { mode: "exclusive" } });
  assert.equal(first.isValid(), false);
  assert.equal(second.isValid(), true);
  assert.equal(w.focus.size, 1);
  assert.equal(b.hudkit.forSlot(1).tryOwnMotd({ title: "blocked" }).error.code, "Busy");
});

test("hideAll preserves explicit owned surfaces", () => {
  const w = pluginWorld();
  const a = w.plugin();
  const owned = a.hudkit.forSlot(1).tryOwnBanner({ text: "owned", holdSeconds: 0 }).value;
  a.hudkit.callout(1, { message: "legacy", holdSeconds: 0 });
  a.hudkit.hideAll(1);
  assert.equal(owned.isValid(), true);
  assert.equal([...w.owned.values()].some(r => r.mode === "legacy" && r.slot === 1), false);
});

test("owned dashboards claim per player, fence callbacks, and activate parent before linked focus", () => {
  const w = pluginWorld();
  const a = w.plugin(), b = w.plugin();
  let aReads = 0, aPicks = 0, bReads = 0, bPicks = 0;
  const specA = { title: "A", tabs: [{ id: "a", title: "A" }],
    rows: () => { aReads++; return [{ id: "a-row", a: "A" }]; }, onPick: () => aPicks++ };
  const specB = { title: "B", tabs: [{ id: "b", title: "B" }],
    rows: () => { bReads++; return [{ id: "b-row", a: "B" }]; }, onPick: () => bPicks++ };
  const ownedA = a.hudkit.tryOwnDashboard(specA).value;
  const ownedB = b.hudkit.tryOwnDashboard(specB).value;
  assert.equal(w.owned.size, 0, "controller construction is not a player claim");

  const openedA = ownedA.tryOpenResult(1, { focus: { mode: "exclusive", priority: 4 } });
  assert.equal(openedA.ok, true);
  assert.equal(ownedB.tryOpenResult(1).error.code, "Busy");
  assert.equal(bReads, 0, "content providers run only after reservation");
  const parent = [...w.owned.values()][0];
  const child = [...w.focus.values()][0];
  assert.deepEqual(parent.adapter, {});
  assert.equal(child.parentToken, parent.token);
  assert.deepEqual(child.adapter, {
    capture: { call: "setInputCaptureEnabledForPlayer", token: "panel:s2_dash" },
    suspend: { call: "setHasClassForPlayer", args: ["s2_dash", "s2-hide", 1] },
  });
  assert.deepEqual(w.activationCalls.slice(-2), [parent.token, child.token]);
  w.dispatchClick(1, "s2_dash_r0");
  assert.equal(aPicks, 1); assert.equal(bPicks, 0);
  a.hudkit.hideAll(1);
  assert.equal(openedA.value.isOpen(), true);

  ownedA.dispose(); ownedA.dispose();
  assert.equal(openedA.value.isValid(), false);
  assert.equal(ownedA.tryOpenResult(1).error.code, "Released");
  assert.equal(ownedB.tryOpenResult(1).ok, true);
  w.dispatchClick(1, "s2_dash_r0");
  assert.equal(aPicks, 1); assert.equal(bPicks, 1);
  assert.equal(aReads, 1); assert.equal(bReads, 1);
});

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
  w.disconnect(1);
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

test("retained hudkit views reject a same-slot same-Steam replacement client", () => {
  const w = pluginWorld(), p = w.plugin();
  let modalPicks = 0, dashPicks = 0, motdCloses = 0;
  const modal = p.base.kit.modal({ rows: [{ id: "modal", a: "Modal" }], onPick: () => modalPicks++ });
  const dashboard = p.base.kit.dashboard({ title: "Dash", tabs: [{ id: "tab", title: "Tab" }],
    rows: () => [{ id: "dash", a: "Dash" }], onPick: () => dashPicks++ });
  const badge = p.base.kit.badge({ title: "Badge" });
  const oldModal = modal.open(1);
  const oldDashboard = dashboard.open(1);
  const oldBadge = badge.show(1, { text: "old" });
  const oldMotd = p.base.kit.motd(1, { title: "Rules", onClose: () => motdCloses++ });
  const oldKit = p.base.kit.forSlot(1);
  const oldClient = w.client(1);
  assert.equal(oldClient.steamId, "76561198000000000");
  for (const view of [oldModal, oldDashboard, oldBadge, oldMotd, oldKit]) assert.equal(view.isValid(), true);

  w.replace(1);
  assert.equal(w.client(1).steamId, oldClient.steamId, "Steam identity deliberately stays the same");
  assert.notEqual(w.client(1).generation, oldClient.generation);
  for (const view of [oldModal, oldDashboard, oldBadge, oldMotd, oldKit]) assert.equal(view.isValid(), false);

  w.writes.length = 0;
  oldModal.close(); oldModal.refresh(); oldModal.page(1); oldModal.select(0); oldModal.forget();
  assert.equal(oldModal.isOpen(), false); assert.equal(oldModal.cursor(), -1);
  assert.equal(oldModal.tryOpen().ok, false);
  assert.throws(() => oldModal.open(), /stale/i);
  oldDashboard.close(); oldDashboard.setTab("tab"); oldDashboard.refresh();
  assert.equal(oldDashboard.isOpen(), false);
  assert.throws(() => oldDashboard.open(), /stale/i);
  oldBadge.show({ text: "wrong" }); oldBadge.hide(); oldMotd.close();
  oldKit.toast({ title: "wrong" }); oldKit.callout({ message: "wrong" }); oldKit.banner({ text: "wrong" });
  oldKit.motd({ title: "wrong" }); oldKit.hideAll(); oldKit.forget();
  p.click(1, "s2_m0_r0"); p.click(1, "s2_dash_r0"); p.click(1, "s2_motd_ok");
  assert.equal(w.writes.length, 0, "no stale operation may touch the replacement client");
  assert.deepEqual([modalPicks, dashPicks, motdCloses], [0, 0, 0],
    "stale dispatch returns before any domain callback");

  assert.equal(modal.open(1).isValid(), true, "slot-first APIs adopt the current occupant");
  assert.equal(dashboard.open(1).isValid(), true);
  assert.equal(badge.show(1, { text: "fresh" }).isValid(), true);
  assert.equal(p.base.kit.motd(1, { title: "fresh" }).isValid(), true);
  assert.equal(p.base.kit.forSlot(1).isValid(), true);
});

test("component views remain reusable across ordinary close and reopen for the same client", () => {
  const w = pluginWorld(), p = w.plugin();
  const modal = p.base.kit.modal({ rows: [{ a: "row" }] });
  const oldModal = modal.open(1); modal.close(1); const newModal = modal.open(1);
  const dashboard = p.base.kit.dashboard({ title: "Dash", tabs: [{ id: "t", title: "T" }], rows: () => [] });
  const oldDashboard = dashboard.open(1); dashboard.close(1); const newDashboard = dashboard.open(1);
  const badge = p.base.kit.badge();
  const oldBadge = badge.show(1, { text: "one" }); badge.hide(1); const newBadge = badge.show(1, { text: "two" });
  const oldMotd = p.base.kit.motd(1, { title: "one" });
  const newMotd = p.base.kit.motd(1, { title: "two" });
  assert.equal(oldModal.isValid(), true); assert.equal(oldDashboard.isValid(), true);
  assert.equal(oldBadge.isValid(), true); assert.equal(oldMotd.isValid(), false);
  oldModal.close(); oldModal.open();
  oldDashboard.close(); oldDashboard.open();
  oldBadge.hide(); oldBadge.show({ text: "three" });
  const beforeStaleMotd = w.writes.length;
  oldMotd.close();
  assert.equal(w.writes.length, beforeStaleMotd, "an old one-shot MOTD close cannot close its replacement");
  for (const view of [newModal, newDashboard, newBadge, newMotd]) assert.equal(view.isValid(), true);
});

test("delayed fade callbacks are fenced by client and component lifetimes", () => {
  const w = pluginWorld(), p = w.plugin();
  p.base.kit.banner(1, { text: "old", holdSeconds: 1 });
  p.runTimer(); // The old hold adds the fade and schedules its final hide.
  p.base.kit.banner(1, { text: "new", holdSeconds: 0 });
  const beforeReopenHide = w.writes.length;
  p.runTimer();
  assert.equal(w.writes.length, beforeReopenHide, "old final hide cannot touch a reopened banner");

  p.base.kit.callout(1, { message: "departing", holdSeconds: 1 });
  w.replace(1);
  const beforeReplacementFade = w.writes.length;
  p.runTimer();
  assert.equal(w.writes.length, beforeReplacementFade, "old hold cannot fade a replacement client's callout");
  p.base.kit.callout(1, { message: "replacement", holdSeconds: 0 });
});

test("released badge views cannot hide a pool slot reclaimed by another plugin", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const claimed = a.base.kit.badge();
  const stale = claimed.show(1, { text: "A" });
  claimed.release();
  const replacement = b.base.kit.badge();
  replacement.show(1, { text: "B" });
  const before = w.writes.length;
  stale.hide(); stale.show({ text: "wrong" });
  assert.equal(w.writes.length, before);
  assert.equal(stale.isValid(), false);
});

test("forget and silent layout replacement invalidate retained component views", () => {
  const w = pluginWorld(), p = w.plugin();
  const modal = p.base.kit.modal({ rows: [{ a: "row" }] });
  const dashboard = p.base.kit.dashboard({ title: "Dash", tabs: [{ id: "t", title: "T" }], rows: () => [] });
  const badge = p.base.kit.badge();
  const forgotten = [modal.open(1), dashboard.open(1), badge.show(1), p.base.kit.motd(1, { title: "M" }),
    p.base.kit.forSlot(1)];
  p.base.kit.forget(1);
  for (const view of forgotten) assert.equal(view.isValid(), false);
  const afterForget = w.writes.length;
  forgotten[0].refresh(); forgotten[1].refresh(); forgotten[2].show({ text: "wrong" }); forgotten[3].close();
  forgotten[4].hideAll();
  assert.equal(w.writes.length, afterForget);

  const replaced = [modal.open(1), dashboard.open(1), badge.show(1), p.base.kit.motd(1, { title: "M2" }),
    p.base.kit.forSlot(1)];
  w.replaceLayoutEntity();
  for (const view of replaced) assert.equal(view.isValid(), false);
  const afterReplace = w.writes.length;
  replaced[0].refresh(); replaced[1].refresh(); replaced[2].hide(); replaced[3].close(); replaced[4].hideAll();
  assert.equal(w.writes.length, afterReplace, "stale component views cannot drive a replacement entity");
  assert.equal(modal.open(1).isValid(), true, "a slot-first open adopts the replacement layout lifetime");
});

test("dashboard spec replacement invalidates retained dashboard views", () => {
  const w = pluginWorld(), p = w.plugin();
  const dashboard = p.base.kit.dashboard({ title: "one", tabs: [{ id: "a", title: "A" }], rows: () => [] });
  const stale = dashboard.open(1);
  p.base.kit.dashboard({ title: "two", tabs: [{ id: "b", title: "B" }], rows: () => [] });
  assert.equal(stale.isValid(), false);
  const before = w.writes.length;
  stale.close(); stale.refresh(); stale.setTab("a");
  assert.throws(() => stale.open(), /stale/i);
  assert.equal(w.writes.length, before);
  assert.equal(dashboard.forSlot(1).isValid(), true);
});

test("released modal views cannot act on a pool slot reclaimed by another plugin", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const owner = a.base.kit.modal({ rows: [{ a: "A" }] });
  const stale = owner.open(1);
  owner.release();
  const replacement = b.base.kit.modal({ rows: [{ a: "B" }] });
  replacement.open(1);
  const before = w.writes.length;
  stale.close(); stale.refresh(); stale.page(1); stale.select(0); stale.forget();
  assert.equal(stale.tryOpen().ok, false);
  assert.throws(() => stale.open(), /stale|released/i);
  assert.equal(w.writes.length, before);
});

test("old plugin timers cannot alter a surface after cleanup and reload", () => {
  const w = pluginWorld(), oldPlugin = w.plugin();
  oldPlugin.base.kit.banner(1, { text: "old", holdSeconds: 1 });
  oldPlugin.base.kit.forget(1);
  const replacement = w.plugin();
  replacement.base.kit.banner(1, { text: "new", holdSeconds: 0 });
  const before = w.writes.length;
  oldPlugin.runTimer();
  assert.equal(w.writes.length, before);
});

test("a component operation cannot adopt a replacement created during argument coercion", () => {
  const w = pluginWorld(), p = w.plugin();
  const text = { toString() { w.replace(1); return "replacement must not see this"; } };
  const before = w.writes.length;
  p.base.kit.banner(1, { text, holdSeconds: 0 });
  assert.equal(w.writes.length, before);

  const dashboard = p.base.kit.dashboard({
    title: "Dashboard",
    tabs: [{ id: "one", title: "One" }],
    rows: () => []
  });
  const retained = dashboard.forSlot(1);
  const opts = { get tab() { w.replace(1); return "one"; } };
  assert.throws(() => retained.open(opts), /stale/i);
  assert.equal(retained.isValid(), false);
  assert.equal(w.writes.length, before);
});

test("a released badge cannot resume painting after its pool slot is reclaimed during coercion", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const owner = a.base.kit.badge();
  const retained = owner.show(1, { text: "A" });
  let boundary = -1;
  retained.show({
    title: { toString() {
      owner.release();
      b.base.kit.badge().show(1, { title: "B", text: "B" });
      boundary = w.writes.length;
      return "STALE";
    } },
    text: "STALE BODY"
  });
  assert.notEqual(boundary, -1);
  assert.equal(retained.isValid(), false);
  assert.deepEqual(w.writes.slice(boundary), []);
});

test("a fresh same-plugin badge binds independently of the released badge coercing its title", () => {
  const w = pluginWorld(), p = w.plugin();
  const owner = p.base.kit.badge();
  const retained = owner.show(1, { text: "A" });
  let replacement, beforeReplacement = -1, afterReplacement = -1;
  retained.show({
    title: { toString() {
      owner.release();
      beforeReplacement = w.writes.length;
      replacement = p.base.kit.badge().show(1, { title: "B", text: "B" });
      afterReplacement = w.writes.length;
      return "STALE";
    } },
    text: "STALE BODY"
  });
  assert.notEqual(afterReplacement, -1);
  assert.equal(retained.isValid(), false);
  assert.equal(replacement.isValid(), true);
  assert.ok(w.writes.slice(beforeReplacement, afterReplacement).some(write =>
    write.name === "setDialogVariableStringForPlayer" && write.args[4] === "B"));
  assert.deepEqual(w.writes.slice(afterReplacement), [], "the old badge cannot resume painting");
});

test("an entity replacement during coercion fences the retained component before its first write", () => {
  const w = pluginWorld(), p = w.plugin();
  const retained = p.base.kit.badge().show(1, { text: "A" });
  let boundary = -1;
  retained.show({ title: { toString() {
    w.replaceLayoutEntity();
    boundary = w.writes.length;
    return "STALE ENTITY";
  } } });
  assert.notEqual(boundary, -1);
  assert.equal(retained.isValid(), false);
  assert.deepEqual(w.writes.slice(boundary), []);
});

test("a superseded retained modal open cannot return a replacement-client view", () => {
  const w = pluginWorld(), p = w.plugin();
  let trigger = false;
  let modal;
  modal = p.base.kit.modal({ rows() {
    if (trigger) {
      trigger = false;
      w.replace(1);
      modal.open(1);
    }
    return [{ a: "row" }];
  } });
  const retained = modal.open(1);
  trigger = true;
  const result = retained.tryOpen();
  const beforeClose = w.writes.length;
  if (result.ok) result.view.close();
  assert.deepEqual({ ok: result.ok, closeWrites: w.writes.length - beforeClose },
    { ok: false, closeWrites: 0 });
  assert.equal(retained.isValid(), false);
  assert.equal(modal.isOpen(1), true, "the replacement presentation remains open");
});

test("a same-client modal opened during coercion remains the authoritative presentation", () => {
  const w = pluginWorld(), p = w.plugin();
  let trigger = false;
  let modal;
  const label = { toString() {
    if (trigger) {
      trigger = false;
      modal.open(1);
    }
    return "row";
  } };
  modal = p.base.kit.modal({ rows: () => [{ a: label }] });
  const retained = modal.open(1);
  trigger = true;
  const result = retained.tryOpen();
  assert.equal(result.ok, true);
  assert.equal(result.ok && result.view.isValid(), true);
  assert.equal(modal.isOpen(1), true);
});

test("a modal click does not evaluate providers after its client is replaced", () => {
  const w = pluginWorld(), p = w.plugin();
  let calls = 0;
  const modal = p.base.kit.modal({
    rows() { calls++; return [{ a: "row" }]; },
    onPick() { w.replace(1); }
  });
  modal.open(1);
  const before = calls;
  p.click(1, "s2_m0_r0");
  assert.equal(calls - before, 0);
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

test("structured modal and badge factories report pool exhaustion and recover after release", () => {
  const w = pluginWorld(), p = w.plugin();
  const badModalSpec = {};
  Object.defineProperty(badModalSpec, "pageSize", { get() { throw new Error("bad modal spec"); } });
  assert.deepEqual(plain(p.hudkit.tryModal(badModalSpec)), {
    ok: false, error: { code: "InvalidArgument", message: "bad modal spec" },
  });
  assert.ok(w.owners.every(owner => owner === null), "failed structured construction releases its modal claim");
  const modals = Array.from({ length: 6 }, () => p.hudkit.tryModal({ rows: [] }));
  assert.ok(modals.every(result => result.ok));
  assert.deepEqual(plain(p.hudkit.tryModal({ rows: [] })), {
    ok: false,
    error: { code: "PoolExhausted", message: "hudkit: modal pool exhausted" },
  });
  modals[0].value.release();
  const recoveredModal = p.hudkit.tryModal({ rows: [] });
  assert.equal(recoveredModal.ok, true);
  for (const result of modals.slice(1)) result.value.release();
  recoveredModal.value.release();

  const badBadgeSpec = {};
  Object.defineProperty(badBadgeSpec, "corner", { get() { throw new Error("bad badge spec"); } });
  assert.deepEqual(plain(p.hudkit.tryBadge(badBadgeSpec)), {
    ok: false, error: { code: "InvalidArgument", message: "bad badge spec" },
  });
  assert.ok(w.owners.every(owner => owner === null), "failed structured construction releases its badge claim");
  const badges = Array.from({ length: 4 }, () => p.hudkit.tryBadge());
  assert.ok(badges.every(result => result.ok));
  assert.deepEqual(plain(p.hudkit.tryBadge()), {
    ok: false,
    error: { code: "PoolExhausted", message: "hudkit: badge pool exhausted" },
  });
  badges[0].value.release();
  assert.equal(p.hudkit.tryBadge().ok, true);
});

test("structured component operations distinguish stale and released lifetimes", () => {
  const w = pluginWorld(), p = w.plugin();
  const modal = p.hudkit.tryModal({ rows: [] }).value;
  const modalView = modal.tryOpenResult(1).value;
  const dashboard = p.hudkit.dashboard({ title: "Dash", tabs: [], rows: () => [] });
  const dashView = dashboard.tryOpenResult(1).value;
  const badge = p.hudkit.tryBadge().value;
  const badgeView = badge.show(1, { text: "before" });

  w.replace(1);
  const beforeStale = w.writes.length;
  assert.deepEqual(plain(modalView.tryOpenResult()), {
    ok: false, error: { code: "StaleClient", message: "hudkit: stale client or component" },
  });
  assert.deepEqual(plain(modalView.tryRefresh()), {
    ok: false, error: { code: "StaleClient", message: "hudkit: stale client or component" },
  });
  assert.deepEqual(plain(dashView.tryOpenResult()), {
    ok: false, error: { code: "StaleClient", message: "hudkit: stale client or component" },
  });
  assert.deepEqual(plain(dashView.tryRefresh()), {
    ok: false, error: { code: "StaleClient", message: "hudkit: stale client or component" },
  });
  assert.deepEqual(plain(badgeView.tryShow({ text: "wrong" })), {
    ok: false, error: { code: "StaleClient", message: "hudkit: stale client or component" },
  });
  assert.equal(w.writes.length, beforeStale);

  const currentModalView = modal.tryOpenResult(1).value;
  const currentBadgeView = badge.show(1, { text: "current" });
  modal.release();
  badge.release();
  const beforeReleased = w.writes.length;
  assert.deepEqual(plain(currentModalView.tryOpenResult()), {
    ok: false, error: { code: "Released", message: "hudkit: modal has been released" },
  });
  assert.deepEqual(plain(currentModalView.tryRefresh()), {
    ok: false, error: { code: "Released", message: "hudkit: modal has been released" },
  });
  assert.deepEqual(plain(currentBadgeView.tryShow({ text: "wrong" })), {
    ok: false, error: { code: "Released", message: "hudkit: badge has been released" },
  });
  assert.equal(w.writes.length, beforeReleased);
});

test("structured opens classify missing clients, unavailable bindings, and partial paint failures", () => {
  const pendingWorld = pluginWorld({ notReady: true }), pendingPlugin = pendingWorld.plugin();
  const pendingModal = pendingPlugin.hudkit.tryModal({ title: "Pending" }).value;
  assert.equal(pendingModal.tryOpenResult(2).error.code, "NotReady");
  pendingModal.release();

  const missingClientWorld = pluginWorld(), missingClientPlugin = missingClientWorld.plugin();
  missingClientWorld.disconnect(7);
  const missingClientModal = missingClientPlugin.hudkit.tryModal({ rows: [] }).value;
  assert.deepEqual(plain(missingClientModal.tryOpenResult(7)), {
    ok: false, error: { code: "StaleClient", message: "hudkit: stale client or component" },
  });

  const unavailableWorld = pluginWorld({ unresolved: "setDialogVariableStringForPlayer" });
  const unavailablePlugin = unavailableWorld.plugin();
  const unavailableModal = unavailablePlugin.hudkit.tryModal({ title: "Unavailable" }).value;
  assert.deepEqual(plain(unavailableModal.tryOpenResult(2)), {
    ok: false,
    error: {
      code: "Unavailable",
      message: "unavailable: signature unresolved",
    },
  });
  assert.equal(unavailableWorld.writes.some(call =>
    call.name === "setInputCaptureEnabledForPlayer" && call.args[2] === true), false);
  unavailableModal.release();
  assert.ok(unavailableWorld.owners.every(owner => owner === null));

  const options = {};
  const partialWorld = pluginWorld(options), partialPlugin = partialWorld.plugin();
  const partialModal = partialPlugin.hudkit.tryModal({ title: "Partial", rows: [{ a: "Row" }] }).value;
  options.failInvoke = "setDialogVariableStringForPlayer";
  assert.deepEqual(plain(partialModal.tryOpenResult(2)), {
    ok: false,
    error: {
      code: "PaintFailed",
      message: "setDialogVariableStringForPlayer: engine invocation failed",
    },
  });
  assert.equal(partialModal.isOpen(2), false);
  assert.equal(partialWorld.writes.some(call =>
    call.name === "setInputCaptureEnabledForPlayer" && call.args[2] === true), false);
  partialModal.release();
  assert.ok(partialWorld.owners.every(owner => owner === null));

  const dashOptions = { failInvoke: "setDialogVariableStringForPlayer" };
  const dashWorld = pluginWorld(dashOptions), dashPlugin = dashWorld.plugin();
  const dashboard = dashPlugin.hudkit.dashboard({ title: "Partial dashboard",
    tabs: [{ id: "main", title: "Main" }], rows: () => [] });
  assert.equal(dashboard.tryOpenResult(2).error.code, "PaintFailed");
  assert.equal(dashboard.isOpen(2), false);
  assert.equal(dashWorld.writes.some(call =>
    call.name === "setInputCaptureEnabledForPlayer" && call.args[2] === true), false);

  const badgeWorld = pluginWorld(), badgePlugin = badgeWorld.plugin();
  const badge = badgePlugin.hudkit.tryBadge().value;
  const badgeView = badge.show(2, { text: "Before" });
  badgeWorld.writes.length = 0;
  const badText = { toString() { throw new Error("badge conversion failed"); } };
  assert.deepEqual(plain(badgeView.tryShow({ text: badText })), {
    ok: false, error: { code: "PaintFailed", message: "badge conversion failed" },
  });
  assert.equal(badgeWorld.writes.length, 0);
});

test("structured refresh requires an open presentation", () => {
  const w = pluginWorld(), p = w.plugin();
  const modal = p.hudkit.tryModal({ rows: [] }).value;
  const dashboard = p.hudkit.dashboard({ title: "Dash", tabs: [], rows: () => [] });
  assert.equal(modal.tryRefresh().error.code, "InvalidArgument");
  assert.equal(dashboard.tryRefresh().error.code, "InvalidArgument");
  assert.equal(modal.tryRefresh(2).error.code, "InvalidArgument");
  assert.equal(dashboard.tryRefresh(2).error.code, "InvalidArgument");
});

test("structured modal and dashboard refresh report partial paint and disable actions until retry", () => {
  const options = {};
  const w = pluginWorld(options), p = w.plugin();
  const picked = [];
  let modalTitle = "Modal one", dashTitle = "Dash one";
  const modal = p.hudkit.tryModal({ title: () => modalTitle, rows: [{ id: "modal", a: "Modal" }],
    onPick: () => picked.push("modal") }).value;
  const dashboard = p.hudkit.dashboard({ title: () => dashTitle, tabs: [{ id: "tab", title: "Tab" }],
    rows: () => [{ id: "dash", a: "Dash" }], onPick: () => picked.push("dash") });
  const modalView = modal.tryOpenResult(2).value;
  const dashView = dashboard.tryOpenResult(2).value;

  modalTitle = "Modal two"; dashTitle = "Dash two";
  options.failInvoke = "setDialogVariableStringForPlayer";
  assert.equal(modal.tryRefresh(2).error.code, "PaintFailed");
  assert.equal(modalView.tryRefresh().error.code, "PaintFailed");
  assert.equal(dashboard.tryRefresh(2).error.code, "PaintFailed");
  assert.equal(dashView.tryRefresh().error.code, "PaintFailed");
  options.failInvoke = null;
  p.click(2, "s2_m0_r0");
  p.click(2, "s2_dash_r0");
  assert.deepEqual(picked, []);
  assert.deepEqual(plain(modal.tryRefresh(2)), { ok: true });
  assert.deepEqual(plain(dashboard.tryRefresh(2)), { ok: true });
  p.click(2, "s2_m0_r0");
  p.click(2, "s2_dash_r0");
  assert.deepEqual(picked, ["modal", "dash"]);
});

test("two plugin modals and a manual cursor token cannot release each other's capture", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const ma = a.base.kit.modal({ rows: [] }), mb = b.base.kit.modal({ rows: [] });
  ma.open(1); mb.open(1);
  a.base.kit.layout.cursor(1, true);
  const captures = () => w.writes.filter(v => v.name === "setInputCaptureEnabledForPlayer");
  assert.equal(captures().length, 1, "all owners share one first-on engine operation");
  ma.setCursor(1, false);
  ma.close(1);
  mb.close(1);
  assert.equal(captures().length, 1, "modal close does not release the manual token");
  a.base.kit.layout.cursor(1, false);
  assert.deepEqual(captures().map(c => c.args[2]), [true, false]);
});

test("turning a modal cursor off releases the root token acquired by open", () => {
  const w = pluginWorld(), p = w.plugin();
  const modal = p.base.kit.modal({ rows: [] });
  modal.open(1); modal.setCursor(1, false);
  const captures = w.writes.filter(v => v.name === "setInputCaptureEnabledForPlayer");
  assert.deepEqual(captures.map(c => c.args[2]), [true, false]);
  assert.equal(modal.isOpen(1), true, "cursor-off keeps the modal painted");
});

test("modal tryOpen before world readiness fails closed and can retry after an active client", () => {
  const w = pluginWorld({ notReady: true });
  const p = w.plugin();
  const modal = p.base.kit.modal({ title: "Ready?" });
  const failed = modal.forSlot(2).tryOpen();
  assert.equal(failed.ok, false);
  assert.match(failed.error, /world not ready/);
  assert.equal(modal.isOpen(2), false);
  assert.equal(w.writes.length, 0);
  assert.throws(() => modal.open(2), /modal.open failed:.*world not ready/);
  p.lifecycle.active.forEach(fn => fn());
  const view = modal.open(2);
  assert.equal(view.slot, 2);
  assert.equal(view.isOpen(), true);
  assert.equal(typeof view.close, "function");
});

test("modal tryOpen exposes an unresolved descriptor and never captures input", () => {
  const w = pluginWorld({ unresolved: "setDialogVariableStringForPlayer" });
  const p = w.plugin();
  const modal = p.base.kit.modal({ title: "Unavailable" });
  const result = modal.tryOpen(2);
  assert.equal(result.ok, false);
  assert.match(result.error, /signature unresolved/);
  assert.equal(modal.isOpen(2), false);
  assert.equal(w.writes.filter(c => c.name === "setInputCaptureEnabledForPlayer").length, 0);
});

test("native invocation rejection does not poison paint caches and retry paints the full title", () => {
  const options = { failInvoke: "setDialogVariableStringForPlayer" };
  const w = pluginWorld(options);
  const p = w.plugin();
  const modal = p.base.kit.modal({ title: "Retry me" });
  const result = modal.tryOpen(2);
  assert.equal(result.ok, false);
  assert.match(result.error, /engine invocation failed/);
  assert.equal(modal.isOpen(2), false);
  assert.equal(w.writes.filter(c => c.name === "setInputCaptureEnabledForPlayer").length, 0);
  options.failInvoke = null;
  const before = w.writes.length;
  const retry = modal.tryOpen(2);
  assert.equal(retry.ok, true);
  assert.equal(retry.view.isOpen(), true);
  assert.ok(w.writes.slice(before).some(c => c.name === "setDialogVariableStringForPlayer" && c.args.includes("Retry me")));
});

test("a failed modal repaint disables rows and footers until a complete repaint succeeds", () => {
  const options = {};
  const w = pluginWorld(options), p = w.plugin();
  let title = "Old", row = { id: "old", a: "Old" }, footer = "old";
  const picked = [];
  const modal = p.base.kit.modal({
    title: () => title,
    rows: () => [row],
    buttons: () => [{ text: "Act", onClick: () => picked.push(footer) }],
    onPick: (_slot, _index, value) => picked.push(value.id),
  });
  modal.open(2);
  title = "New"; row = { id: "new", a: "New" }; footer = "new";
  options.failInvoke = "setDialogVariableStringForPlayer";
  modal.refresh(2);
  options.failInvoke = null;
  p.click(2, "s2_m0_r0"); p.click(2, "s2_m0_f0");
  assert.deepEqual(picked, [], "a partially changed sheet must not retain clickable old actions");
  modal.refresh(2);
  p.click(2, "s2_m0_r0"); p.click(2, "s2_m0_f0");
  assert.deepEqual(picked, ["new", "new"]);
});

test("a failed dashboard repaint disables dispatch until a complete repaint succeeds", () => {
  const options = {};
  const w = pluginWorld(options), p = w.plugin();
  let title = "Old", row = { id: "old", a: "Old" };
  const picked = [];
  const dash = p.base.kit.dashboard({
    title: () => title,
    tabs: [{ id: "tab", title: "Tab" }],
    rows: () => [row],
    onPick: (_slot, tabId, value) => picked.push([tabId, value.id]),
  });
  dash.open(2);
  title = "New"; row = { id: "new", a: "New" };
  options.failInvoke = "setDialogVariableStringForPlayer";
  dash.refresh(2);
  options.failInvoke = null;
  p.click(2, "s2_dash_r0");
  assert.deepEqual(picked, []);
  dash.refresh(2);
  p.click(2, "s2_dash_r0");
  assert.deepEqual(picked, [["tab", "new"]]);
});


test("capture rejection rolls modal open back and retry reacquires instead of retaining a false lease", () => {
  const options = { captureError: "capture invocation failed" };
  const w = pluginWorld(options), p = w.plugin();
  const modal = p.base.kit.modal({ title: "Capture retry" });
  const failed = modal.tryOpen(2);
  assert.equal(failed.ok, false);
  assert.match(failed.error, /capture invocation failed/);
  assert.equal(modal.isOpen(2), false);
  assert.ok(w.writes.some(c => c.name === "setHasClassForPlayer" &&
    c.args[2] === "s2_m0" && c.args[3] === "s2-hide" && c.args[4] === 1));
  options.captureError = null;
  assert.equal(modal.tryOpen(2).ok, true);
  modal.close(2);
  assert.deepEqual(w.writes.filter(c => c.name === "setInputCaptureEnabledForPlayer")
    .map(c => c.args[2]), [true, true, false]);
});

const exclusive = priority => ({ focus: { mode: "exclusive", priority } });
function focusedModal(p, pick, rows = () => [{ id: "one", a: "One" }]) {
  return p.hudkit.modal({ title: "Focus", rows, onPick: pick });
}
test("exclusive focus suspends other plugins before painting and restores after a host frame", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  let aReads = 0, bReads = 0, aPicks = 0, bPicks = 0;
  const am = focusedModal(a, () => aPicks++, () => { aReads++; return [{ a: "A" }]; });
  const bm = focusedModal(b, () => bPicks++, () => { bReads++; return [{ a: "B" }]; });
  assert.equal(am.tryOpenResult(1, exclusive(2)).ok, true);
  const before = w.writes.length;
  assert.equal(bm.tryOpenResult(1, exclusive(1)).ok, true);
  assert.equal(bReads, 0, "covered opens must not evaluate providers");
  assert.equal(w.writes.length, before);
  w.dispatchClick(1, "s2_m1_r0"); assert.equal(bPicks, 0);
  bm.open(1, { ...exclusive(3), cursor: false });
  w.dispatchClick(1, "s2_m0_r0"); assert.equal(aPicks, 0);
  assert.ok(w.writes.some(x => x.host && x.args[2] === "s2_m0"));
  assert.equal(w.writes.filter(x => x.name === "setInputCaptureEnabledForPlayer").at(-1).args[2], false);
  bm.close(1);
  const reads = aReads;
  w.dispatchClick(1, "s2_m0_r0"); assert.equal(aReads, reads); assert.equal(aPicks, 0);
  w.frame(); assert.equal(aReads, reads + 1);
  w.dispatchClick(1, "s2_m0_r0"); assert.equal(aPicks, 1);
});
test("focus priority validates before replacing or evaluating a presentation", () => {
  const w = pluginWorld(), p = w.plugin(); let reads = 0;
  const m = focusedModal(p, () => {}, () => { reads++; return []; });
  for (const priority of [NaN, Infinity, -Infinity, 0.5, 2147483648, -2147483649, "1", null]) {
    const result = m.tryOpenResult(1, exclusive(priority));
    assert.equal(result.ok, false); assert.equal(result.error.code, "InvalidArgument");
  }
  assert.equal(reads, 0); assert.equal(w.focusCalls.length, 0);
  m.open(1, { focus: { mode: "exclusive" } });
  assert.equal(w.focusCalls[0].priority, 0);
  assert.equal(w.focusCalls[0].surface, "cs2:hudkit:exclusive");
});
test("legacy dashboard replacement retires its linked child before MOTD focus restoration", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin(); let reads = 0;
  const spec = { title: "Dash", tabs: [{ id: "t", title: "T" }], rows: () => { reads++; return [{ id: "r", a: "R" }]; } };
  const ad = a.hudkit.dashboard(spec), bd = b.hudkit.dashboard(spec);
  ad.open(1, exclusive(2)); bd.open(1, exclusive(1)); assert.equal(reads, 2);
  const before = w.writes.length; ad.close(1); assert.equal(w.writes.length, before);
  const motd = b.hudkit.motd(1, { title: "Rules", ...exclusive(3) });
  assert.equal(motd.isValid(), true); assert.equal(w.focus.size, 2);
  motd.close(); w.frame(); assert.equal(reads, 3);
});

test("focused failures retire exact tokens, disable actions and require explicit retry", () => {
  for (const component of ["modal", "dashboard", "motd"]) {
    const options = {}, w = pluginWorld(options), p = w.plugin(); let picks = 0, reads = 0;
    const m = component === "modal" ? focusedModal(p, () => picks++, () => { reads++; return [{ a: "Row" }]; }) :
      component === "dashboard" ? p.hudkit.dashboard({ title: "D", tabs: [{ id: "t", title: "T" }],
        rows: () => { reads++; return [{ id: "r", a: "Row" }]; }, onPick: () => picks++ }) : null;
    options.activateError = true;
    if (m) assert.equal(m.tryOpenResult(1, exclusive(0)).ok, false);
    else assert.equal(p.hudkit.motd(1, { title: "M", ...exclusive(0) }).isValid(), false);
    assert.equal(w.focus.size, 0);
    options.activateError = false;
    if (!m) continue;
    m.open(1, exclusive(0)); options.failInvoke = "setDialogVariableStringForPlayer";
    // Force a noncached text submission on the refresh.
    p.base.kit.layout._focus.invalidate(p.base.kit.layout._captureBinding(1), component === "modal" ? "s2_m0" : "s2_dash");
    assert.equal(m.tryRefresh(1).ok, false);
    assert.equal(w.focus.size, 0);
    const before = reads; w.frame(); assert.equal(reads, before);
    w.dispatchClick(1, component === "modal" ? "s2_m0_r0" : "s2_dash_r0"); assert.equal(picks, 0);
    options.failInvoke = null; assert.equal(m.tryRefresh(1).ok, true); assert.equal(w.focus.size, 1);
  }
});

test("restoration clears unchanged cached values even if coverage was never polled", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const am = focusedModal(a, () => {}), bm = focusedModal(b, () => {});
  am.open(1, exclusive(0)); bm.open(1, exclusive(0)); bm.close(1);
  const before = w.writes.length;
  w.frame();
  assert.ok(w.writes.slice(before).some(x => x.name === "setHasClassForPlayer" && x.args[2] === "s2_m0" && x.args[4] === 0));
  assert.ok(w.writes.slice(before).some(x => x.name === "setDialogVariableStringForPlayer" && x.args.includes("Focus")));
});

test("covered cursor intent is remembered without acquiring and exact close preserves same-root winner", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const am = focusedModal(a, () => {}), bm = focusedModal(b, () => {});
  am.open(1, exclusive(0)); bm.open(1, exclusive(1));
  const before = w.writes.length; am.setCursor(1, false); assert.equal(w.writes.length, before);
  bm.close(1); w.frame();
  assert.equal(w.writes.filter(x => x.name === "setInputCaptureEnabledForPlayer").at(-1).args[2], false);
  am.setCursor(1, true);
  assert.equal(w.writes.filter(x => x.name === "setInputCaptureEnabledForPlayer").at(-1).args[2], true);
});

test("focus binding blocks a takeover triggered by value coercion before the obsolete engine write", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const bm = focusedModal(b, () => {}); let armed = false;
  const am = a.hudkit.modal({ title: () => armed ? { toString() {
    armed = false; bm.open(1, exclusive(1)); return "obsolete";
  } } : "Initial", rows: [] });
  am.open(1, exclusive(0)); armed = true;
  assert.equal(am.tryRefresh(1).ok, false);
  assert.equal(w.writes.some(x => x.args.includes("obsolete")), false);
});

test("focus failure codes pass through and stale lifetimes do not repaint during frame reconciliation", () => {
  for (const code of ["InvalidArgument", "StaleClient", "NotReady", "Unavailable", "Released", "Busy", "PoolExhausted"]) {
    const w = pluginWorld({ focusError: code }), p = w.plugin(); let reads = 0;
    const m = focusedModal(p, () => {}, () => { reads++; return []; });
    assert.equal(m.tryOpenResult(1, exclusive(0)).error.code, code); assert.equal(reads, 0);
  }
  for (const change of [w => w.replace(1), w => w.replaceLayoutEntity(), w => w.disconnect(1), w => w.mapChange()]) {
    const w = pluginWorld(), a = w.plugin(), b = w.plugin(); let reads = 0;
    const am = focusedModal(a, () => {}, () => { reads++; return []; }), bm = focusedModal(b, () => {});
    const old = am.open(1, exclusive(0)); bm.open(1, exclusive(1)); change(w);
    const before = w.writes.length; w.frame(); old.refresh(); old.close();
    assert.equal(reads, 1); assert.equal(w.writes.length, before); assert.equal(old.isValid(), false);
  }
});

test("native activation fences the outer and nested delivery while raw observers still observe", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin(); let bPicks = 0, raw = 0;
  const bd = b.hudkit.dashboard({ title: "B", tabs: [{ id: "t", title: "T" }],
    rows: () => [{ id: "b", a: "B" }], onPick: () => bPicks++ });
  let transfer = true;
  const ad = a.hudkit.dashboard({ title: "A", tabs: [{ id: "t", title: "T" }],
    rows: () => [{ id: "a", a: "A" }], onPick: () => {
      if (transfer) { transfer = false; bd.open(1, exclusive(1)); w.dispatchClick(1, "s2_dash_r0"); }
    } });
  a.rawListeners.push(() => raw++); b.rawListeners.push(() => raw++);
  ad.open(1, exclusive(0)); w.dispatchClick(1, "s2_dash_r0");
  assert.equal(bPicks, 0); assert.equal(raw, 4);
  w.dispatchClick(1, "s2_dash_r0"); assert.equal(bPicks, 1); assert.equal(raw, 6);
});
test("unloading retired legacy dashboard contexts cannot alter their replacement", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin(), c = w.plugin();
  const spec = { title: "D", tabs: [{ id: "t", title: "T" }], rows: () => [] };
  const ad = a.hudkit.dashboard(spec), bd = b.hudkit.dashboard(spec), cd = c.hudkit.dashboard(spec);
  ad.open(1, exclusive(0)); bd.open(1, exclusive(1)); cd.open(1, exclusive(-1));
  const before = w.writes.length;
  w.unload(a); w.unload(b);
  assert.equal(w.writes.length, before);
  assert.equal(w.focus.size, 1);
  w.unload(c);
  assert.equal(w.focus.size, 0);
});
test("a replaced dashboard context drops queued work before provider evaluation", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  let aReads = 0, bReads = 0;
  const ad = a.hudkit.dashboard({ title: "A", tabs: [{ id: "a", title: "A" }],
    rows: () => { aReads++; return []; } });
  const bd = b.hudkit.dashboard({ title: "B", tabs: [{ id: "b", title: "B" }],
    rows: () => { bReads++; return []; } });
  ad.open(1, exclusive(0));
  ad.invalidate(1);
  assert.equal(a.base.kit._pendingInvalidationCount(), 1);
  bd.open(1, exclusive(1));

  w.frame();

  assert.equal(aReads, 1, "retired parent tokens must fence deferred providers");
  assert.equal(bReads, 1);
  assert.equal(a.base.kit._pendingInvalidationCount(), 0);
});
test("a throwing focused reopen releases its candidate even when a prior local state remains", () => {
  const w = pluginWorld(), p = w.plugin(); let fail = false;
  const m = p.hudkit.modal({ title: () => fail ? { toString() { throw Error("boom"); } } : "T", rows: [] });
  m.open(1, exclusive(0)); fail = true;
  assert.equal(m.tryOpenResult(1, exclusive(0)).ok, false);
  assert.equal(w.focus.size, 0);
});
test("a nested successful focus refresh remains authoritative if the outer provider throws", () => {
  const w = pluginWorld(), p = w.plugin(); let nested = false, picks = 0, m;
  m = focusedModal(p, () => picks++, () => {
    if (nested) { nested = false; m.refresh(1); throw Error("obsolete outer"); }
    return [{ a: "Current" }];
  });
  m.open(1, exclusive(0)); nested = true; m.tryRefresh(1);
  assert.equal(w.focus.size, 1);
  w.dispatchClick(1, "s2_m0_r0"); assert.equal(picks, 1);
});
test("dashboard spec replacement retires and reserves before evaluating the replacement provider", () => {
  const w = pluginWorld(), p = w.plugin();
  const spec = { title: "D", tabs: [{ id: "t", title: "T" }], rows: () => [] };
  const d = p.hudkit.dashboard(spec), old = d.open(1, exclusive(4));
  const token = [...w.focus.keys()][0];
  p.hudkit.dashboard({ ...spec, rows: () => {
    assert.equal(w.focus.has(token), false); assert.equal(w.focus.size, 1); return [];
  } });
  assert.equal(old.isValid(), false);
  assert.notEqual([...w.focus.keys()][0], token);
});

test("open option getters cannot overwrite a nested focused presentation", () => {
  for (const kind of ["modal", "dashboard", "motd"]) {
    const w = pluginWorld(), p = w.plugin();
    const m = kind === "modal" ? focusedModal(p, () => {}) : kind === "dashboard" ?
      p.hudkit.dashboard({ title: "D", tabs: [{ id: "t", title: "T" }], rows: () => [] }) : null;
    const open = options => m ? m.tryOpenResult(1, options) : p.hudkit.motd(1, { title: "M", ...options });
    let calls = 0;
    const opts = { focus: { mode: "exclusive", get priority() {
      calls++; open(exclusive(9)); return 0;
    } } };
    open(opts);
    assert.equal(calls, 1);
    assert.equal(w.focus.size, 1);
    assert.equal([...w.focus.values()][0].priority, 9);
  }
});
test("focus descriptor always records exact panel capture and missing host support fails before providers", () => {
  const w = pluginWorld(), p = w.plugin(); let reads = 0;
  const m = focusedModal(p, () => {}, () => { reads++; return []; });
  delete p.ctx.__s2_surface_reserve;
  assert.equal(m.tryOpenResult(1, exclusive(0)).error.code, "Unavailable"); assert.equal(reads, 0);
  const w2 = pluginWorld(), p2 = w2.plugin(), m2 = focusedModal(p2, () => {});
  for (const priority of [-2147483648, 2147483647]) {
    m2.open(1, { ...exclusive(priority), cursor: false });
    const call = w2.focusCalls.at(-1);
    assert.equal(call.priority, priority); assert.equal(call.index, 1); assert.equal(call.id, 1); assert.equal(call.slot, 1);
    assert.deepEqual(call.adapter, { capture: { call: "setInputCaptureEnabledForPlayer", token: "panel:s2_m0" },
      suspend: { call: "setHasClassForPlayer", args: ["s2_m0", "s2-hide", 1] } });
  }
});

test("failed focused dashboard open is closed even through the legacy adapter", () => {
  const w = pluginWorld({ focusError: "Busy" }), p = w.plugin();
  const d = p.hudkit.dashboard({ title: "D", tabs: [{ id: "t", title: "T" }], rows: () => [] });
  d.open(1, exclusive(0)); assert.equal(d.isOpen(1), false); assert.equal(w.focus.size, 0);
});
test("focused MOTD failures use the invalid handle adapter and do not strand a reservation", () => {
  for (const property of ["cursor", "onClose", "title"]) {
    const w = pluginWorld(), p = w.plugin();
    const spec = { title: "M", ...exclusive(0) };
    Object.defineProperty(spec, property, { get() { throw Error("bad " + property); } });
    const handle = p.hudkit.motd(1, spec);
    assert.equal(handle.isValid(), false); assert.equal(w.focus.size, 0);
  }
});
test("focused capture and restoration failures leave no actions or automatic retry", () => {
  const options = {}, w = pluginWorld(options), a = w.plugin(), b = w.plugin(); let reads = 0, picks = 0;
  const am = focusedModal(a, () => picks++, () => { reads++; return [{ a: "A" }]; });
  options.captureError = "capture failed";
  assert.equal(am.tryOpenResult(1, exclusive(0)).error.code, "PaintFailed"); assert.equal(w.focus.size, 0);
  options.captureError = null; am.open(1, exclusive(0));
  const bm = focusedModal(b, () => {}); bm.open(1, exclusive(1)); bm.close(1);
  options.failInvoke = "setDialogVariableStringForPlayer"; w.frame(); assert.equal(w.focus.size, 0);
  const before = reads; w.frame(); w.dispatchClick(1, "s2_m0_r0");
  assert.equal(reads, before); assert.equal(picks, 0);
});

test("forgetting a formerly active covered dashboard cannot raw-hide the same-root winner", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  const spec = { title: "D", tabs: [{ id: "t", title: "T" }], rows: () => [{ id: "r", a: "R" }] };
  a.hudkit.dashboard(spec).open(1, exclusive(0)); b.hudkit.dashboard(spec).open(1, exclusive(1));
  const before = w.writes.length;
  a.hudkit.forget(1);
  assert.equal(w.writes.length, before, "host focus retirement already hid A; raw forget must not hide B");
  assert.equal([...w.focus.values()][0].state, "active");
});

test("covered modal navigation accumulates intent without evaluating providers", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin(); let reads = 0;
  const am = focusedModal(a, () => {}, () => { reads++; return Array.from({ length: 24 }, (_, i) => ({ a: String(i) })); });
  const bm = focusedModal(b, () => {}); bm.open(1, exclusive(1)); am.open(1, exclusive(0));
  am.page(1, 1); am.page(1, 1); assert.equal(reads, 0);
  bm.close(1); w.frame(); assert.equal(am.cursor(1), 16);
});

test("incomplete native focus support is unavailable before any reservation or provider", () => {
  for (const name of ["reserve", "state", "activate", "active", "release"]) {
    const w = pluginWorld(), p = w.plugin(); let reads = 0;
    const m = focusedModal(p, () => {}, () => { reads++; return []; });
    delete p.ctx["__s2_surface_" + name];
    assert.equal(m.tryOpenResult(1, exclusive(0)).error.code, "Unavailable");
    assert.equal(reads, 0); assert.equal(w.focus.size, 0);
  }
});

test("100 explicit invalidations coalesce to one next-frame provider evaluation", () => {
  const w = pluginWorld(), p = w.plugin();
  let modalReads = 0, dashReads = 0;
  const modal = p.hudkit.modal({ rows: () => { modalReads++; return [{ id: "m", a: String(modalReads) }]; } });
  const dash = p.hudkit.dashboard({ title: "D", tabs: [{ id: "t", title: "T" }],
    rows: () => { dashReads++; return [{ id: "d", a: String(dashReads) }]; } });
  const modalView = modal.open(1), dashView = dash.open(2);
  assert.deepEqual(plain(modalView.lastUpdateResult()), { ok: true });
  assert.deepEqual(plain(dashView.lastUpdateResult()), { ok: true });
  for (let i = 0; i < 100; i++) { modal.invalidate(1); dashView.invalidate(); }
  assert.deepEqual(plain(modalView.lastUpdateResult()), { ok: true },
    "pending invalidation retains the previous completion");
  assert.deepEqual([modalReads, dashReads, p.base.kit._pendingInvalidationCount()], [1, 1, 2]);
  w.frame();
  assert.deepEqual([modalReads, dashReads, p.base.kit._pendingInvalidationCount()], [2, 2, 0]);
});

test("invalidation during a provider runs on the following frame", () => {
  const w = pluginWorld(), p = w.plugin();
  let reads = 0, reenter = false, view;
  const modal = p.hudkit.modal({ rows: () => {
    reads++;
    if (reenter) { reenter = false; view.invalidate(); }
    return [{ id: "m", a: String(reads) }];
  } });
  view = modal.open(1);
  reenter = true;
  view.invalidate();
  w.frame();
  assert.equal(reads, 2);
  assert.equal(p.base.kit._pendingInvalidationCount(), 1);
  w.frame();
  assert.equal(reads, 3);
  assert.equal(p.base.kit._pendingInvalidationCount(), 0);
});

test("a nested synchronous repaint during a drain does not abort later dirty views", () => {
  const w = pluginWorld(), p = w.plugin();
  let nested = false, firstReads = 0, secondReads = 0, first;
  first = p.hudkit.modal({ rows: () => {
    firstReads++;
    if (nested) { nested = false; first.refresh(1); }
    return [{ id: "first", a: "First" }];
  } });
  const second = p.hudkit.modal({ rows: () => { secondReads++; return [{ id: "second", a: "Second" }]; } });
  const firstView = first.open(1); second.open(2);
  nested = true; first.invalidate(1); second.invalidate(2);
  assert.doesNotThrow(() => w.frame());
  assert.deepEqual([firstReads, secondReads], [3, 2]);
  assert.deepEqual(plain(firstView.lastUpdateResult()), { ok: true });
  assert.equal(p.base.kit._pendingInvalidationCount(), 0);
});

test("a throwing deferred provider records failure without starving another dirty view", () => {
  const w = pluginWorld(), p = w.plugin();
  let fail = false, goodReads = 0, picks = 0;
  const bad = p.hudkit.modal({ rows: () => {
    if (fail) throw new Error("deferred provider failed");
    return [{ id: "bad", a: "Bad" }];
  }, onPick: () => picks++ });
  const good = p.hudkit.modal({ rows: () => { goodReads++; return [{ id: "good", a: String(goodReads) }]; } });
  const badView = bad.open(1); good.open(2);
  fail = true; bad.invalidate(1); good.invalidate(2);
  w.frame();
  assert.deepEqual(plain(badView.lastUpdateResult()), {
    ok: false, error: { code: "InvalidArgument", message: "deferred provider failed" },
  });
  assert.equal(goodReads, 2);
  assert.equal(p.base.kit._pendingInvalidationCount(), 0);
  p.click(1, "s2_m0_r0");
  assert.equal(picks, 0, "failed deferred repaint must disable interaction");
  assert.equal(p.logs.filter(line => line.includes("deferred provider failed")).length, 1);
  w.frame();
  assert.equal(p.logs.filter(line => line.includes("deferred provider failed")).length, 1,
    "failure does not automatically retry");

  bad.invalidate(1); w.frame();
  assert.equal(p.logs.filter(line => line.includes("deferred provider failed")).length, 1,
    "repeated failure is not diagnosed again before success");
  fail = false; bad.invalidate(1); w.frame();
  assert.deepEqual(plain(badView.lastUpdateResult()), { ok: true });
  fail = true; bad.invalidate(1); w.frame();
  assert.equal(p.logs.filter(line => line.includes("deferred provider failed")).length, 2,
    "success rearms the transition diagnostic");
});

test("close, forget, release, reconnect and entity replacement discard dirty work", () => {
  for (const cleanup of ["close", "forget", "release", "reconnect", "entity"]) {
    const w = pluginWorld(), p = w.plugin(); let reads = 0;
    const modal = p.hudkit.modal({ rows: () => { reads++; return [{ a: "row" }]; } });
    modal.open(1); modal.invalidate(1);
    if (cleanup === "close") modal.close(1);
    else if (cleanup === "forget") modal.forget(1);
    else if (cleanup === "release") modal.release();
    else if (cleanup === "reconnect") w.replace(1);
    else w.replaceLayoutEntity();
    const before = w.writes.length;
    w.frame();
    assert.equal(w.writes.length, before, cleanup + " must cause zero deferred writes");
    assert.equal(reads, 1, cleanup + " must cause zero deferred provider reads");
    assert.equal(p.base.kit._pendingInvalidationCount(), 0, cleanup + " must empty the queue");
  }
});

test("focus restoration fulfills an already-pending invalidation with one repaint", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin(); let reads = 0;
  const am = focusedModal(a, () => {}, () => { reads++; return [{ id: "a", a: String(reads) }]; });
  const bm = focusedModal(b, () => {});
  const view = am.open(1, exclusive(0));
  bm.open(1, exclusive(1));
  view.invalidate();
  w.frame();
  assert.equal(reads, 1, "covered focus cannot repaint");
  assert.equal(a.base.kit._pendingInvalidationCount(), 1);
  bm.close(1); w.frame();
  assert.equal(reads, 2, "ready restoration and dirty intent share one repaint");
  assert.equal(a.base.kit._pendingInvalidationCount(), 0);
});

test("invalidation raised during focus restoration waits for the following frame", () => {
  const w = pluginWorld(), a = w.plugin(), b = w.plugin();
  let reads = 0, invalidateDuringRestore = false, view;
  const am = focusedModal(a, () => {}, () => {
    reads++;
    if (invalidateDuringRestore) { invalidateDuringRestore = false; view.invalidate(); }
    return [{ id: "a", a: String(reads) }];
  });
  const bm = focusedModal(b, () => {});
  view = am.open(1, exclusive(0));
  bm.open(1, exclusive(1));
  view.invalidate();
  bm.close(1);
  invalidateDuringRestore = true;
  w.frame();
  assert.equal(reads, 2, "restoration must not drain its reentrant invalidation in the same frame");
  assert.equal(a.base.kit._pendingInvalidationCount(), 1);
  w.frame();
  assert.equal(reads, 3);
  assert.equal(a.base.kit._pendingInvalidationCount(), 0);
});

test("a failed focus restoration consumes pending intent and does not retry", () => {
  const options = {}, w = pluginWorld(options), a = w.plugin(), b = w.plugin(); let reads = 0;
  const am = focusedModal(a, () => {}, () => { reads++; return [{ id: "a", a: "A" }]; });
  const bm = focusedModal(b, () => {});
  const view = am.open(1, exclusive(0)); bm.open(1, exclusive(1)); view.invalidate();
  bm.close(1); options.failInvoke = "setDialogVariableStringForPlayer"; w.frame();
  assert.equal(view.lastUpdateResult().ok, false);
  assert.equal(a.base.kit._pendingInvalidationCount(), 0);
  const afterFailure = reads;
  options.failInvoke = null; w.frame();
  assert.equal(reads, afterFailure, "restoration failure must not automatically retry");
});

test("invalidate is an explicit retry after a deferred focused failure", () => {
  const options = {}, w = pluginWorld(options), p = w.plugin(); let reads = 0, label = "A";
  const modal = focusedModal(p, () => {}, () => { reads++; return [{ id: "a", a: label }]; });
  const view = modal.open(1, exclusive(0));
  label = "B"; options.failInvoke = "setDialogVariableStringForPlayer"; view.invalidate(); w.frame();
  assert.equal(view.lastUpdateResult().ok, false);
  const afterFailure = reads;
  options.failInvoke = null; view.invalidate(); w.frame();
  assert.equal(reads, afterFailure + 1);
  assert.deepEqual(plain(view.lastUpdateResult()), { ok: true });
  assert.equal(p.base.kit._pendingInvalidationCount(), 0);
  assert.equal([...w.focus.values()][0].state, "active");
});

test("owned dashboard invalidate retries after a failed focused repaint retires both tokens", () => {
  const options = {}, w = pluginWorld(options), p = w.plugin();
  let reads = 0;
  const owned = p.hudkit.tryOwnDashboard({ title: "Owned", tabs: [{ id: "t", title: "T" }],
    rows: () => { reads++; return [{ id: "r", a: String(reads) }]; } }).value;
  const view = owned.tryOpenResult(1, exclusive(0)).value;

  options.failInvoke = "setDialogVariableStringForPlayer";
  view.invalidate();
  w.frame();
  assert.equal(view.lastUpdateResult().ok, false);
  assert.equal(w.owned.size, 0, "failed repaint retires the parent ownership token");
  assert.equal(w.focus.size, 0, "parent retirement cascades the linked focus token");
  const afterFailure = reads;

  options.failInvoke = null;
  view.invalidate();
  assert.equal(p.base.kit._pendingInvalidationCount(), 1);
  w.frame();
  assert.equal(reads, afterFailure + 1);
  assert.deepEqual(plain(view.lastUpdateResult()), { ok: true });
  assert.equal(w.owned.size, 1);
  assert.equal(w.focus.size, 1);
});

for (const recovery of [
  { mode: "legacy", deferred: false },
  { mode: "explicit", deferred: true },
]) test(`nonfocused ${recovery.mode} dashboard ${recovery.deferred ? "invalidate" : "refresh"} retry reveals and recaptures its root`, () => {
  const options = {}, w = pluginWorld(options), p = w.plugin();
  let title = "A", reads = 0;
  const spec = { title: () => title, tabs: [{ id: "t", title: "T" }],
    rows: () => { reads++; return [{ id: "r", a: String(reads) }]; } };
  const dashboard = recovery.mode === "legacy" ? p.hudkit.dashboard(spec) :
    p.hudkit.tryOwnDashboard(spec).value;
  const view = dashboard.tryOpenResult(1).value;
  title = "B";
  options.failInvoke = "setDialogVariableStringForPlayer";
  assert.equal(view.tryRefresh().error.code, "PaintFailed");
  options.failInvoke = null;
  const beforeWrites = w.writes.length;

  if (recovery.deferred) { view.invalidate(); w.frame(); }
  else assert.equal(view.tryRefresh().ok, true);

  const retryWrites = w.writes.slice(beforeWrites);
  assert.ok(retryWrites.some(call => call.name === "setHasClassForPlayer" &&
    call.args[2] === "s2_dash" && call.args[3] === "s2-hide" && call.args[4] === 0),
  "the replacement parent must reveal the retired dashboard root");
  assert.ok(retryWrites.some(call => call.name === "setInputCaptureEnabledForPlayer" &&
    call.args[2] === true), "the requested cursor must be reacquired");
  assert.deepEqual(plain(view.lastUpdateResult()), { ok: true });
  assert.equal(view.isOpen(), true);
});

for (const focused of [false, true]) test(`hideAll retires a tokenless failed legacy ${focused ? "focused" : "nonfocused"} dashboard`, () => {
  const options = {}, w = pluginWorld(options), p = w.plugin();
  let reads = 0;
  const dashboard = p.hudkit.dashboard({ title: "Legacy", tabs: [{ id: "t", title: "T" }],
    rows: () => { reads++; return [{ id: "r", a: String(reads) }]; } });
  const view = dashboard.tryOpenResult(1, focused ? exclusive(0) : undefined).value;
  options.failInvoke = "setDialogVariableStringForPlayer";
  assert.equal(view.tryRefresh().error.code, "PaintFailed");
  options.failInvoke = null;
  view.invalidate();
  assert.equal(p.base.kit._pendingInvalidationCount(), 1);

  p.hudkit.hideAll(1);

  assert.equal(view.isOpen(), false);
  assert.equal(p.base.kit._pendingInvalidationCount(), 0);
  const beforeReads = reads, beforeWrites = w.writes.length;
  w.frame();
  view.invalidate();
  w.frame();
  assert.equal(reads, beforeReads, "a cleared legacy view cannot evaluate its provider again");
  assert.equal(w.writes.length, beforeWrites, "a cleared legacy view cannot repaint or reopen");
});

test("hideAll preserves a tokenless explicit dashboard retry and its dirty work", () => {
  const options = {}, w = pluginWorld(options), p = w.plugin();
  let reads = 0;
  const dashboard = p.hudkit.tryOwnDashboard({ title: "Explicit", tabs: [{ id: "t", title: "T" }],
    rows: () => { reads++; return [{ id: "r", a: String(reads) }]; } }).value;
  const view = dashboard.tryOpenResult(1).value;
  options.failInvoke = "setDialogVariableStringForPlayer";
  assert.equal(view.tryRefresh().error.code, "PaintFailed");
  options.failInvoke = null;
  view.invalidate();

  p.hudkit.hideAll(1);

  assert.equal(view.isOpen(), true);
  assert.equal(p.base.kit._pendingInvalidationCount(), 1);
  w.frame();
  assert.equal(reads, 3);
  assert.deepEqual(plain(view.lastUpdateResult()), { ok: true });
  assert.equal(view.isOpen(), true);
});

test("last completed update survives close but stale lifetimes cannot read replacement results", () => {
  const w = pluginWorld(), p = w.plugin();
  const modal = p.hudkit.modal({ rows: [] });
  const beforeOpen = modal.forSlot(1);
  assert.equal(beforeOpen.lastUpdateResult(), null);
  modal.open(1);
  assert.deepEqual(plain(beforeOpen.lastUpdateResult()), { ok: true });
  modal.close(1);
  assert.deepEqual(plain(beforeOpen.lastUpdateResult()), { ok: true });
  w.replace(1);
  assert.equal(beforeOpen.lastUpdateResult(), null);
  const replacement = modal.open(1);
  assert.deepEqual(plain(replacement.lastUpdateResult()), { ok: true });
  assert.equal(beforeOpen.lastUpdateResult(), null);
});

test("failed initial opens and synchronous refreshes are retained as completed results", () => {
  const options = { failInvoke: "setDialogVariableStringForPlayer" };
  const w = pluginWorld(options), p = w.plugin();
  const modal = p.hudkit.modal({ title: "M", rows: [] });
  const beforeOpen = modal.forSlot(1);
  const failedOpen = modal.tryOpenResult(1);
  assert.equal(failedOpen.error.code, "PaintFailed");
  assert.deepEqual(plain(beforeOpen.lastUpdateResult()), plain(failedOpen));

  options.failInvoke = null;
  const view = modal.open(1);
  options.failInvoke = "setDialogVariableStringForPlayer";
  const badTitle = { toString() { throw new Error("sync title failed"); } };
  const dashboard = p.hudkit.dashboard({ title: () => badTitle,
    tabs: [{ id: "t", title: "T" }], rows: () => [] });
  const dashBeforeOpen = dashboard.forSlot(2);
  const dashFailure = dashboard.tryOpenResult(2);
  assert.equal(dashFailure.error.code, "PaintFailed");
  assert.deepEqual(plain(dashBeforeOpen.lastUpdateResult()), plain(dashFailure));

  options.failInvoke = null;
  modal.refresh(1);
  assert.deepEqual(plain(view.lastUpdateResult()), { ok: true });
});
