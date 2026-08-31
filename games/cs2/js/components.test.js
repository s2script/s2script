// node:test suite for games/cs2/js/components.js — the ui.components() library.
//
// components.js is an ES5 IIFE that decorates the host's ui factory, so it cannot be
// `require`d. It is evaluated against a stub host that records every drive call, which is what
// lets these assertions check the CLASS NAMES and the intern accounting rather than just that
// nothing threw. A parse check would not have caught the missing-function bug this suite found.
const test = require("node:test");
const assert = require("node:assert");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

/** Evaluate components.js against a fresh stub host; returns the library plus the call log. */
function mount() {
  const calls = [];
  const clickHandlers = {};
  const hud = {
    set:      (s, id, v) => { calls.push({ op: "set", slot: s, id, value: v }); return null; },
    setClass: (s, id, cls, on) => { calls.push({ op: "cls", slot: s, id, cls, on }); return null; },
    show:     (s, id, opts) => { calls.push({ op: "show", slot: s, id, opts }); return null; },
    hide:     (s, id) => { calls.push({ op: "hide", slot: s, id }); return null; },
    cursor:   () => null,
    forget:   () => {},
    onClick:  (id, fn) => { clickHandlers[id] = fn; },
  };
  // A fresh global each mount: the pool is deliberately host-global (two plugins must not both
  // believe they own modal 0), so tests must not inherit each other's claims.
  delete globalThis.__s2ui_pool;
  globalThis.__s2pkg_game_ctx = { ui: () => ({ hud: () => hud }) };
  // The REAL shape: promises, no callback. The stub used to take a callback, which is exactly why
  // this suite did not catch the reveal never clearing its fade class.
  const pending = [];
  globalThis.__s2pkg_timers = {
    nextFrame: () => Promise.resolve(),
    delay: () => Promise.resolve(),
    after: (ms, fn) => { pending.push(fn); },
  };

  const src = readFileSync(join(__dirname, "components.js"), "utf8");
  new Function(src)();
  return { ui: globalThis.__s2pkg_game_ctx.ui({}, "test").components(), calls, clickHandlers, pending };
}

const classSet = (calls, cls) => calls.some((c) => c.op === "cls" && c.cls === cls && c.on === true);

test("a footer button's variant reaches a class (it was accepted and dropped)", () => {
  const { ui, calls } = mount();
  const m = ui.modal({ title: "T", rows: [], buttons: [{ text: "X", variant: "bad", onClick: () => {} }] });
  m.open(1);
  assert.ok(classSet(calls, "s2-btn-bad"), "variant must be applied, not merely stored");
});

test("each component family uses its own fade class", () => {
  const { ui, calls } = mount();
  ui.toast(1, { title: "t", message: "m", variant: "good", holdSeconds: 0 });
  assert.ok(calls.some((c) => c.op === "cls" && c.cls === "s2-toast-out"));
  // A shared "out" class would transition translatex on a sheet, which does not declare it —
  // the panel would pop instead of fading.
  assert.ok(!calls.some((c) => c.op === "cls" && c.cls === "s2-sheet-out"));
});

test("a badge sets exactly one corner class", () => {
  const { ui, calls } = mount();
  ui.badge({ corner: "tr", accent: "bad" }).show(1, { title: "A", text: "b" });
  const on = calls.filter((c) => c.op === "cls" && c.cls.startsWith("s2-corner-") && c.on === true);
  assert.strictEqual(on.length, 1);
  assert.strictEqual(on[0].cls, "s2-corner-tr");
  assert.ok(classSet(calls, "s2-hudbadge-bad"));
});

test("row clicks report an absolute index, not a page-relative one", () => {
  const { ui, clickHandlers } = mount();
  const picked = [];
  const rows = Array.from({ length: 20 }, (_, i) => ({ a: `i${i}` }));
  const m = ui.modal({ title: "T", rows: () => rows, onPick: (slot, i) => picked.push(i) });
  m.open(1);
  clickHandlers["s2_m0_r2"](1);
  m.page(1, 1);
  clickHandlers["s2_m0_r0"](1);
  assert.deepStrictEqual(picked, [2, 8], "page 2 row 0 is item 8");
});

test("a disabled row is greyed but still delivers its click", () => {
  const { ui, calls, clickHandlers } = mount();
  const picked = [];
  const m = ui.modal({
    title: "T", rows: [{ a: "no", disabled: true }], onPick: (slot, i) => picked.push(i),
  });
  m.open(1);
  assert.ok(classSet(calls, "s2-li-disabled"));
  clickHandlers["s2_m0_r0"](1);
  // Cosmetic-only by design: the caller refuses AND explains. A row that silently does nothing
  // reads as a broken menu.
  assert.deepStrictEqual(picked, [0]);
});

test("an empty list hides its container rather than painting blank rows", () => {
  const { ui, calls } = mount();
  ui.modal({ title: "Confirm?", rows: [] }).open(1);
  assert.ok(calls.some((c) => c.op === "hide" && c.id === "s2_m0_list"));
  assert.ok(calls.some((c) => c.op === "hide" && c.id === "s2_m0_detail"));
});

test("the modal pool refuses rather than colliding when exhausted", () => {
  const { ui } = mount();
  assert.ok(ui.modal({ title: "a", rows: [] }));
  assert.ok(ui.modal({ title: "b", rows: [] }));
  // Two plugins must never both believe they own the same pooled panel.
  assert.strictEqual(ui.modal({ title: "c", rows: [] }), null);
});

test("the three intern vectors are counted separately, at cap 1024", () => {
  const { ui } = mount();
  ui.modal({ title: "T", rows: [{ a: "x" }] }).open(1);
  const b = ui.budget();
  assert.strictEqual(b.cap, 1024, "one 1024 per vector, not one shared 3072");
  assert.ok(b.panelIds > 0 && b.classNames > 0 && b.variables > 0);
  // `set` spends from two different ledgers, so a single combined total would be wrong in both
  // directions: crying wolf when spread evenly, silent when one vector alone runs out.
  assert.notStrictEqual(b.panelIds, b.variables);
});

test("interning is idempotent — repainting costs nothing", () => {
  const { ui } = mount();
  const m = ui.modal({ title: "T", rows: [{ a: "x" }] });
  m.open(1);
  const before = ui.budget();
  for (let i = 0; i < 50; i++) m.refresh(1);
  const after = ui.budget();
  assert.deepStrictEqual(
    [after.panelIds, after.classNames, after.variables],
    [before.panelIds, before.classNames, before.variables],
    "the engine's find-or-add returns the existing index; a repaint must not charge again",
  );
});

test("showing a panel never leaves a fade class set", () => {
  const { ui, calls } = mount();
  ui.modal({ title: "T", rows: [] }).open(1);
  // Synchronous on purpose: nothing about becoming visible may depend on a later callback. A
  // panel un-hidden at opacity 0 is present, grabs the cursor, and is completely invisible —
  // indistinguishable from a broken addon, and it cost two live debugging rounds.
  const fade = calls.filter((c) => c.op === "cls" && c.cls.endsWith("-out"));
  assert.ok(fade.length > 0, "expected the fade class to be addressed at all");
  for (const c of fade) {
    assert.strictEqual(c.on, false, `${c.cls} must never be set ON while revealing`);
  }
});

test("a toast is visible synchronously too", () => {
  const { ui, calls } = mount();
  ui.toast(1, { title: "t", message: "m", holdSeconds: 0 });
  const shown = calls.find((c) => c.op === "show" && c.id === "s2_t0");
  assert.ok(shown, "toast must be shown");
  const fadeOn = calls.filter((c) => c.op === "cls" && c.cls === "s2-toast-out" && c.on === true);
  assert.strictEqual(fadeOn.length, 0, "a toast must not be revealed transparent");
});

test("a toast schedules its own dismissal through the callback timer API", () => {
  const { ui, pending } = mount();
  ui.toast(1, { title: "t", message: "m", holdSeconds: 5 });
  assert.strictEqual(pending.length, 1, "hold must be scheduled via after(ms, fn), not a Promise-only call");
});

test("cursor() is an absolute index, not a page-relative one", () => {
  const { ui, clickHandlers } = mount();
  const rows = Array.from({ length: 20 }, (_, i) => ({ a: `i${i}` }));
  const picked = [];
  const m = ui.modal({ title: "T", rows: () => rows, onPick: (slot, i) => picked.push(i) });
  m.open(1);
  m.page(1, 1);                 // page 2
  clickHandlers["s2_m0_r3"](1); // 4th row of page 2 -> absolute 11
  assert.strictEqual(picked.at(-1), 11, "onPick reports absolute");
  assert.strictEqual(
    m.cursor(1), 11,
    "cursor() must agree with onPick — a page-relative cursor makes 'act on the selection' " +
    "operate on the wrong row on every page but the first",
  );
});

test("select() takes an absolute index and pages to it", () => {
  const { ui } = mount();
  const rows = Array.from({ length: 20 }, (_, i) => ({ a: `i${i}` }));
  const m = ui.modal({ title: "T", rows: () => rows });
  m.open(1);
  m.select(1, 13);
  assert.strictEqual(m.cursor(1), 13);
});

test("detail() receives the absolute index too", () => {
  const { ui } = mount();
  const rows = Array.from({ length: 20 }, (_, i) => ({ a: `i${i}` }));
  const seen = [];
  const m = ui.modal({
    title: "T", rows: () => rows,
    detail: (slot, row, cursor) => { seen.push({ row: row?.a, cursor }); return ["x"]; },
  });
  m.open(1);
  m.select(1, 11);
  const last = seen.at(-1);
  assert.strictEqual(last.cursor, 11, "cursor must be absolute");
  assert.strictEqual(last.row, "i11", "row and cursor must describe the SAME item");
});

test("forSlot binds modal verbs to one player", () => {
  const { ui } = mount();
  const m = ui.modal({ title: "T", rows: [] });
  const view = m.forSlot(1);
  view.open();
  assert.ok(m.isOpen(1));
  assert.strictEqual(view.isOpen(), true);
  view.close();
  assert.ok(!m.isOpen(1));
});

test("open() returns the bound view", () => {
  const { ui } = mount();
  const m = ui.modal({ title: "T", rows: [] });
  const view = m.open(3);
  assert.strictEqual(view.slot, 3);
  assert.ok(m.isOpen(3));
});

test("a footer onClick receives the bound view so close() does not re-thread the slot", () => {
  const { ui, clickHandlers } = mount();
  const m = ui.modal({
    title: "T", rows: [],
    buttons: [{ text: "X", onClick: (_slot, view) => view.close() }],
  });
  m.open(1);
  clickHandlers["s2_m0_f0"](1);
  assert.ok(!m.isOpen(1));
});

test("forgetting the layout on disconnect closes the modal for that player", () => {
  const { ui } = mount();
  const m = ui.modal({ title: "T", rows: [] });
  m.open(1);
  assert.ok(m.isOpen(1));
  ui.hud.forget(1);
  assert.ok(!m.isOpen(1), "hud.forget (disconnect) must drop per-player modal state");
});

test("kit forSlot.toast drives the same player as toast(slot)", () => {
  const { ui, calls } = mount();
  ui.forSlot(4).toast({ title: "t", message: "m", holdSeconds: 0 });
  assert.ok(calls.some((c) => c.op === "show" && c.slot === 4 && c.id === "s2_t0"));
});

test("open() grabs the cursor unless opts.cursor is false", () => {
  const { ui, calls } = mount();
  const m = ui.modal({ title: "T", rows: [] });
  m.open(1);
  const shown = calls.filter((c) => c.op === "show" && c.id === "s2_m0");
  assert.deepStrictEqual(shown[0].opts, { cursor: true });
  m.close(1);
  m.open(1, { cursor: false });
  const shownOff = calls.filter((c) => c.op === "show" && c.id === "s2_m0");
  assert.deepStrictEqual(shownOff.at(-1).opts, { cursor: false });
});

test("lib descriptor declares the published vote rail ids on s2script_lib.xml", () => {
  const { ui } = mount();
  assert.strictEqual(ui.descriptor.resource, "panorama/layout/custom_game/s2script_lib.xml");
  assert.ok(ui.descriptor.buttons.includes("s2_vote_o0"));
  assert.ok(ui.descriptor.buttons.includes("s2_vote_o8"));
  assert.strictEqual(ui.descriptor.text.s2_vote_q, "s2_vote_q");
  assert.strictEqual(ui.descriptor.text.s2_vote_o3_c, "s2_vote_o3_c");
  assert.ok(!String(ui.descriptor.resource).includes("s2_vote.xml"));
});

test("buttons may be a per-slot function", () => {
  const { ui, clickHandlers } = mount();
  const clicks = [];
  const m = ui.modal({
    title: "T",
    rows: [],
    buttons: (slot) => [{ text: "Go", onClick: () => clicks.push(slot) }],
  });
  m.open(2);
  clickHandlers["s2_m0_f0"](2);
  assert.deepStrictEqual(clicks, [2]);
});
