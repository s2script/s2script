const test = require("node:test");
const assert = require("node:assert/strict");
const { mount: mountWith, cleanup } = require("./gamekit-fake.js");

const mount = (opts = {}) => mountWith(Object.assign({ files: ["gamekit-panels.js", "gamekit-sheet.js"] }, opts));

test.afterEach(cleanup);

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

test("a partial drawer update keeps rows and callbacks; a refused drawer is listed in chat once", () => {
  const m = mount({ refuse: [2] });
  const picked = [];
  const d = m.GameHud.drawer(0, { title: "Shop", right: "1", rows: [{ title: "Radar", id: "radar" }], onSelect: (r) => picked.push(r.id) });
  d.update({ right: "{yellow}2 credits" });
  assert.equal(m.text(0, "gk_dr_right"), "2 credits");
  assert.equal(m.text(0, "gk_dr_title"), "Shop");
  assert.equal(m.text(0, "gk_dr_r0_t"), "Radar");
  m.click("gk_dr_r0", 0);
  assert.deepEqual(picked, ["radar"]);
  const r = m.GameHud.drawer(2, { title: "Shop", rows: [{ title: "Radar" }] });
  const n = m.chat.length;
  assert.ok(n > 0);
  r.update({ right: "x" });
  r.setTab("none");
  assert.equal(m.chat.length, n, "not repeated");
});

test("a disconnect closes the drawer through onClose", () => {
  const m = mount();
  let closed = null;
  m.GameHud.drawer(1, { title: "A", onClose: (p) => { closed = p; } });
  m.disconnect(1);
  assert.equal(closed, false);
  assert.equal(m.claims["gamekit:drawer:1"], undefined);
});
