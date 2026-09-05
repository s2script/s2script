const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

function fakeElement() {
  const classes = new Set();
  return {
    classList: {
      add: (name) => classes.add(name),
      remove: (name) => classes.delete(name),
      contains: (name) => classes.has(name),
    },
  };
}

function mount(options = {}) {
  const elements = {};
  const clickHandlers = {};
  const calls = [];
  const armed = [];
  const disarmed = [];
  const cast = [];
  let renderer;

  const element = (id) => elements[id] || (elements[id] = fakeElement());
  const view = {
    setText(id, value) { calls.push({ op: "text", id, value }); },
    setClass(id, name, on) {
      const classes = element(id).classList;
      if (on) classes.add(name); else classes.remove(name);
      calls.push({ op: "class", id, name, on });
    },
    show(id) {
      element(id).classList.remove("s2-hide");
      calls.push({ op: "show", id });
    },
    hide(id) {
      element(id).classList.add("s2-hide");
      calls.push({ op: "hide", id });
    },
    cursor(on) { calls.push({ op: "cursor", on }); },
  };
  const layout = {
    _cursorForPanel(slot, root, on) { assert.equal(root, "s2_vote"); view.cursor(on); },
    forSlot: () => view,
    onClick: (id, handler) => { clickHandlers[id] = handler; },
  };
  let created = 0;

  // voterail no longer reads hudkit.layout at all: it waits for the plugin's ctx-bound kit via
  // hudkit.whenLive (P0-2). The mount collects the waiter and fires it below, standing in for
  // __s2_make_ctx building the plugin's ui namespace.
  const liveWaiters = [];
  globalThis.__s2pkg_cs2 = {
    CustomHudLayout: {
      create: () => { created++; return layout; },
    },
    hudkit: {
      whenLive: (cb) => { liveWaiters.push(cb); },
      descriptor: {
        addons: ["3790153369"],
        resource: "panorama/layout/custom_game/s2script_lib.xml",
      },
    },
  };
  globalThis.__s2pkg_hudinput = {
    HudInput: {
      arm: (slot, opts) => armed.push({ slot, opts }),
      disarm: (slot) => disarmed.push(slot),
    },
  };
  globalThis.__s2pkg_votes = {
    Vote: { registerTallyRenderer: (value) => { renderer = value; } },
  };
  globalThis.__s2pkg_clients = { Clients: { onDisconnect() {} } };
  globalThis.__s2_vote_cast = (slot, index) => cast.push({ slot, index });
  delete globalThis.__s2pkg_voterail;

  const src = readFileSync(join(__dirname, "voterail.js"), "utf8");
  new Function(src)();
  const goLive = () => { for (const cb of liveWaiters.splice(0)) cb({ layout }); };
  if (!options.deferLive) goLive();
  return {
    elements, clickHandlers, calls, armed, disarmed, cast, created, goLive,
    get renderer() { return renderer; },
  };
}

function tally(choice = null) {
  return {
    question: "Kick Rex?",
    options: [
      { label: "Yes", count: 2 },
      { label: "No", count: 1 },
      { label: "Maybe", count: 0 },
    ],
    total: 3,
    secondsLeft: 12,
    choice,
  };
}

test.afterEach(() => {
  delete globalThis.__s2pkg_cs2;
  delete globalThis.__s2pkg_hudinput;
  delete globalThis.__s2pkg_votes;
  delete globalThis.__s2pkg_clients;
  delete globalThis.__s2_vote_cast;
  delete globalThis.__s2pkg_voterail;
});

test("waiting hides counts, arms Tab once, and routes an option click to cast", () => {
  const mounted = mount();
  mounted.renderer.show(2, tally());
  mounted.renderer.show(2, tally());

  assert.equal(mounted.armed.length, 1);
  assert.equal(mounted.elements.s2_vote_o0_c.classList.contains("s2-vote-count-hide"), true);
  mounted.clickHandlers.s2_vote_o1({ slot: 2 });
  assert.deepEqual(mounted.cast, [{ slot: 2, index: 1 }]);
});

test("Tab activation enables cursor on the rail", () => {
  const mounted = mount();
  mounted.renderer.show(2, tally());
  mounted.armed[0].opts.onActivate();
  assert.deepEqual(mounted.calls.filter((call) => call.op === "cursor"), [{ op: "cursor", on: true }]);
});

test("voted state reveals counts, highlights the choice, disarms, and ignores clicks", () => {
  const mounted = mount();
  mounted.renderer.show(2, tally());
  mounted.renderer.show(2, tally(1));

  assert.equal(mounted.elements.s2_vote_o0_c.classList.contains("s2-vote-count-hide"), false);
  assert.equal(mounted.elements.s2_vote_o1.classList.contains("s2-vote-picked"), true);
  assert.deepEqual(mounted.disarmed, [2]);
  mounted.clickHandlers.s2_vote_o0({ slot: 2 });
  assert.deepEqual(mounted.cast, []);
});

test("hide hides the root and disarms a slot that never voted", () => {
  const mounted = mount();
  mounted.renderer.show(2, tally());
  mounted.renderer.clear(2);

  assert.equal(mounted.elements.s2_vote.classList.contains("s2-hide"), true);
  assert.ok(mounted.disarmed.includes(2));
});

test("unused options receive the hidden class", () => {
  const mounted = mount();
  mounted.renderer.show(2, {
    ...tally(),
    options: [{ label: "Yes", count: 2 }, { label: "No", count: 1 }],
  });

  assert.equal(mounted.elements.s2_vote_o2.classList.contains("s2-hide"), true);
  assert.equal(mounted.elements.s2_vote_o8.classList.contains("s2-hide"), true);
  assert.equal(mounted.created, 0, "must reuse hudkit.layout, not CustomHudLayout.create");
});

test("vote rail does not spawn a second layout resource", () => {
  const mounted = mount();
  assert.ok(mounted.renderer);
  assert.equal(mounted.created, 0);
  assert.equal(globalThis.__s2pkg_cs2.hudkit.descriptor.resource,
    "panorama/layout/custom_game/s2script_lib.xml");
});

test("binds clicks and registers the renderer when the kit goes live, not at eval and not at first paint", () => {
  // Two invariants, both timing. (1) Nothing registers at prelude eval: with no live rail, Vote's
  // own "no tally renderer registered" warning must stay reachable rather than a registered
  // renderer that silently paints nothing. (2) Once live, the nine option clicks bind IMMEDIATELY
  // — whenLive fires inside __s2_make_ctx while the load window is open, which is the only time
  // layout.onClick can still install the click hook through the plugin's ledgered registrar.
  // Binding lazily at the first paint (the old shape) would land after the window seals.
  const mounted = mount({ deferLive: true });
  assert.equal(mounted.renderer, undefined, "no tally renderer before the ctx-bound kit exists");
  assert.deepEqual(Object.keys(mounted.clickHandlers), [], "no clicks bound at eval");

  mounted.goLive();
  assert.ok(mounted.renderer, "renderer registers once the kit is live");
  assert.equal(Object.keys(mounted.clickHandlers).length, 9,
    "all option clicks bind at go-live, before any paint");

  mounted.renderer.show(2, tally());
  assert.equal(mounted.elements.s2_vote_o0_c.classList.contains("s2-vote-count-hide"), true);
  mounted.clickHandlers.s2_vote_o1({ slot: 2 });
  assert.deepEqual(mounted.cast, [{ slot: 2, index: 1 }]);
});
