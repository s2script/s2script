// Shared fake for the GameHud panel tests: a ui base whose hud() serves both game-kit layouts.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

function evalFile(name) {
  new Function(readFileSync(join(__dirname, name), "utf8"))();
}

// A fake ui base whose hud() serves both gamekit layouts, records per-slot panel state, cursor
// grabs and click handlers. `refuse` makes every drive for that slot fail.
function mount(opts = {}) {
  const panels = {};
  const meters = {};
  const chat = [];
  const timers = [];
  const claims = {};
  const cursors = {};
  const clicks = {};
  const armed = {};
  const disabled = {};
  const chatSubs = [];
  const commands = [];
  const frameSubs = new Set();
  let drives = 0;
  const refuse = new Set(opts.refuse || []);
  const slots = opts.slots || [0, 1, 2];
  let disconnect = null;
  let mapStart = null;

  function panel(slot, id) {
    const k = slot + "|" + id;
    return panels[k] || (panels[k] = { classes: new Set(), text: undefined });
  }
  function drive(slot, fn) {
    if (refuse.has(slot)) return "custom_hud_layout entity unavailable";
    drives++;
    fn();
    return null;
  }
  function makeHud() {
    return {
      setText: (slot, id, v) => drive(slot, () => { panel(slot, id).text = String(v); }),
      set: (slot, id, v) => drive(slot, () => { panel(slot, id).text = String(v); }),
      _setRaw: (slot, id, v) => drive(slot, () => { panel(slot, id).text = String(v); }),
      show: (slot, id, o) => drive(slot, () => {
        panel(slot, id).classes.delete("s2-hide");
        if (o && o.cursor) cursors[slot + "|" + id] = true;
      }),
      hide: (slot, id) => drive(slot, () => { panel(slot, id).classes.add("s2-hide"); delete cursors[slot + "|" + id]; }),
      setClass: (slot, id, cls, on) => drive(slot, () => { if (on) panel(slot, id).classes.add(cls); else panel(slot, id).classes.delete(cls); }),
      setMeter: (slot, name, pct) => drive(slot, () => { meters[slot + "|" + name] = pct; }),
      setDisabled: (slot, id, on) => drive(slot, () => { disabled[slot + "|" + id] = !!on; }),
      _cursorForPanel: (slot, root, on) => { if (on) cursors[slot + "|" + root] = true; else delete cursors[slot + "|" + root]; return null; },
      onClick: (id, fn) => { assert.equal(clicks[id], undefined, "one handler per button"); clicks[id] = fn; },
      invalidatePanelTree: () => {},
    };
  }
  const huds = {};
  const base = {
    hud: (desc) => {
      assert.equal(desc.observable, false);
      return huds[desc.resource] || (huds[desc.resource] = makeHud());
    },
  };

  globalThis.__s2pkg_cs2 = {};
  globalThis.__s2pkg_game_ctx = { ui: () => base };
  globalThis.__s2pkg_clients = {
    Clients: {
      all: () => slots.map((slot) => ({ slot, signonState: 6 })),
      onDisconnect: (fn) => { disconnect = fn; return 1; },
      fromSlot: (slot) => (slots.includes(slot) ? { slot, fakeCommand: (cmd) => { commands.push({ slot, cmd }); return true; } } : null),
    },
  };
  globalThis.__s2pkg_server = { Server: { onMapStart: (fn) => { mapStart = fn; return 2; } } };
  globalThis.__s2pkg_chat = { Chat: { toSlot: (slot, msg) => chat.push({ slot, msg }) } };
  globalThis.__s2pkg_timers = { after: (ms, fn) => timers.push({ ms, fn }) };
  globalThis.__s2pkg_hudinput = {
    HudInput: {
      arm: (slot, o) => { armed[slot] = o; },
      disarm: (slot) => { delete armed[slot]; },
    },
  };
  globalThis.__s2pkg_frame = {
    OnGameFrame: {
      subscribe: (fn) => { const sub = { fn }; frameSubs.add(sub); return { dispose: () => frameSubs.delete(sub) }; },
    },
  };
  globalThis.__s2_chat_on_message = (fn) => { chatSubs.push(fn); return 9; };
  globalThis.__s2_ui_pool_claim = (kind) => { if (claims[kind]) return -1; claims[kind] = true; return 0; };
  globalThis.__s2_ui_pool_release = (kind) => { delete claims[kind]; return true; };

  evalFile("gamekit.generated.js");
  evalFile("gamekit.js");
  for (const f of opts.files || ["gamekit-panels.js"]) evalFile(f);
  globalThis.__s2pkg_game_ctx.ui((thunk) => thunk(), (call) => () => call());
  const GameHud = globalThis.__s2pkg_cs2.GameHud;
  return {
    GameHud, meters, chat, timers, claims, cursors, clicks, armed, disabled, commands,
    hidden: (slot, id) => panel(slot, id).classes.has("s2-hide"),
    has: (slot, id, cls) => panel(slot, id).classes.has(cls),
    text: (slot, id) => panel(slot, id).text,
    say: (slot, text) => chatSubs.reduce((r, fn) => fn(slot, text) || r, undefined),
    click: (id, slot) => clicks[id]({ slot }),
    disconnect: (slot) => disconnect({ slot }),
    mapStart: () => mapStart(),
    tick: () => { for (const t of timers.splice(0)) t.fn(); },
    frame: () => { for (const sub of [...frameSubs]) sub.fn(); },
    get frameSubs() { return frameSubs.size; },
    get drives() { return drives; },
    refuse,
  };
}

function cleanup() {
  for (const k of ["__s2pkg_cs2", "__s2pkg_game_ctx", "__s2pkg_clients", "__s2pkg_server", "__s2pkg_chat",
    "__s2pkg_timers", "__s2pkg_hudinput", "__s2_chat_on_message", "__s2_ui_pool_claim", "__s2_ui_pool_release",
    "__s2pkg_gamekit_layout", "__s2pkg_gamekit_util", "__s2gk_claims", "__s2pkg_frame", "__s2_schema_offset"]) {
    delete globalThis[k];
  }
}

module.exports = { mount, cleanup };
