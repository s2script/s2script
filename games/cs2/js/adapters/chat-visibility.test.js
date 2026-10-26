// cs2.chat-visibility.v1 conformance, driven through the real adapter source over the recording S2
// facade (recording-host.js).
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

const SOURCE = adapterSource("chat-visibility.js");
const Continue = 0, Changed = 1, Handled = 2, Stop = 3;

function setup(contexts = ["a"]) {
  const host = createHost({ name: "playerCanHearChat", returns: "bool", suppression: "generic", scratch: [], pre: true, post: false });
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [SOURCE]);
  const player = ref => (ref ? { wrapped: ref } : null);
  return {
    host,
    on: (name, handler) => mounted[name].sandbox.__s2pkg_cs2_adapters.chatVisibility.subscribe(handler, player),
  };
}

const RECEIVER = { index: 1, id: 11 }, SENDER = { index: 2, id: 22 };
function fire(host, { engine = true, teamOnly = false, receiver = RECEIVER, sender = SENDER } = {}) {
  return host.dispatch({
    fields: { receiver: () => receiver, sender: () => sender, teamOnly: () => teamOnly },
    original: () => engine,
  });
}
function vote(canHear, action) {
  return v => { if (canHear !== undefined) v.canHear = canHear; return action; };
}

test("no vote: the engine decides", () => {
  const t = setup();
  t.on("a", () => Continue);
  t.on("a", vote(false, Continue));
  assert.deepEqual(fire(t.host, { engine: false }), { returnValue: false, skipped: false });
  assert.deepEqual(fire(t.host, { engine: true }), { returnValue: true, skipped: false });
});

test("the view exposes wrapped players, teamOnly and the handler's own write", () => {
  const t = setup();
  const seen = [];
  t.on("a", v => {
    seen.push([v.receiver.wrapped.id, v.sender.wrapped.id, v.teamOnly, v.canHear]);
    v.canHear = false;
    seen.push(v.canHear);
    v.canHear = "no";
    seen.push(v.canHear);
  });
  fire(t.host, { teamOnly: true });
  assert.deepEqual(seen, [[11, 22, true, true], false, false]);
  const nulls = [];
  t.on("a", v => { nulls.push(v.receiver, v.sender); });
  fire(t.host, { receiver: null, sender: null });
  assert.deepEqual(nulls.slice(-2), [null, null]);
});

test("Handled true overrides an engine false and skips the original", () => {
  const t = setup();
  t.on("a", vote(true, Handled));
  assert.deepEqual(fire(t.host, { engine: false }), { returnValue: true, skipped: true });
  assert.ok(t.host.trace.includes("original-skipped"));
});

test("Handled with no write votes false", () => {
  const t = setup();
  t.on("a", vote(undefined, Handled));
  assert.deepEqual(fire(t.host), { returnValue: false, skipped: true });
});

test("Changed votes only when it wrote canHear", () => {
  const t = setup();
  t.on("a", vote(undefined, Changed));
  assert.deepEqual(fire(t.host, { engine: false }), { returnValue: false, skipped: false });
  const u = setup();
  u.on("a", vote(true, Changed));
  assert.deepEqual(fire(u.host, { engine: false }), { returnValue: true, skipped: true });
});

test("any false vote wins over true votes, across plugins", () => {
  const t = setup(["a", "b"]);
  t.on("a", vote(true, Handled));
  t.on("b", vote(false, Changed));
  t.on("a", vote(true, Handled));
  assert.deepEqual(fire(t.host), { returnValue: false, skipped: true });
});

test("Stop ends delivery: a later deny never runs", () => {
  const t = setup();
  let later = 0;
  t.on("a", vote(true, Stop));
  t.on("a", () => { later++; return Handled; });
  assert.deepEqual(fire(t.host, { engine: false }), { returnValue: true, skipped: true });
  assert.equal(later, 0);
});

test("a throwing handler is Continue and its write is dropped", () => {
  const t = setup();
  t.on("a", v => { v.canHear = true; throw new Error("boom"); });
  assert.deepEqual(fire(t.host, { engine: false }), { returnValue: false, skipped: false });
});

test("an unavailable binding names the reason once and returns null", () => {
  const host = createHost({ name: "playerCanHearChat", returns: "bool", suppression: "generic", scratch: [], pre: true, post: false, unavailable: true });
  const ctx = host.mount("a", [SOURCE]);
  const api = ctx.sandbox.__s2pkg_cs2_adapters.chatVisibility;
  assert.equal(api.subscribe(() => Continue, r => r), null);
  assert.equal(api.subscribe(() => Continue, r => r), null);
  assert.equal(host.logs.filter(l => l.includes("chat visibility is unavailable")).length, 1);
});
