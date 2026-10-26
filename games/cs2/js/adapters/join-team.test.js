// cs2.join-team.v1 conformance, driven through the real adapter source over the recording S2
// facade (recording-host.js).
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

const Continue = 0, Changed = 1, Handled = 2, Stop = 3;
const CONTROLLER = { index: 1, id: 11 };
const player = ref => (ref ? { wrapped: ref } : null);

function setup(contexts = ["a"], opts = {}) {
  const host = createHost({ name: "joinTeam", returns: "bool", suppression: "generic", scratch: [], pre: true, post: false, ...opts });
  const source = adapterSource("join-team.js");
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [source]);
  return {
    host,
    on: (name, handler) => mounted[name].sandbox.__s2pkg_cs2_adapters.joinTeam.subscribe(handler, player),
  };
}

function fire(host, { team = 3, queue = false, ref = CONTROLLER, engine = true } = {}) {
  return host.dispatch({
    fields: { player: () => ref, team: () => team, queue: () => queue },
    original: () => engine,
  });
}

test("the view carries the player, team and queue; Continue and Changed let it through", () => {
  const t = setup();
  const seen = [];
  t.on("a", v => { seen.push(v.player, v.team, v.queue); return Continue; });
  t.on("a", () => Changed);
  assert.deepEqual(fire(t.host, { team: 2, queue: true }), { returnValue: true, skipped: false });
  assert.deepEqual(seen, [{ wrapped: CONTROLLER }, 2, true]);
  assert.ok(t.host.trace.includes("original"));
});

test("Handled refuses with false across plugins; Stop ends delivery", () => {
  const t = setup(["a", "b"]);
  let later = 0;
  t.on("a", () => Handled);
  t.on("b", () => Stop);
  t.on("a", () => { later++; return Continue; });
  assert.deepEqual(fire(t.host), { returnValue: false, skipped: true });
  assert.equal(later, 0);
});

test("a throwing handler is Continue; a null controller gives a null player", () => {
  const t = setup();
  const players = [];
  t.on("a", v => { players.push(v.player); throw new Error("boom"); });
  assert.deepEqual(fire(t.host, { ref: null }), { returnValue: true, skipped: false });
  assert.deepEqual(players, [null]);
  assert.equal(t.host.logs.filter(l => l.includes("Teams.onJoinTeam handler threw")).length, 1);
});

test("an unavailable binding warns once and returns null", () => {
  const t = setup(["a"], { unavailable: true });
  assert.equal(t.on("a", () => Continue), null);
  assert.equal(t.on("a", () => Continue), null);
  assert.equal(t.host.logs.filter(l => l.includes("this hook is unavailable")).length, 1);
});
