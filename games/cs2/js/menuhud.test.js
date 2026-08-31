const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const source = readFileSync(join(__dirname, "menuhud.js"), "utf8");

function mount(options = {}) {
  const calls = [];
  const handlers = {};
  const claim = {
    open: (slot, opts) => calls.push({ method: "open", slot, opts }),
    close: (slot) => calls.push({ method: "close", slot }),
    setCursor: (slot, on) => calls.push({ method: "setCursor", slot, on }),
    setTitle: (slot, value) => calls.push({ method: "setTitle", slot, value }),
    setSubtitle: (slot, value) => calls.push({ method: "setSubtitle", slot, value }),
    setRows: (slot, value) => calls.push({ method: "setRows", slot, value }),
    setPager: (slot, value) => calls.push({ method: "setPager", slot, value }),
    onPick: (slot, fn) => { handlers.pick = { slot, fn }; },
    onAction: (slot, fn) => { handlers.action = { slot, fn }; },
    onBack: (slot, fn) => { handlers.back = { slot, fn }; },
    onClose: (slot, fn) => { handlers.close = { slot, fn }; }
  };
  const hudInput = {
    arms: [],
    disarms: [],
    active: {},
    arm: (slot, opts) => hudInput.arms.push({ slot, opts }),
    disarm: (slot) => hudInput.disarms.push(slot),
    isActive: (slot) => !!hudInput.active[slot]
  };
  const pawn = { moveType: 2 };

  globalThis.__s2pkg_components = { hudkit: { modal: () => options.claimNull ? null : claim } };
  globalThis.__s2pkg_menu = {
    Menu: { registerRenderer: (renderer) => { globalThis.registeredRenderer = renderer; } }
  };
  globalThis.__s2pkg_hudinput = { HudInput: hudInput };
  globalThis.Player = { fromSlot: () => ({ pawn }) };
  delete globalThis.__s2pkg_menuhud;
  delete globalThis.registeredRenderer;
  new Function(source)();

  return {
    calls,
    handlers,
    hudInput,
    pawn,
    renderer: globalThis.registeredRenderer,
    claim
  };
}

function session(options = {}) {
  const selected = [];
  const actions = { prev: 0, next: 0, close: 0 };
  const value = {
    player: { slot: options.slot == null ? 3 : options.slot },
    menu: {
      title: options.title || "Test menu",
      exitButton: options.exitButton !== false,
      freezePlayer: !!options.freezePlayer,
      activation: options.activation || "immediate"
    },
    page: options.page || 0,
    pageCount: options.pageCount || 1,
    hasPrev: !!options.hasPrev,
    hasNext: !!options.hasNext,
    pageItems: () => options.items || [],
    select: (index) => selected.push(index),
    prevPage: () => { actions.prev++; },
    nextPage: () => { actions.next++; },
    close: () => { actions.close++; }
  };
  value.selected = selected;
  value.actions = actions;
  return value;
}

test.afterEach(() => {
  delete globalThis.__s2pkg_components;
  delete globalThis.__s2pkg_menu;
  delete globalThis.__s2pkg_hudinput;
  delete globalThis.Player;
  delete globalThis.__s2pkg_menuhud;
  delete globalThis.registeredRenderer;
});

test("display maps page rows and picks the original item index", () => {
  const mounted = mount();
  const s = session({
    items: [
      { index: 4, display: "First" },
      { index: 8, display: "Second" },
      { index: 12, display: "Third" }
    ]
  });

  mounted.renderer.display(s);

  const rows = mounted.calls.find((call) => call.method === "setRows");
  assert.deepStrictEqual(rows.value, [
    { a: "First", disabled: false },
    { a: "Second", disabled: false },
    { a: "Third", disabled: false }
  ]);
  mounted.handlers.pick.fn(3, 1, rows.value[1]);
  assert.deepStrictEqual(s.selected, [8]);
});

test("disabled rows remain visible but are not selectable", () => {
  const mounted = mount();
  const s = session({
    items: [
      { index: 10, display: "Available" },
      { index: 11, display: "Cooldown", disabled: true },
      { index: 12, display: "Also available" }
    ]
  });

  mounted.renderer.display(s);
  mounted.handlers.pick.fn(3, 1);
  assert.deepStrictEqual(s.selected, []);
  mounted.handlers.pick.fn(3, 2);
  assert.deepStrictEqual(s.selected, [12]);
});

test("pager exposes Menu paging controls and routes Next", () => {
  const mounted = mount();
  const s = session({ hasNext: true, page: 1, pageCount: 3 });

  mounted.renderer.display(s);

  const pager = mounted.calls.find((call) => call.method === "setPager");
  assert.deepStrictEqual(pager.value, {
    page: 2,
    pageCount: 3,
    hasPrev: false,
    hasNext: true
  });
  mounted.handlers.action.fn(3, "next");
  assert.strictEqual(s.actions.next, 1);
});

test("hide closes the modal and restores a frozen pawn", () => {
  const mounted = mount();
  const s = session({ freezePlayer: true });

  mounted.renderer.display(s);
  assert.strictEqual(mounted.pawn.moveType, 0);
  mounted.renderer.hide(s);

  assert.ok(mounted.calls.some((call) => call.method === "close" && call.slot === 3));
  assert.strictEqual(mounted.pawn.moveType, 2);
});

test("tab activation arms HudInput and opens without the cursor", () => {
  const mounted = mount();
  const s = session({ activation: "tab" });

  mounted.renderer.display(s);

  const opened = mounted.calls.find((call) => call.method === "open");
  assert.deepStrictEqual(opened.opts, { cursor: false });
  assert.strictEqual(mounted.hudInput.arms.length, 1);
  mounted.hudInput.arms[0].opts.onActivate();
  assert.ok(mounted.calls.some((call) =>
    call.method === "setCursor" && call.slot === 3 && call.on === true));
  mounted.renderer.hide(s);
  assert.deepStrictEqual(mounted.hudInput.disarms, [3]);
});

test("immediate activation opens with the cursor", () => {
  const mounted = mount();
  const s = session();

  mounted.renderer.display(s);

  const opened = mounted.calls.find((call) => call.method === "open");
  assert.deepStrictEqual(opened.opts, { cursor: true });
  assert.strictEqual(mounted.hudInput.arms.length, 0);
});
