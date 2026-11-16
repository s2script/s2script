const test = require("node:test");
const assert = require("node:assert/strict");
const { mount: mountWith, cleanup } = require("./gamekit-fake.js");

const FILES = ["gamekit-panels.js", "gamekit-sheet.js", "gamekit-hub.js"];
const mount = (opts = {}) => mountWith(Object.assign({ files: FILES }, opts));

test.afterEach(cleanup);

const players = [
  { slot: 1, name: "{green}alice", sub: "Innocent" },
  { slot: 2, name: "bob", sub: "Traitor", tone: "red" },
];

test("GameHud has the whole kit", () => {
  const m = mount();
  for (const k of ["rail", "status", "feed", "toast", "summary", "hideSummary", "theme", "vote", "drawer", "sheet", "prompt", "intro", "spectate"]) {
    assert.equal(typeof m.GameHud[k], "function", k);
  }
});

test("the intro paints the brand, menu and card, takes the cursor and reports picks", () => {
  const m = mount();
  const picked = [];
  const h = m.GameHud.intro(0, {
    eyebrow: "Welcome to", title: "Trouble Town", sub: "Find the traitors",
    items: [{ id: "play", label: "{orange}Play" }, { id: "spec", label: "Spectate", meta: "3 watching" }, { id: "x", label: "Soon", disabled: true }],
    card: { heading: "You", name: "gabe", sub: "Level 4", fields: [{ key: "Karma", value: 1000 }] },
    onSelect: (it) => picked.push(it.id),
  });
  assert.ok(h.isOpen());
  assert.equal(h.kind(), "intro");
  assert.equal(m.claims["gamekit:hub:0"], true);
  assert.equal(m.cursors["0|gk_hub"], true);
  assert.equal(m.text(0, "gk_hub_title"), "Trouble Town");
  assert.equal(m.text(0, "gk_hub_list_title"), "Menu");
  assert.equal(m.text(0, "gk_hub_i0_t"), "Play");
  assert.equal(m.has(0, "gk_hub_i0_t", "gk-t-orange"), true);
  assert.equal(m.text(0, "gk_hub_i1_m"), "3 watching");
  assert.equal(m.disabled["0|gk_hub_i2"], true);
  assert.equal(m.hidden(0, "gk_hub_i3"), true);
  assert.equal(m.hidden(0, "gk_hub_card"), false);
  assert.equal(m.text(0, "gk_hub_kv0_v"), "1000");
  assert.equal(m.hidden(0, "gk_hub_kv1"), true);
  assert.equal(m.hidden(0, "gk_hub_pick"), true, "no picker on the intro");
  assert.equal(m.hidden(0, "gk_hub_shot"), true);
  m.click("gk_hub_i0", 0);
  m.click("gk_hub_i2", 0);
  assert.deepEqual(picked, ["play"]);
  assert.deepEqual(m.commands, [], "the intro never moves the camera");
});

test("the spectate hub replaces the intro, picks players and follows them", () => {
  const m = mount();
  let closes = 0;
  m.GameHud.intro(0, { title: "T", onClose: () => closes++ });
  const picks = [];
  const h = m.GameHud.spectate(0, { title: "Spectating", players, onPick: (p) => picks.push(p.slot) });
  assert.equal(h.kind(), "spectate");
  assert.equal(closes, 0, "switching is not a close");
  assert.equal(m.text(0, "gk_hub_list_title"), "Spectating");
  assert.equal(m.hidden(0, "gk_hub_pick"), false);
  assert.equal(m.text(0, "gk_hub_p0_t"), "alice");
  assert.equal(m.has(0, "gk_hub_p0", "gk-e-green"), true);
  assert.equal(m.has(0, "gk_hub_p1", "gk-e-red"), true);
  assert.equal(m.hidden(0, "gk_hub_p2"), true);
  m.click("gk_hub_p1", 0);
  assert.deepEqual(picks, [2]);
  assert.deepEqual(m.commands.map((c) => c.cmd), ["spec_mode 2", "spec_player 3"]);
  assert.equal(m.has(0, "gk_hub_p1", "gk-sel"), true);
  assert.equal(m.text(0, "gk_hub_shot_t"), "Following bob");
  h.follow(1, "chase");
  assert.deepEqual(m.commands.slice(2).map((c) => c.cmd), ["spec_mode 3", "spec_player 2"]);
  assert.equal(m.has(0, "gk_hub_p1", "gk-sel"), false);
});

test("a shot list runs fixed and follow shots and advances on a timer", () => {
  const m = mount();
  const seen = [];
  const h = m.GameHud.spectate(0, {
    title: "Watch", players,
    shots: [
      { id: "over", kind: "fixed", label: "Overview", where: "Above mid", position: { x: 10, y: -20.5, z: 300 }, angles: { pitch: 60, yaw: 90 } },
      { id: "bob", kind: "follow", label: "Bob's view", target: 2, mode: "eye" },
    ],
    shotSeconds: 5,
    onShot: (s) => seen.push(s.id),
  });
  assert.deepEqual(m.commands.map((c) => c.cmd), ["spec_mode 4", "spec_goto 10 -20.5 300 60 90"]);
  assert.equal(m.text(0, "gk_hub_shot_t"), "Overview");
  assert.equal(m.text(0, "gk_hub_shot_w"), "Above mid");
  assert.equal(m.timers.at(-1).ms, 5000);
  m.tick();
  assert.deepEqual(m.commands.slice(2).map((c) => c.cmd), ["spec_mode 2", "spec_player 3"]);
  assert.equal(m.text(0, "gk_hub_shot_t"), "Bob's view");
  assert.equal(m.has(0, "gk_hub_p1", "gk-sel"), true);
  m.tick();
  assert.deepEqual(seen, ["over", "bob", "over"]);
  // Picking a player stops the timer: the next tick does nothing.
  m.click("gk_hub_p0", 0);
  const n = m.commands.length;
  m.tick();
  assert.equal(m.commands.length, n);
  h.shot("bob");
  assert.equal(m.text(0, "gk_hub_shot_t"), "Bob's view");
});

test("spectate with cursor: false leaves the mouse and arms Tab", () => {
  const m = mount();
  m.GameHud.intro(0, { title: "T" });
  assert.equal(m.cursors["0|gk_hub"], true);
  m.GameHud.spectate(0, { title: "S", cursor: false, players });
  assert.equal(m.cursors["0|gk_hub"], undefined, "switching from the intro releases the cursor");
  assert.ok(m.armed[0]);
  m.armed[0].onActivate();
  assert.equal(m.cursors["0|gk_hub"], true);
});

test("close frees the claim; another plugin's hub blocks; refused goes to chat", () => {
  const m = mount({ refuse: [2] });
  let from = null;
  m.GameHud.intro(0, { title: "T", onClose: (f) => { from = f; } });
  m.click("gk_hub_list_close", 0);
  assert.equal(from, true);
  assert.equal(m.claims["gamekit:hub:0"], undefined);
  m.GameHud.intro(0, { title: "T", closeable: false });
  assert.equal(m.hidden(0, "gk_hub_list_close"), true);
  m.click("gk_hub_list_close", 0);
  assert.equal(m.claims["gamekit:hub:0"], true, "not closeable");
  globalThis.__s2_ui_pool_claim("gamekit:hub:1", 1);
  assert.equal(m.GameHud.intro(1, { title: "T" }), null);
  m.GameHud.intro(2, { title: "Trouble Town", items: ["Play", "Rules"] });
  const lines = m.chat.filter((c) => c.slot === 2).map((c) => c.msg);
  assert.deepEqual(lines, ["{orange}Trouble Town", "{grey}- Play", "{grey}- Rules"]);
});

test("disconnect and map start release the hub and stop the shot timer", () => {
  const m = mount();
  m.GameHud.spectate(0, { title: "S", shots: [{ kind: "follow", target: 1 }, { kind: "follow", target: 2 }], shotSeconds: 3 });
  m.disconnect(0);
  assert.equal(m.claims["gamekit:hub:0"], undefined);
  const n = m.commands.length;
  m.tick();
  assert.equal(m.commands.length, n);
  m.GameHud.intro(1, { title: "T" });
  m.mapStart();
  assert.equal(m.claims["gamekit:hub:1"], undefined);
});

test("the round-end summary paints stats and a table for its audience, then hides", () => {
  const m = mount();
  m.GameHud.summary([0, 1], {
    eyebrow: "Round 4", title: "{red}Traitors win", sub: "All innocents died",
    stats: [{ key: "Kills", value: 7 }, { key: "Time", value: "2:13" }],
    table: { columns: ["Player", "Role", "Kills"], rows: [{ cells: ["bob", "Traitor", 4] }, ["alice", "Innocent", 1]] },
    seconds: 6,
  });
  assert.equal(m.hidden(0, "gk_sum"), false);
  assert.equal(m.text(0, "gk_sum_t"), "Traitors win");
  assert.equal(m.has(0, "gk_sum_t", "gk-t-red"), true);
  assert.equal(m.has(0, "gk_sum", "gk-e-red"), true);
  assert.equal(m.text(1, "gk_sum_kv1_v"), "2:13");
  assert.equal(m.hidden(1, "gk_sum_kv2"), true);
  assert.equal(m.text(0, "gk_sum_th2"), "Kills");
  assert.equal(m.hidden(0, "gk_sum_th3"), true);
  assert.equal(m.text(0, "gk_sum_tr1_c0"), "alice");
  assert.equal(m.hidden(0, "gk_sum_tr2"), true);
  assert.equal(m.text(2, "gk_sum_t"), undefined, "slot 2 was not in the audience");
  m.GameHud.summary(0, { title: "Innocents win", tone: "green" });
  assert.equal(m.has(0, "gk_sum_t", "gk-t-red"), false, "the old tone is cleared");
  assert.equal(m.hidden(0, "gk_sum_table"), true);
  const sumTimers = m.timers.splice(0);
  sumTimers[0].fn(); // the first summary's timer for slot 0
  assert.equal(m.hidden(0, "gk_sum"), false, "an old timer does not hide the newer summary");
  sumTimers[1].fn();
  assert.equal(m.hidden(1, "gk_sum"), true);
  sumTimers[2].fn();
  assert.equal(m.hidden(0, "gk_sum"), true);
});

test("a refused summary goes to chat as one line", () => {
  const m = mount({ refuse: [1] });
  m.GameHud.summary(1, { title: "Draw", stats: [{ key: "Rounds", value: 12 }] });
  assert.deepEqual(m.chat.map((c) => c.msg), ["Draw {grey}Rounds {yellow}12"]);
});
