/**
 * `ui` — offline tests over the real shipped CS2 addon bundle (see cs2-addon.mjs).
 */
import { test } from "node:test";
import assert from "node:assert";
import vm from "node:vm";
import { cs2AddonBundle } from "./cs2-addon.mjs";

const HUD_CALLS = [
  "setHasClassForPlayer",
  "setDialogVariableStringForPlayer",
  "setInputCaptureEnabledForPlayer",
];

const BASE_CALLS = [
  "commitSuicide", "changeTeam", "switchTeam", "terminateRound",
  "respawn", "setPawn", "giveNamedItem", "removePlayerItem",
  ...HUD_CALLS,
];

function makeHost({ ready = BASE_CALLS, onInvoke, onHook, entities = [], signon = 6 } = {}) {
  const readySet = new Set(ready);
  const invokes = [];
  const hooks = [];
  const hookSubs = [];
  const logs = [];
  const created = [];
  let mapReady = false;
  let activeHandlers = [];
  let disconnectHandlers = [];
  let mapStartHandlers = [];

  function EntityRef(index, id) {
    this.index = index;
    this.id = id;
    this.live = true;
  }
  EntityRef.prototype.isValid = function () { return this.live !== false; };
  EntityRef.prototype.remove = function () { this.live = false; return true; };

  const entityList = entities.slice();

  const ctx = {
    __s2require: (n) => {
      if (n === "@s2script/sdk/entity") return { EntityRef };
      if (n === "@s2script/sdk/math") {
        return {
          Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
          QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
        };
      }
      if (n === "@s2script/sdk/events") return {};
      return null;
    },
    __s2_game_call_ready: (name) => readySet.has(name),
    __s2_game_call_receiverless: () => false,
    __s2_game_call_status: (name) => readySet.has(name) ? "available" : "degraded: " + name,
    __s2_game_call_invoke: (name, index, id, args) => {
      invokes.push({ name, index, id, args });
      return onInvoke ? onInvoke(name, args, { index, id }) : undefined;
    },
    __s2_hook_on: (owner, name, handler) => {
      hooks.push({ owner, name, handler });
      hookSubs.push(handler);
      return hooks.length;
    },
    __s2_hook_self_matches: () => false,
    __s2_schema_offset: () => -1,
    __s2_client_valid: () => true,
    __s2_client_signon: () => signon,
    __s2_cvar_get: (n) => (n === "mm_extra_addons" ? "3790153369" : ""),
    __s2_map_start_subscribe: (h) => { mapStartHandlers.push(h); return 1; },
    __s2_client_subscribe: (ev, h) => {
      if (ev === "active") activeHandlers.push(h);
      if (ev === "disconnect") disconnectHandlers.push(h);
      return 1;
    },
    __s2pkg_entity: {
      EntityRef,
      Entity: {
        findByClass: (cls) => (cls === "custom_hud_layout" ? entityList.filter((e) => e.isValid()) : []),
      },
      createEntity: (cls, kv) => {
        const ref = new EntityRef(500 + created.length, 9 + created.length);
        ref.name = kv.targetname || null;
        ref.kv = kv;
        created.push(ref);
        entityList.push(ref);
        return ref;
      },
    },
    __s2pkg_server: {
      Server: {
        getCvar: (n) => ctx.__s2_cvar_get(n),
        onMapStart: (h) => { mapStartHandlers.push(h); },
      },
    },
    __s2pkg_clients: {
      Clients: {
        all: () => [{ slot: 0, signonState: signon }],
        onActive: (h) => { activeHandlers.push(h); },
        onDisconnect: (h) => { disconnectHandlers.push(h); },
      },
    },
    console: { log: (m) => logs.push(String(m)) },
  };

  ctx.__s2pkg_cs2 = {
    Player: {
      all: () => [],
    },
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(cs2AddonBundle, ctx);

  function armPlugin() {
    const pending = [];
    let armed = false;
    const reg = (fn) => { if (armed) fn(); else pending.push(fn); };
    const viaId = (call) => call;
    const ui = ctx.__s2pkg_game_ctx.ui(reg, viaId);
    ctx.__s2_ctx_arm = () => { armed = true; pending.forEach((f) => f()); };
    return ui;
  }

  function fireActive(slot) {
    activeHandlers.forEach((h) => h({ slot: slot }));
  }

  function fireMapStart() {
    mapStartHandlers.forEach((h) => h());
  }

  function fireClick(buttonId, playerRef) {
    for (const sub of hookSubs) sub({ player: playerRef, buttonId });
  }

  return {
    ctx, invokes, hooks, logs, created, armPlugin, fireActive, fireMapStart, fireClick, EntityRef,
  };
}

test("bundle includes ui.js", () => {
  assert.match(cs2AddonBundle, /required workshop addons/);
  assert.match(cs2AddonBundle, /ctx\.ui/);
});

test("setHasClassForPlayer arg order and enum status int", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  const err = hud.show(0, "s2_dialog");
  assert.equal(err, null);
  assert.equal(h.invokes.length, 1);
  assert.equal(h.invokes[0].name, "setHasClassForPlayer");
  assert.deepEqual(h.invokes[0].args.slice(0, 4), [0, "s2_dialog", "s2-hidden", 0]);
});

test("setDialogVariableStringForPlayer arg order", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.setText(0, "s2_dialog_title", "hello");
  assert.equal(h.invokes[0].name, "setDialogVariableStringForPlayer");
  assert.deepEqual(h.invokes[0].args, [0, "s2_dialog_title", "title", "hello"]);
});

test("setInputCaptureEnabledForPlayer on show with cursor", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.show(0, "s2_dialog", { cursor: true });
  assert.deepEqual(h.invokes.map((i) => i.name), [
    "setHasClassForPlayer",
    "setInputCaptureEnabledForPlayer",
  ]);
  assert.deepEqual(h.invokes[1].args, [0, true]);
});

test("rejects .vxml resource extension", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  assert.throws(
    () => ui.hud({
      addons: ["3790153369"],
      resource: "panorama/layout/custom_game/x.vxml",
      hideClass: "h",
      text: {},
      buttons: [],
      meters: {},
    }),
    /\.xml source extension|must be under panorama/,
  );
});

test("hud() throw prefix is ui.hud, not ctx.ui", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  assert.throws(
    () => ui.hud({
      addons: [],
      resource: "panorama/layout/custom_game/x.xml",
      hideClass: "h",
      text: {},
      buttons: [],
      meters: {},
    }),
    /^Error: ui\.hud: descriptor\.addons must be a non-empty string array$/,
  );
});

test("degraded setHasClassForPlayer returns named reason", () => {
  const h = makeHost({ ready: BASE_CALLS.filter((n) => n !== "setHasClassForPlayer") });
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  const err = hud.show(0, "s2_dialog");
  assert.match(err, /unavailable/);
  assert.equal(h.invokes.length, 0);
});

test("no entity spawn before active-client readiness", () => {
  const h = makeHost({ signon: 2 });
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  ui.createLayout();
  const hud = ui.hud();
  const err = hud.show(0, "s2_dialog");
  assert.match(err, /not ready/);
  assert.equal(h.created.length, 0);
});

test("createLayout is idempotent by targetname", () => {
  const existing = [];
  const h = makeHost({ entities: existing });
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.show(0, "s2_dialog");
  assert.equal(h.created.length, 1);
  const first = h.created[0];
  existing.push(first);
  h.invokes.length = 0;
  ui.createLayout();
  hud.show(0, "s2_dialog");
  assert.equal(h.created.length, 1, "a second createLayout must find the existing entity");
  assert.equal(h.invokes[0].index, first.index);
});

test("hud() after an active client spawns the layout", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  const hud = ui.hud();
  const err = hud.show(0, "s2_dialog");
  assert.equal(err, null);
  assert.equal(h.created.length, 1, "hud() must spawn once a client is active");
  assert.equal(h.invokes.length, 1);
});

test("hud() before join, then onActive, spawns without createLayout", () => {
  const h = makeHost({ signon: 2 });
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  const hud = ui.hud();
  assert.equal(h.created.length, 0, "no spawn before an active client");
  h.fireActive(0);
  const err = hud.show(0, "s2_dialog");
  assert.equal(err, null);
  assert.equal(h.created.length, 1, "player-join must spawn layouts registered at load");
});

test("createLayout from onActive spawns", () => {
  const h = makeHost({ signon: 2 });
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.ctx.__s2pkg_clients.Clients.onActive(() => {
    const err = ui.createLayout();
    assert.equal(err, null);
  });
  assert.equal(h.created.length, 0);
  h.fireActive(0);
  assert.equal(h.created.length, 1);
});


test("setPool overflow refuses without invoke", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  const err = hud.setPool(0, "rows", new Array(9).fill(["x"]));
  assert.match(err, /paginate/);
  assert.equal(h.invokes.length, 0);
});

test("setPool uses per-slot vars", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.setPool(0, "rows", [["a"], ["b"]]);
  const varCalls = h.invokes.filter((i) => i.name === "setDialogVariableStringForPlayer");
  assert.deepEqual(varCalls[0].args, [0, "s2_row_0", "row0", "a"]);
  assert.deepEqual(varCalls[1].args, [0, "s2_row_1", "row1", "b"]);
});

test("setMeter(50) applies s2-w5 and clears previous step", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.setMeter(0, "meter", 20);
  hud.setMeter(0, "meter", 50);
  const classCalls = h.invokes.filter((i) => i.name === "setHasClassForPlayer");
  const applied = classCalls.map((c) => c.args[2]);
  assert.ok(applied.includes("s2-w2"));
  assert.ok(applied.includes("s2-w5"));
  assert.ok(applied.some((c) => c === "s2-w2" && classCalls.find((x) => x.args[2] === "s2-w2").args[3] === 1));
  assert.ok(applied.some((c) => c === "s2-w2" && classCalls.some((x) => x.args[2] === "s2-w2" && x.args[3] === 0)));
});

test("cursor lease released across two panels on same layout", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.show(0, "s2_dialog", { cursor: true });
  hud.show(0, "s2_banner", { cursor: true });
  hud.hide(0, "s2_dialog");
  assert.equal(h.invokes.filter((i) => i.name === "setInputCaptureEnabledForPlayer" && i.args[1] === false).length, 0);
  hud.hide(0, "s2_banner");
  assert.deepEqual(
    h.invokes.filter((i) => i.name === "setInputCaptureEnabledForPlayer" && i.args[1] === false).map((i) => i.args),
    [[0, false]],
  );
});

test("disabled button blocks dispatchClick", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  let fired = false;
  hud.onClick("s2_btn_0", () => { fired = true; });
  hud.setDisabled(0, "s2_btn_0", true);
  assert.equal(hud.dispatchClick(0, "s2_btn_0"), false);
  assert.equal(fired, false);
});

test("click resolves exact index+id match", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  let slot = -1;
  hud.onClick("s2_btn_0", (s) => { slot = s; });
  h.ctx.__s2pkg_cs2.Player = {
    all: () => [{ slot: 2, ref: { index: 7, id: 3 } }],
  };
  h.fireClick("s2_btn_0", new h.EntityRef(7, 3));
  assert.equal(slot, 2);
});

test("button id collision throws", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  ui.createLayout();
  const hud = ui.hud();
  hud.onClick("s2_btn_0", () => {});
  assert.throws(() => hud.onClick("s2_btn_0", () => {}), /conflicting handler/);
});

test("one lazy click subscription per plugin context", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  ui.createLayout();
  const hud = ui.hud();
  hud.onClick("s2_btn_0", () => {});
  ui.onCustomHudClicked(() => {});
  assert.equal(h.hooks.filter((x) => x.name === "onCustomHudClicked").length, 1);
});

test("MAM banner emitted once on first hud()", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  ui.hud();
  ui.hud();
  const banners = h.logs.filter((l) => l.includes("required workshop addons"));
  assert.equal(banners.length, 1);
});

test("map reset clears entity cache and readiness", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  h.fireActive(0);
  ui.createLayout();
  const hud = ui.hud();
  hud.show(0, "s2_dialog");
  assert.equal(h.created.length, 1);
  for (const e of h.created) e.live = false;
  h.fireMapStart();
  const err = hud.show(0, "s2_dialog");
  assert.match(err, /not ready/);
  h.fireActive(0);
  assert.equal(hud.show(0, "s2_dialog"), null);
  assert.equal(h.created.length, 2, "the next player-join must respawn the layout");
});

test("onCustomHudClicked wrapper returns Continue", () => {
  const h = makeHost();
  const ui = h.armPlugin();
  h.ctx.__s2_ctx_arm();
  ui.onCustomHudClicked(() => {});
  assert.equal(h.hooks.length, 1);
  const ret = h.hooks[0].handler({ player: null, buttonId: "x" });
  assert.equal(ret, 0);
});
