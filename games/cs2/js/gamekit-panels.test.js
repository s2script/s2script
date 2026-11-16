const test = require("node:test");
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
  globalThis.__s2_chat_on_message = (fn) => { chatSubs.push(fn); return 9; };
  globalThis.__s2_ui_pool_claim = (kind) => { if (claims[kind]) return -1; claims[kind] = true; return 0; };
  globalThis.__s2_ui_pool_release = (kind) => { delete claims[kind]; return true; };

  evalFile("gamekit.generated.js");
  evalFile("gamekit.js");
  evalFile("gamekit-panels.js");
  globalThis.__s2pkg_game_ctx.ui((thunk) => thunk(), (call) => () => call());
  const GameHud = globalThis.__s2pkg_cs2.GameHud;
  return {
    GameHud, meters, chat, timers, claims, cursors, clicks, armed, disabled,
    hidden: (slot, id) => panel(slot, id).classes.has("s2-hide"),
    has: (slot, id, cls) => panel(slot, id).classes.has(cls),
    text: (slot, id) => panel(slot, id).text,
    say: (slot, text) => chatSubs.reduce((r, fn) => fn(slot, text) || r, undefined),
    click: (id, slot) => clicks[id]({ slot }),
    disconnect: (slot) => disconnect({ slot }),
    mapStart: () => mapStart(),
    tick: () => { for (const t of timers.splice(0)) t.fn(); },
  };
}

test.afterEach(() => {
  for (const k of ["__s2pkg_cs2", "__s2pkg_game_ctx", "__s2pkg_clients", "__s2pkg_server", "__s2pkg_chat",
    "__s2pkg_timers", "__s2pkg_hudinput", "__s2_chat_on_message", "__s2_ui_pool_claim", "__s2_ui_pool_release",
    "__s2pkg_gamekit_layout", "__s2pkg_gamekit_util", "__s2gk_claims"]) {
    delete globalThis[k];
  }
});

test("GameHud keeps the in-round API and gains vote and drawer", () => {
  const m = mount();
  for (const k of ["rail", "status", "feed", "toast", "theme", "vote", "drawer"]) {
    assert.equal(typeof m.GameHud[k], "function", k);
  }
  assert.ok(Object.isFrozen(m.GameHud));
});

test("a vote pops up without the cursor and arms Tab for voters only", () => {
  const m = mount();
  const v = m.GameHud.vote({ title: "Map vote", question: "Next map?", options: ["Dust", "{green}Mirage", "Inferno"], voters: [0, 1], viewers: "all" });
  assert.ok(v);
  assert.equal(m.claims["gamekit:vote"], true);
  assert.equal(m.hidden(0, "gk_vote"), false);
  assert.equal(m.hidden(2, "gk_vote"), false, "a viewer sees the vote");
  assert.equal(m.cursors["0|gk_vote"], undefined, "the vote does not grab the cursor");
  assert.ok(m.armed[0] && m.armed[1]);
  assert.equal(m.armed[2], undefined);
  assert.equal(m.text(0, "gk_vote_q"), "Next map?");
  assert.equal(m.text(0, "gk_vo1_t"), "Mirage");
  assert.equal(m.has(0, "gk_vo1_t", "gk-t-green"), true);
  assert.equal(m.hidden(0, "gk_vo3"), true, "unused rows are hidden");
  assert.equal(m.text(0, "gk_vo0_c"), "", "counts stay hidden until you vote");
  assert.equal(m.disabled["2|gk_vo0"], true, "a watcher cannot vote");
  assert.match(m.text(0, "gk_vote_ft"), /Type 1-3 in chat/);
  m.armed[0].onActivate();
  assert.equal(m.cursors["0|gk_vote"], true, "Tab hands over the cursor");
  assert.equal(m.GameHud.vote({ question: "x", options: ["a", "b"] }), null, "one vote at a time");
});

test("a typed number casts the vote and the chat line is swallowed", () => {
  const m = mount();
  const casts = [];
  m.GameHud.vote({ question: "Q", options: ["A", "B"], voters: [0, 1], onVote: (s, i) => casts.push([s, i]) });
  assert.equal(m.say(0, " 2 "), 2, "HANDLED");
  assert.deepEqual(casts, [[0, 1]]);
  assert.equal(m.has(0, "gk_vo1", "gk-sel"), true);
  assert.equal(m.text(0, "gk_vo1_c"), "1");
  assert.equal(m.armed[0], undefined, "voting disarms Tab");
  assert.match(m.text(0, "gk_vote_ft"), /Voted B/);
  assert.equal(m.say(0, "9"), undefined, "out of range passes through");
  assert.equal(m.say(0, "hello"), undefined);
  assert.equal(m.say(2, "1"), undefined, "a non-voter's chat passes through");
});

test("a click casts, the timer counts down, and the winner is reported", () => {
  const m = mount();
  let end = null;
  m.GameHud.vote({ question: "Q", options: ["A", "B", "C"], voters: [0, 1, 2], seconds: 2, onEnd: (r) => { end = r; } });
  m.click("gk_vo2", 0);
  m.click("gk_vo2", 1);
  m.click("gk_vo0", 2);
  assert.equal(m.meters["0|vo2"], (2 / 3) * 100);
  m.tick();
  assert.equal(m.text(0, "gk_vote_right"), "1s");
  assert.equal(m.meters["0|voteTimer"], 50);
  m.tick();
  assert.deepEqual(end, { counts: [1, 0, 2], winner: 2, votes: { 0: 2, 1: 2, 2: 0 } });
  assert.equal(m.hidden(0, "gk_vote"), true);
  assert.equal(m.claims["gamekit:vote"], undefined, "the claim is released");
  assert.equal(m.say(0, "1"), undefined, "chat passes through once the vote ends");
});

test("a tie or an empty vote has no winner; cancel skips onEnd", () => {
  const m = mount();
  let end = null;
  const v = m.GameHud.vote({ question: "Q", options: ["A", "B"], voters: [0, 1], onEnd: (r) => { end = r; } });
  v.cast(0, 0);
  v.cast(1, 1);
  v.end();
  assert.equal(end.winner, null);
  end = "untouched";
  const w = m.GameHud.vote({ question: "Q", options: ["A", "B"], onEnd: (r) => { end = r; } });
  w.cancel();
  assert.equal(end, "untouched");
  assert.equal(w.isOpen(), false);
});

test("a vote the HUD refuses goes to chat once", () => {
  const m = mount({ refuse: [1] });
  m.GameHud.vote({ title: "Kick", question: "Kick bob?", options: ["Yes", "No"], voters: [0, 1] });
  const mine = m.chat.filter((c) => c.slot === 1).map((c) => c.msg);
  assert.ok(mine.some((t) => /Kick bob\?/.test(t)));
  assert.ok(mine.some((t) => /2\{grey\}\. No/.test(t)));
  const before = m.chat.length;
  m.tick();
  assert.equal(m.chat.length, before, "not repeated every tick");
  assert.equal(m.say(1, "1"), 2, "typed votes still count");
});

test("the vote needs 2 to 8 options", () => {
  const m = mount();
  assert.throws(() => m.GameHud.vote({ question: "Q", options: ["A"] }), /between 2 and 8/);
  assert.throws(() => m.GameHud.vote({ options: ["A", "B"] }), /question/);
});

test("a drawer takes the cursor and paints tabs, rows and a close button", () => {
  const m = mount();
  const d = m.GameHud.drawer(1, {
    title: "Shop", right: "{yellow}3 credits", footer: "Click to buy",
    tabs: [
      { id: "gear", label: "Gear", rows: [{ title: "Radar", meta: "1", image: "file://{images}/x.svg" }, { title: "Armour", disabled: true }] },
      { id: "guns", label: "Guns", rows: [{ title: "{red}AWP", sub: "One shot" }] },
    ],
  });
  assert.ok(d && d.isOpen());
  assert.equal(m.claims["gamekit:drawer:1"], true);
  assert.equal(m.cursors["1|gk_drawer"], true);
  assert.equal(m.text(1, "gk_dr_right"), "3 credits");
  assert.equal(m.text(1, "gk_dr_tab1_t"), "Guns");
  assert.equal(m.has(1, "gk_dr_tab0", "gk-sel"), true);
  assert.equal(m.hidden(1, "gk_dr_tab2"), true);
  assert.equal(m.text(1, "gk_dr_r0_img"), "file://{images}/x.svg", "image tokens are kept");
  assert.equal(m.has(1, "gk_dr_r1", "s2-btn-disabled"), true);
  assert.equal(m.disabled["1|gk_dr_r1"], true);
  assert.equal(m.hidden(1, "gk_dr_r2"), true);
  assert.equal(m.text(1, "gk_dr_close_t"), "Close");
  assert.equal(m.GameHud.drawer(1, { title: "Again" }), d, "reopening repaints the same drawer");
});

test("drawer tabs switch, rows select, disabled rows do nothing, close frees the claim", () => {
  const m = mount();
  const picked = [];
  const tabs = [];
  let closed = null;
  m.GameHud.drawer(0, {
    title: "Shop",
    tabs: [
      { id: "gear", label: "Gear", rows: [{ title: "Radar", id: "radar" }, { title: "Armour", disabled: true }] },
      { id: "guns", label: "Guns", rows: [{ title: "AWP", id: "awp" }] },
    ],
    onSelect: (row) => picked.push(row.id),
    onTab: (id) => tabs.push(id),
    onClose: (fromPlayer) => { closed = fromPlayer; },
  });
  m.click("gk_dr_r1", 0);
  assert.deepEqual(picked, []);
  m.click("gk_dr_tab1", 0);
  assert.deepEqual(tabs, ["guns"]);
  assert.equal(m.text(0, "gk_dr_r0_t"), "AWP");
  assert.equal(m.hidden(0, "gk_dr_r1"), true);
  m.click("gk_dr_r0", 0);
  assert.deepEqual(picked, ["awp"]);
  m.click("gk_dr_r0", 1);
  assert.deepEqual(picked, ["awp"], "another player's click does nothing");
  m.click("gk_dr_close", 0);
  assert.equal(closed, true);
  assert.equal(m.hidden(0, "gk_drawer"), true);
  assert.equal(m.claims["gamekit:drawer:0"], undefined);
});

test("a drawer opens on the tab asked for and falls back to chat when refused", () => {
  const m = mount({ refuse: [2] });
  m.GameHud.drawer(2, { title: "Rules", tab: "b", tabs: [{ id: "a", label: "A", rows: [{ title: "One" }] }, { id: "b", label: "B", rows: [{ title: "Two", meta: "5" }] }] });
  const lines = m.chat.filter((c) => c.slot === 2).map((c) => c.msg);
  assert.ok(lines.some((t) => /Rules/.test(t)));
  assert.ok(lines.some((t) => /Two \{yellow\}5/.test(t)));
});

test("disconnect and map start release votes and drawers", () => {
  const m = mount();
  m.GameHud.drawer(0, { title: "A" });
  m.disconnect(0);
  assert.equal(m.claims["gamekit:drawer:0"], undefined);
  m.GameHud.drawer(1, { title: "B" });
  m.GameHud.vote({ question: "Q", options: ["A", "B"] });
  m.mapStart();
  assert.equal(m.claims["gamekit:drawer:1"], undefined);
  assert.equal(m.claims["gamekit:vote"], undefined);
});
