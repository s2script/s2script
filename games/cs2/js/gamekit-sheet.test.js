const test = require("node:test");
const assert = require("node:assert/strict");
const { mount: mountWith, cleanup } = require("./gamekit-fake.js");

const mount = (opts = {}) => mountWith(Object.assign({ files: ["gamekit-panels.js", "gamekit-sheet.js"] }, opts));

test.afterEach(cleanup);

const rules = (n) => Array.from({ length: n }, (_, i) => `Rule text ${i + 1}`);

function rulesSpec(extra = {}) {
  return Object.assign({
    title: "Rules",
    sections: [
      { id: "general", label: "General", body: { kind: "list", items: rules(14) } },
      { id: "guard", label: "Guards", group: "Teams", body: { kind: "list", items: rules(3) } },
      { id: "rebel", label: "Rebels", group: "Teams", body: { kind: "list", items: [{ n: 1, text: "No camping" }, { n: 7, text: "Seven" }] } },
      { id: "about", label: "About", body: { kind: "text", paragraphs: ["One", "Two", "Three", "Four", "Five"] } },
    ],
  }, extra);
}

test("GameHud gains sheet and prompt and keeps the rest", () => {
  const m = mount();
  for (const k of ["rail", "status", "feed", "toast", "theme", "vote", "drawer", "sheet", "prompt"]) {
    assert.equal(typeof m.GameHud[k], "function", k);
  }
});

test("a sheet takes the cursor, groups side tabs and pages a long list six at a time", () => {
  const m = mount();
  const sh = m.GameHud.sheet(0, rulesSpec());
  assert.ok(sh && sh.isOpen());
  assert.equal(m.claims["gamekit:sheet:0"], true);
  assert.equal(m.cursors["0|gk_sheet"], true);
  // Side: General, [Teams], Guards, Rebels, About.
  assert.equal(m.text(0, "gk_sh_s0_t"), "General");
  assert.equal(m.has(0, "gk_sh_s0", "gk-sel"), true);
  assert.equal(m.text(0, "gk_sh_s1_t"), "Teams");
  assert.equal(m.has(0, "gk_sh_s1", "gk-grp"), true);
  assert.equal(m.text(0, "gk_sh_s4_t"), "About");
  assert.equal(m.hidden(0, "gk_sh_s5"), true);
  // Body: list, page 1 of 3.
  assert.equal(m.hidden(0, "gk_sh_list"), false);
  assert.equal(m.hidden(0, "gk_sh_para"), true);
  assert.equal(m.text(0, "gk_sh_k"), "Page 1 of 3");
  assert.equal(m.text(0, "gk_sh_li0_n"), "1");
  assert.equal(m.text(0, "gk_sh_li5_t"), "Rule text 6");
  assert.equal(m.hidden(0, "gk_sh_li6"), true);
  assert.equal(m.hidden(0, "gk_sh_pager"), false);
  assert.equal(m.disabled["0|gk_sh_prev"], true);
  assert.equal(m.has(0, "gk_sh_d0", "gk-on"), true);
  assert.equal(m.hidden(0, "gk_sh_d3"), true);
  m.click("gk_sh_next", 0);
  assert.equal(m.text(0, "gk_sh_li0_n"), "7", "numbers are stable across pages");
  m.click("gk_sh_next", 0);
  assert.equal(m.text(0, "gk_sh_li1_n"), "14");
  assert.equal(m.hidden(0, "gk_sh_li2"), true);
  assert.equal(m.disabled["0|gk_sh_next"], true);
});

test("a sheet opens on a section and highlights a rule by its number", () => {
  const m = mount();
  m.GameHud.sheet(1, rulesSpec({ section: "general", highlight: 9 }));
  assert.equal(m.text(1, "gk_sh_k"), "Page 2 of 3");
  assert.equal(m.text(1, "gk_sh_li2_n"), "9");
  assert.equal(m.has(1, "gk_sh_li2", "gk-hl"), true);
  const sh = m.GameHud.sheet(1, rulesSpec({ section: "rebel", highlight: 7 }));
  assert.equal(m.text(1, "gk_sh_h"), "Rebels");
  assert.equal(m.text(1, "gk_sh_li1_n"), "7", "an item's own n wins");
  assert.equal(m.has(1, "gk_sh_li1", "gk-hl"), true);
  assert.equal(m.has(1, "gk_sh_li0", "gk-hl"), false);
  sh.show("general");
  assert.equal(m.has(1, "gk_sh_li2", "gk-hl"), false, "show without a number clears the highlight");
});

test("clicking a tab or a group header switches section", () => {
  const m = mount();
  const seen = [];
  m.GameHud.sheet(0, rulesSpec({ onSection: (id) => seen.push(id) }));
  m.click("gk_sh_s1", 0); // the Teams header opens its first section
  assert.deepEqual(seen, ["guard"]);
  assert.equal(m.text(0, "gk_sh_h"), "Guards");
  assert.equal(m.has(0, "gk_sh_s2", "gk-sel"), true);
  m.click("gk_sh_s4", 0);
  assert.equal(m.hidden(0, "gk_sh_para"), false);
  assert.equal(m.hidden(0, "gk_sh_list"), true);
  assert.equal(m.text(0, "gk_sh_p3"), "Four");
  m.click("gk_sh_next", 0);
  assert.equal(m.text(0, "gk_sh_p0"), "Five");
  assert.equal(m.hidden(0, "gk_sh_p1"), true);
});

test("too many tabs collapse other groups, then scroll", () => {
  const m = mount();
  const sections = [];
  for (let g = 0; g < 4; g++) {
    for (let i = 0; i < 5; i++) sections.push({ id: `g${g}s${i}`, label: `G${g} S${i}`, group: `Group ${g}`, body: { kind: "list", items: ["x"] } });
  }
  m.GameHud.sheet(0, { title: "Big", sections, section: "g2s1" });
  // 4 headers + 5 tabs of group 2 = 9 entries.
  const labels = [];
  for (let i = 0; i < 16; i++) if (!m.hidden(0, `gk_sh_s${i}`)) labels.push(m.text(0, `gk_sh_s${i}_t`));
  assert.deepEqual(labels, ["Group 0", "Group 1", "Group 2", "G2 S0", "G2 S1", "G2 S2", "G2 S3", "G2 S4", "Group 3"]);

  const flat = Array.from({ length: 20 }, (_, i) => ({ id: `t${i}`, label: `T${i}`, body: { kind: "list", items: ["x"] } }));
  m.GameHud.sheet(1, { title: "Flat", sections: flat, section: "t19" });
  assert.equal(m.text(1, "gk_sh_s0_t"), "▲", "scrolled down to keep the open tab in view");
  assert.equal(m.text(1, "gk_sh_s15_t"), "T19", "nothing below: no down slot");
  const open = [];
  for (let i = 0; i < 16; i++) if (m.has(1, `gk_sh_s${i}`, "gk-sel")) open.push(m.text(1, `gk_sh_s${i}_t`));
  assert.deepEqual(open, ["T19"]);
  m.click("gk_sh_s0", 1);
  assert.equal(m.text(1, "gk_sh_s0_t"), "T0", "the up slot scrolls back to the top");
  assert.equal(m.text(1, "gk_sh_s15_t"), "▼");
});

test("toggles, grid cards and table actions report what was clicked", () => {
  const m = mount();
  const ev = [];
  m.GameHud.sheet(2, {
    title: "Settings",
    sections: [
      { id: "opts", label: "Options", body: { kind: "toggles", items: [{ id: "music", label: "Music", on: true }, { id: "x", label: "Locked", disabled: true }] } },
      { id: "shop", label: "Shop", body: { kind: "grid", items: [{ id: "radar", title: "Radar", price: 2, desc: "See them", selected: true }] } },
      { id: "who", label: "Players", body: { kind: "table", columns: ["Name", "Karma"], rows: [{ id: "p7", cells: ["bob", 950], actions: [{ id: "slay", label: "{red}Slay" }] }] } },
    ],
    onSelect: (e) => ev.push(e),
  });
  assert.equal(m.has(2, "gk_sh_tg0", "gk-on"), true);
  assert.equal(m.text(2, "gk_sh_tg0_v"), "On");
  assert.equal(m.disabled["2|gk_sh_tg1"], true);
  m.click("gk_sh_tg0", 2);
  m.click("gk_sh_tg1", 2);
  m.click("gk_sh_s1", 2);
  assert.equal(m.text(2, "gk_sh_g0_p"), "2");
  assert.equal(m.has(2, "gk_sh_g0", "gk-sel"), true);
  m.click("gk_sh_g0", 2);
  m.click("gk_sh_s2", 2);
  assert.equal(m.text(2, "gk_sh_th1"), "Karma");
  assert.equal(m.hidden(2, "gk_sh_th2"), true);
  assert.equal(m.text(2, "gk_sh_tr0_c1"), "950");
  assert.equal(m.text(2, "gk_sh_tr0_x0_t"), "Slay");
  assert.equal(m.has(2, "gk_sh_tr0_x0_t", "gk-t-red"), true);
  assert.equal(m.hidden(2, "gk_sh_tr0_x1"), true);
  m.click("gk_sh_tr0_x0", 2);
  assert.deepEqual(ev.map((e) => [e.section, e.kind, e.item.id, e.on, e.action]), [
    ["opts", "toggle", "music", false, undefined],
    ["shop", "item", "radar", undefined, undefined],
    ["who", "action", "p7", undefined, "slay"],
  ]);
});

test("close frees the claim, and a refused sheet goes to chat", () => {
  const m = mount({ refuse: [1] });
  let closed = null;
  m.GameHud.sheet(0, rulesSpec({ onClose: (p) => { closed = p; } }));
  m.click("gk_sh_close", 0);
  assert.equal(closed, true);
  assert.equal(m.hidden(0, "gk_sheet"), true);
  assert.equal(m.claims["gamekit:sheet:0"], undefined);
  m.GameHud.sheet(1, rulesSpec({ section: "rebel" }));
  const lines = m.chat.filter((c) => c.slot === 1).map((c) => c.msg);
  assert.ok(lines[0].includes("Rebels"));
  assert.ok(lines.some((t) => t === "{yellow}7{grey}. Seven"));
});

test("a prompt takes the cursor, answers by click, and reports why it ended", () => {
  const m = mount();
  const got = [];
  const p = m.GameHud.prompt(0, {
    title: "Report", question: "Was that kill fair?", why: "You were killed by bob",
    answers: ["Fair", { id: "report", label: "{red}Report", tone: "red" }], onAnswer: (r) => got.push(r),
  });
  assert.ok(p.isOpen());
  assert.equal(m.claims["gamekit:prompt:0"], true);
  assert.equal(m.cursors["0|gk_prompt"], true);
  assert.equal(m.text(0, "gk_pr_q"), "Was that kill fair?");
  assert.equal(m.text(0, "gk_pr_a1_t"), "Report");
  assert.equal(m.has(0, "gk_pr_a1_t", "gk-t-red"), true);
  assert.equal(m.hidden(0, "gk_pr_a2"), true);
  assert.equal(m.hidden(0, "gk_pr_ft"), true);
  assert.equal(m.say(0, "2"), undefined, "a prompt on screen does not take typed numbers");
  m.click("gk_pr_a1", 0);
  assert.deepEqual(got, [{ index: 1, id: "report", reason: "answer" }]);
  assert.equal(m.hidden(0, "gk_prompt"), true);
  assert.equal(m.claims["gamekit:prompt:0"], undefined);

  m.GameHud.prompt(0, { question: "Q", answers: ["A"], onAnswer: (r) => got.push(r) });
  m.click("gk_pr_close", 0);
  assert.equal(got[1].reason, "closed");
});

test("a prompt times out, a new one replaces the old, and a refused one takes typed numbers", () => {
  const m = mount({ refuse: [2] });
  const got = [];
  m.GameHud.prompt(0, { question: "Q", answers: ["A", "B"], seconds: 2, onAnswer: (r) => got.push(r) });
  assert.equal(m.text(0, "gk_pr_right"), "2s");
  m.tick();
  assert.equal(m.text(0, "gk_pr_right"), "1s");
  m.tick();
  assert.deepEqual(got, [{ index: null, id: null, reason: "timeout" }]);

  m.GameHud.prompt(1, { question: "First", answers: ["A"], onAnswer: (r) => got.push(r) });
  m.GameHud.prompt(1, { question: "Second", answers: ["A"] });
  assert.equal(got[1].reason, "replaced");
  assert.equal(m.text(1, "gk_pr_q"), "Second");
  assert.equal(m.claims["gamekit:prompt:1"], true);

  m.GameHud.prompt(2, { question: "Kick him?", answers: ["Yes", "No"], onAnswer: (r) => got.push(r) });
  const lines = m.chat.filter((c) => c.slot === 2).map((c) => c.msg);
  assert.ok(lines.some((t) => t.includes("Kick him?")));
  assert.equal(m.say(2, "2"), 2);
  assert.deepEqual(got[2], { index: 1, id: "No", reason: "answer" });
  assert.throws(() => m.GameHud.prompt(0, { question: "Q", answers: [] }), /between 1 and 4/);
});

test("disconnect and map start release sheets and prompts", () => {
  const m = mount();
  m.GameHud.sheet(0, rulesSpec());
  m.GameHud.prompt(0, { question: "Q", answers: ["A"] });
  m.disconnect(0);
  assert.equal(m.claims["gamekit:sheet:0"], undefined);
  assert.equal(m.claims["gamekit:prompt:0"], undefined);
  m.GameHud.sheet(1, rulesSpec());
  m.GameHud.prompt(1, { question: "Q", answers: ["A"] });
  m.mapStart();
  assert.equal(m.claims["gamekit:sheet:1"], undefined);
  assert.equal(m.claims["gamekit:prompt:1"], undefined);
});

test("a sheet refresh keeps the page, scroll and callbacks", () => {
  const m = mount();
  const ev = [];
  const flat = Array.from({ length: 20 }, (_, i) => ({ id: `t${i}`, label: `T${i}`, body: { kind: "list", items: rules(14) } }));
  const sh = m.GameHud.sheet(0, { title: "Flat", sections: flat, onSection: (id) => ev.push(id) });
  m.click("gk_sh_s15", 0); // scroll down
  const top = m.text(0, "gk_sh_s1_t");
  assert.notEqual(top, "T1");
  m.click("gk_sh_next", 0);
  sh.update({ right: "refreshed" });
  assert.equal(m.text(0, "gk_sh_right"), "refreshed");
  assert.equal(m.text(0, "gk_sh_s1_t"), top, "side scroll kept");
  assert.equal(m.text(0, "gk_sh_li0_n"), "7", "page kept");
  assert.equal(m.text(0, "gk_sh_title"), "Flat");
  m.click("gk_sh_s1", 0);
  assert.equal(ev.length, 1, "callbacks kept");
});

test("a refused prompt's typed answer wins over a running vote, and disconnect ends sheets and prompts", () => {
  const m = mount({ refuse: [2] });
  const got = [];
  m.GameHud.vote({ question: "Q", options: ["A", "B"], voters: [2] });
  m.GameHud.prompt(2, { question: "Kick?", answers: ["Yes", "No"], onAnswer: (r) => got.push(r) });
  assert.equal(m.say(2, "2"), 2);
  assert.equal(got[0].index, 1);
  let closed = null;
  m.GameHud.sheet(0, { title: "S", sections: [{ id: "a", label: "A", body: { kind: "list", items: ["x"] } }], onClose: (p) => { closed = p; } });
  m.GameHud.prompt(0, { question: "Q", answers: ["A"], onAnswer: (r) => got.push(r) });
  m.disconnect(0);
  assert.equal(closed, false);
  assert.equal(got[1].reason, "cancelled");
});

// ── a repaint clears the previous body ───────────────────────────────────────────────────────
const shopSpec = () => ({
  title: "Shop",
  sections: [{ id: "shop", label: "Shop", body: { kind: "grid", items: [{ id: "radar", title: "Radar", price: 1 }, { id: "armor", title: "Armor", price: 2 }] } }],
});
const listRowsShown = (m, slot) => Array.from({ length: 8 }, (_, i) => `gk_sh_li${i}`).filter((id) => !m.hidden(slot, id));

test("a list sheet closed, then a grid sheet: no list row is left", () => {
  const m = mount({ diffCache: true });
  const rulesSheet = m.GameHud.sheet(0, rulesSpec());
  assert.equal(listRowsShown(m, 0).length, 6);
  rulesSheet.close();
  // Close empties the panel: every body, row, heading and the pager.
  assert.deepEqual(listRowsShown(m, 0), []);
  assert.equal(m.hidden(0, "gk_sh_list"), true);
  assert.equal(m.text(0, "gk_sh_li0_t"), "");
  assert.equal(m.text(0, "gk_sh_h"), "");
  assert.equal(m.hidden(0, "gk_sh_k"), true);
  assert.equal(m.hidden(0, "gk_sh_pager"), true);
  assert.equal(m.hidden(0, "gk_sh_s0"), true);
  m.GameHud.sheet(0, shopSpec());
  assert.deepEqual(listRowsShown(m, 0), []);
  assert.equal(m.hidden(0, "gk_sh_list"), true);
  assert.equal(m.hidden(0, "gk_sh_grid"), false);
  assert.equal(m.text(0, "gk_sh_g1_t"), "Armor");
  assert.equal(m.hidden(0, "gk_sh_g2"), true);
});

test("another plugin's list sheet closed, then this plugin's grid sheet: no list row is left", () => {
  // Two plugin contexts, one layout entity. TTT opened its shop earlier, so its diff cache says
  // the list body is hidden; the rules plugin then showed it. TTT's next shop must not trust that.
  const shared = { panels: {}, claims: {} };
  const rulesPlugin = mount({ diffCache: true, shared });
  const ttt = mount({ diffCache: true, shared });
  ttt.GameHud.sheet(0, shopSpec()).close();
  const rulesSheet = rulesPlugin.GameHud.sheet(0, rulesSpec());
  assert.equal(listRowsShown(rulesPlugin, 0).length, 6);
  assert.equal(rulesPlugin.hidden(0, "gk_sh_list"), false);
  assert.equal(rulesPlugin.hidden(0, "gk_sh_grid"), true);
  rulesSheet.close();
  ttt.GameHud.sheet(0, shopSpec());
  assert.deepEqual(listRowsShown(ttt, 0), []);
  assert.equal(ttt.hidden(0, "gk_sh_list"), true);
  assert.equal(ttt.hidden(0, "gk_sh_grid"), false);
  assert.equal(ttt.hidden(0, "gk_sh_g0"), false);
  assert.equal(ttt.text(0, "gk_sh_h"), "Shop");
  assert.equal(ttt.hidden(0, "gk_sh_pager"), true);
});

test("a plugin whose sheet was painted over by another repaints all of it on its next open", () => {
  // The rules plugin never closes cleanly here (e.g. an older runtime): only the fresh claim's
  // cache drop stands between its leftovers and TTT's shop.
  const shared = { panels: {}, claims: {} };
  const rulesPlugin = mount({ diffCache: true, shared });
  const ttt = mount({ diffCache: true, shared });
  ttt.GameHud.sheet(0, shopSpec()).close();
  rulesPlugin.GameHud.sheet(0, rulesSpec({ section: "general", highlight: 3 }));
  delete shared.claims["gamekit:sheet:0"]; // released without the close repaint
  ttt.GameHud.sheet(0, shopSpec());
  assert.equal(ttt.hidden(0, "gk_sh_list"), true);
  assert.deepEqual(listRowsShown(ttt, 0), []);
  assert.equal(ttt.hidden(0, "gk_sh_grid"), false);
  assert.equal(ttt.hidden(0, "gk_sh_s1"), true, "the rules plugin's extra side tabs are gone");
});

test("switching section to another body kind empties the old body's rows", () => {
  const m = mount({ diffCache: true });
  const sh = m.GameHud.sheet(0, {
    title: "Mixed",
    sections: [
      { id: "rules", label: "Rules", body: { kind: "list", items: rules(5) } },
      { id: "shop", label: "Shop", body: { kind: "grid", items: [{ id: "radar", title: "Radar" }] } },
      { id: "who", label: "Players", body: { kind: "table", columns: ["Name"], rows: [{ cells: ["bob"], actions: [{ label: "Slay", tone: "red" }] }] } },
    ],
    section: "rules",
    highlight: 2,
  });
  assert.equal(m.has(0, "gk_sh_li1", "gk-hl"), true);
  m.click("gk_sh_s1", 0);
  assert.deepEqual(listRowsShown(m, 0), []);
  assert.equal(m.text(0, "gk_sh_li0_t"), "", "the list rows are emptied, not only hidden");
  assert.equal(m.has(0, "gk_sh_li1", "gk-hl"), false);
  assert.equal(m.hidden(0, "gk_sh_g0"), false);
  sh.update({ section: "who" });
  assert.equal(m.hidden(0, "gk_sh_g0"), true);
  assert.equal(m.text(0, "gk_sh_g0_t"), "");
  assert.equal(m.text(0, "gk_sh_tr0_c0"), "bob");
  sh.show("rules");
  assert.equal(m.hidden(0, "gk_sh_tr0"), true);
  assert.equal(m.hidden(0, "gk_sh_tr0_x0"), true);
  assert.equal(listRowsShown(m, 0).length, 5);
});

test("a fresh sheet clears a tone another plugin left on a table action", () => {
  const shared = { panels: {}, claims: {} };
  const a = mount({ diffCache: true, shared });
  const b = mount({ diffCache: true, shared });
  const spec = (tone) => ({ title: "P", sections: [{ id: "who", label: "P", body: { kind: "table", columns: ["Name"], rows: [{ cells: ["bob"], actions: [{ label: "Go", tone }] }] } }] });
  a.GameHud.sheet(0, spec("red")).close();
  b.GameHud.sheet(0, spec("green"));
  assert.equal(b.has(0, "gk_sh_tr0_x0_t", "gk-t-green"), true);
  assert.equal(b.has(0, "gk_sh_tr0_x0_t", "gk-t-red"), false);
});
