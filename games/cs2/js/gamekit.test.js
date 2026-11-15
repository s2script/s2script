const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

function evalFile(name) {
  new Function(readFileSync(join(__dirname, name), "utf8"))();
}

// A fake ui base whose hud() records per-slot panel state. `refuse` makes every drive for that
// slot fail, standing in for a missing layout entity.
function mount(opts = {}) {
  const panels = {};      // `${slot}|${id}` -> { classes:Set, text }
  const meters = {};
  const chat = [];
  const timers = [];
  const claims = {};
  const refuse = new Set(opts.refuse || []);
  const stale = new Set(opts.stale || []);
  const slots = opts.slots || [0, 1, 2];
  let disconnect = null;
  let mapStart = null;

  function panel(slot, id) {
    const k = slot + "|" + id;
    return panels[k] || (panels[k] = { classes: new Set(), text: undefined });
  }
  function drive(slot, fn) {
    if (stale.has(slot)) return "stale client";
    if (refuse.has(slot)) return "custom_hud_layout entity unavailable";
    fn();
    return null;
  }
  const hud = {
    setText: (slot, id, v) => drive(slot, () => { panel(slot, id).text = String(v); }),
    set: (slot, id, v) => drive(slot, () => { panel(slot, id).text = String(v); }),
    show: (slot, id) => drive(slot, () => { panel(slot, id).classes.delete("s2-hide"); }),
    hide: (slot, id) => drive(slot, () => { panel(slot, id).classes.add("s2-hide"); }),
    setClass: (slot, id, cls, on) => drive(slot, () => { if (on) panel(slot, id).classes.add(cls); else panel(slot, id).classes.delete(cls); }),
    setMeter: (slot, name, pct) => drive(slot, () => { meters[slot + "|" + name] = pct; }),
    _setRaw: (slot, id, v) => drive(slot, () => { panel(slot, id).text = String(v); }),
    invalidatePanelTree: () => {},
  };
  let hudCalls = 0;
  const base = { hud: (desc) => { hudCalls++; assert.equal(desc.observable, false); return hud; } };

  globalThis.__s2pkg_cs2 = {};
  globalThis.__s2pkg_game_ctx = { ui: () => base };
  globalThis.__s2pkg_clients = {
    Clients: {
      all: () => slots.map((slot) => ({ slot, signonState: 6 })),
      onDisconnect: (fn) => { disconnect = fn; return 1; },
    },
  };
  globalThis.__s2pkg_server = { Server: { onMapStart: (fn) => { mapStart = fn; return 2; } } };
  globalThis.__s2pkg_chat = { Chat: { toSlot: (slot, msg) => chat.push({ slot, msg }) } };
  globalThis.__s2pkg_timers = { after: (ms, fn) => timers.push({ ms, fn }) };
  globalThis.__s2_ui_pool_claim = (kind) => { if (claims[kind]) return -1; claims[kind] = true; return 0; };
  globalThis.__s2_ui_pool_release = (kind) => { delete claims[kind]; return true; };

  evalFile("gamekit.generated.js");
  evalFile("gamekit.js");
  // Stand in for __s2_make_ctx building the plugin's ui namespace.
  globalThis.__s2pkg_game_ctx.ui((thunk) => thunk(), (call) => () => call());
  const GameHud = globalThis.__s2pkg_cs2.GameHud;
  return {
    GameHud, panels, meters, chat, timers, claims,
    get hudCalls() { return hudCalls; },
    hidden: (slot, id) => panel(slot, id).classes.has("s2-hide"),
    has: (slot, id, cls) => panel(slot, id).classes.has(cls),
    text: (slot, id) => panel(slot, id).text,
    disconnect: (slot) => disconnect({ slot }),
    mapStart: () => mapStart(),
    runTimers: () => { for (const t of timers.splice(0)) t.fn(); },
  };
}

test.afterEach(() => {
  for (const k of ["__s2pkg_cs2", "__s2pkg_game_ctx", "__s2pkg_clients", "__s2pkg_server", "__s2pkg_chat",
    "__s2pkg_timers", "__s2_ui_pool_claim", "__s2_ui_pool_release", "__s2pkg_gamekit_layout", "__s2gk_claims"]) {
    delete globalThis[k];
  }
});

test("the layout is created lazily, not at plugin load", () => {
  const m = mount();
  assert.equal(m.hudCalls, 0);
  m.GameHud.toast("all", { title: "Hi" });
  assert.equal(m.hudCalls, 1);
});

test("a region is held by one owner until it is released", () => {
  const m = mount();
  const rail = m.GameHud.rail();
  assert.ok(rail);
  assert.equal(m.GameHud.rail(), rail, "same plugin gets the same claim back");
  // Another plugin's context sees the host claim as taken.
  globalThis.__s2_ui_pool_claim("gamekit:status", 1);
  assert.equal(m.GameHud.status(), null);
  rail.release();
  assert.equal(rail.isValid(), false);
  assert.equal(m.claims["gamekit:rail"], undefined);
  assert.equal(m.hidden(0, "gk_rail"), true);
  assert.throws(() => rail.set("all", {}), /released/);
});

test("rail paints sides, tones from leading tags, and pips per audience", () => {
  const m = mount();
  const rail = m.GameHud.rail();
  rail.set([1], {
    left: { score: 3, label: "{green}Innocents", alive: 2, total: 4 },
    right: { score: 1, label: "Traitors", tone: "red", alive: 1, total: 1 },
    clock: "1:42",
    sub: "{grey}Round 4 of 12",
  });
  assert.equal(m.hidden(1, "gk_rail"), false);
  assert.equal(m.hidden(0, "gk_rail"), true, "slot 0 was not in the audience (claim cleared it)");
  assert.equal(m.text(1, "gk_rail_llab"), "Innocents");
  assert.equal(m.text(1, "gk_rail_sub"), "Round 4 of 12");
  assert.equal(m.has(1, "gk_rail_l", "gk-e-green"), true);
  assert.equal(m.has(1, "gk_rail_r", "gk-e-red"), true);
  assert.equal(m.hidden(1, "gk_rail_lp3"), false);
  assert.equal(m.hidden(1, "gk_rail_lp4"), true);
  assert.equal(m.has(1, "gk_rail_lp1", "gk-off"), false);
  assert.equal(m.has(1, "gk_rail_lp2", "gk-off"), true);
  rail.set(1, { right: { tone: "blue" } });
  assert.equal(m.has(1, "gk_rail_r", "gk-e-red"), false, "the old tone is removed");
  assert.equal(m.has(1, "gk_rail_r", "gk-e-blue"), true);
});

test("status card sets the role tone, bars and meters", () => {
  const m = mount();
  const st = m.GameHud.status();
  st.set((slot) => slot === 2, { role: "{red}Traitor", hint: "Shop: {blue}!buy", bars: [{ label: "HP", value: 74 }] });
  assert.equal(m.text(2, "gk_status_role"), "Traitor");
  assert.equal(m.has(2, "gk_status_role", "gk-t-red"), true);
  assert.equal(m.text(2, "gk_bar0_val"), "74");
  assert.equal(m.meters["2|bar0"], 74);
  assert.equal(m.hidden(2, "gk_bar1"), true);
  assert.equal(m.hidden(1, "gk_status"), true);
});

test("kill feed keeps six rows per player, newest first, and expires rows", () => {
  const m = mount();
  const feed = m.GameHud.feed();
  for (let i = 0; i < 8; i++) feed.push({ attacker: "A" + i, victim: "V" + i, weapon: "ak47" });
  assert.equal(m.text(0, "gk_kf0_v"), "V7");
  assert.equal(m.text(0, "gk_kf5_v"), "V2");
  feed.push({ victim: "{lightred}Solo", tag: "{red}RDM", attackerSlot: 1 }, [1]);
  assert.equal(m.text(1, "gk_kf0_v"), "Solo");
  assert.equal(m.has(1, "gk_kf0_v", "gk-t-lightred"), true);
  assert.equal(m.has(1, "gk_kf0", "gk-e-red"), true);
  assert.equal(m.has(1, "gk_kf0", "gk-mine"), true);
  assert.equal(m.hidden(1, "gk_kf0_a"), true, "no attacker, no attacker label");
  assert.equal(m.text(0, "gk_kf0_v"), "V7", "slot 0 was not in that row's audience");
  m.runTimers();
  assert.equal(m.hidden(0, "gk_kf0"), true);
  assert.equal(m.hidden(1, "gk_kf0"), true);
});

test("a refused paint falls back to chat; a stale client does not", () => {
  const m = mount({ refuse: [2], stale: [1] });
  m.GameHud.feed().push({ attacker: "{green}Ann", victim: "{red}Bob", weapon: "awp", headshot: true });
  m.GameHud.toast("all", { title: "{orange}Body found", sub: "Bob was a traitor" });
  assert.deepEqual(m.chat.map((c) => c.slot), [2, 2]);
  assert.match(m.chat[0].msg, /\{green\}Ann .*awp, headshot.*\{red\}Bob/);
  assert.equal(m.chat[1].msg, "{orange}Body found {grey}Bob was a traitor");
});

test("toast hides after its time unless a newer toast replaced it", () => {
  const m = mount();
  m.GameHud.toast(0, { title: "One" });
  m.GameHud.toast(0, { title: "Two", tone: "yellow" });
  assert.equal(m.has(0, "gk_toast", "gk-e-yellow"), true);
  m.timers[0].fn();
  assert.equal(m.hidden(0, "gk_toast"), false, "the first toast's timer must not hide the second");
  m.timers[1].fn();
  assert.equal(m.hidden(0, "gk_toast"), true);
});

test("theme swaps one class and rejects unknown names", () => {
  const m = mount();
  m.GameHud.theme("all", "red");
  m.GameHud.theme(0, "teal");
  assert.equal(m.has(0, "gk_theme", "gk-theme-red"), false);
  assert.equal(m.has(0, "gk_theme", "gk-theme-teal"), true);
  assert.equal(m.has(1, "gk_theme", "gk-theme-red"), true);
  assert.throws(() => m.GameHud.theme("all", "ttt"), /unknown theme/);
});

test("map start forgets applied classes so the next paint re-sends them", () => {
  const m = mount();
  m.GameHud.theme(0, "red");
  // The new map's entity starts from the layout defaults.
  m.panels["0|gk_theme"].classes.clear();
  m.mapStart();
  m.GameHud.theme(0, "red");
  assert.equal(m.has(0, "gk_theme", "gk-theme-red"), true);
});

test("disconnect drops that player's feed rows", () => {
  const m = mount();
  const feed = m.GameHud.feed();
  feed.push({ victim: "X" }, [0]);
  m.disconnect(0);
  feed.push({ victim: "Y" }, [0]);
  assert.equal(m.text(0, "gk_kf0_v"), "Y");
  assert.equal(m.hidden(0, "gk_kf1"), true);
});

test("GameHud needs plugin context", () => {
  globalThis.__s2pkg_cs2 = {};
  globalThis.__s2pkg_game_ctx = { ui: () => ({ hud() {} }) };
  evalFile("gamekit.generated.js");
  evalFile("gamekit.js");
  assert.throws(() => globalThis.__s2pkg_cs2.GameHud.rail(), /requires plugin context/);
});

test("theme clears the layout's default theme and another plugin's theme", () => {
  const m = mount();
  // The layout ships gk-theme-amber; another plugin's context painted red.
  m.panels["0|gk_theme"] = { classes: new Set(["gk-theme-amber", "gk-theme-red"]), text: undefined };
  m.GameHud.theme(0, "teal");
  assert.deepEqual([...m.panels["0|gk_theme"].classes], ["gk-theme-teal"]);
});

test("a toast clears an edge another plugin left on the shared toast", () => {
  const m = mount();
  m.panels["0|gk_toast"] = { classes: new Set(["gk-e-red"]), text: undefined };
  m.GameHud.toast(0, { title: "Hi", tone: "green" });
  assert.equal(m.has(0, "gk_toast", "gk-e-red"), false);
  assert.equal(m.has(0, "gk_toast", "gk-e-green"), true);
});

test("a status paint without bars leaves the bars alone", () => {
  const m = mount();
  const st = m.GameHud.status();
  st.set(0, { role: "Guard", bars: [{ label: "HP", value: 80 }] });
  st.set(0, { hint: "Open cells at 0:30" });
  assert.equal(m.hidden(0, "gk_bar0"), false);
  assert.equal(m.text(0, "gk_bar0_val"), "80");
  st.set(0, { bars: [] });
  assert.equal(m.hidden(0, "gk_bar0"), true);
});

test("weapon icon paths keep their Panorama tokens", () => {
  const m = mount();
  m.GameHud.feed().push({ victim: "V", icon: "file://{images}/icons/equipment/ak47.svg" }, [0]);
  assert.equal(m.text(0, "gk_kf0_wi"), "file://{images}/icons/equipment/ak47.svg");
});
