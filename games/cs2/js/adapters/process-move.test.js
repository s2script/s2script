// cs2.process-move.v1 conformance, driven through the real adapter source over the recording S2
// facade (recording-host.js).
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

const PAWN = { ref: { index: 3, id: 33 }, controller: { slot: 0 } };
const pawns = { maxPlayers: 4, forSlot: s => (s === 0 ? PAWN : null) };
const VELOCITY = ["velocityX", "velocityY", "velocityZ"];

function setup(contexts = ["a"], opts = {}) {
  const host = createHost({ name: "processMove", returns: "void", suppression: "none", scratch: [], pre: true, post: true, ...opts });
  const source = adapterSource("process-move.js");
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [source]);
  return {
    host,
    api: name => mounted[name].sandbox.__s2pkg_cs2_adapters.processMove,
    pre: (name, handler) => mounted[name].sandbox.__s2pkg_cs2_adapters.processMove.subscribe("pre", handler, pawns),
    post: (name, handler) => mounted[name].sandbox.__s2pkg_cs2_adapters.processMove.subscribe("post", handler, pawns),
  };
}

function fire(host, { related = true, move = true, original } = {}) {
  const values = { pitch: 10, yaw: 90, roll: 0, forwardMove: 1, sideMove: -1,
    velocityX: 250, velocityY: 0, velocityZ: -10, originX: 1, originY: 2, originZ: 3 };
  const native = {
    records: { move: { present: move, values, writable: VELOCITY,
      storage: Object.fromEntries(Object.keys(values).map(k => [k, "f32"])) } },
    fields: {},
    hiddenReferencedBy: (...args) => related && args[1] === PAWN.ref,
  };
  native.original = () => {
    native.seen = { x: values.velocityX, y: values.velocityY, z: values.velocityZ };
    if (original) original(values);
  };
  host.dispatch(native);
  return native;
}

test("the view reads the move data and hops to the player", () => {
  const t = setup();
  const seen = [];
  t.pre("a", v => { seen.push(v.player, v.viewAngles, v.forwardMove, v.sideMove, v.origin, v.velocity); });
  fire(t.host);
  assert.deepEqual(JSON.parse(JSON.stringify(seen)), [PAWN.controller, { x: 10, y: 90, z: 0 }, 1, -1,
    { x: 1, y: 2, z: 3 }, { x: 250, y: 0, z: -10 }]);
});

test("a PRE velocity write reaches the original; later handlers see it", () => {
  const t = setup();
  const later = [];
  t.pre("a", v => { v.velocity = [0, 0, 300]; });
  t.pre("a", v => { later.push(v.velocity); v.velocity = { x: 5, y: v.velocity.y, z: v.velocity.z }; });
  const native = fire(t.host);
  assert.deepEqual(JSON.parse(JSON.stringify(later)), [{ x: 0, y: 0, z: 300 }]);
  assert.deepEqual(native.seen, { x: 5, y: 0, z: 300 });
});

test("POST sees the moved data and ignores assignment", () => {
  const t = setup();
  const seen = [];
  t.post("a", v => { seen.push(v.velocity, v.origin); v.velocity = [1, 1, 1]; });
  fire(t.host, { original: values => { values.velocityX = 100; values.originZ = 64; } });
  assert.deepEqual(JSON.parse(JSON.stringify(seen)), [{ x: 100, y: 0, z: -10 }, { x: 1, y: 2, z: 64 }]);
  assert.deepEqual(t.host.logs.filter(l => l.includes("WARN")), []);
});

test("a non-finite velocity is refused with a warning and the prior value stands", () => {
  const t = setup();
  t.pre("a", v => { v.velocity = [NaN, 0, 0]; });
  t.pre("a", v => { v.velocity = "fast"; });
  const native = fire(t.host);
  assert.deepEqual(native.seen, { x: 250, y: 0, z: -10 });
  assert.equal(t.host.logs.filter(l => l.includes("velocity write refused")).length, 2);
});

test("a throwing handler keeps its earlier write and later handlers still run", () => {
  const t = setup();
  let ran = false;
  t.pre("a", v => { v.velocity = [1, 2, 3]; throw new Error("boom"); });
  t.pre("a", () => { ran = true; });
  const native = fire(t.host);
  assert.ok(ran);
  assert.deepEqual(native.seen, { x: 1, y: 2, z: 3 });
  assert.equal(t.host.logs.filter(l => l.includes("handler threw")).length, 1);
});

test("a handler's return never suppresses movement", () => {
  const t = setup();
  t.pre("a", () => 2);
  const native = fire(t.host);
  assert.ok(native.seen, "the original ran");
  assert.ok(!t.host.trace.includes("original-skipped"));
});

test("a missed hop gives a null player, warned once; a null record reads zeros", () => {
  const t = setup();
  const seen = [];
  t.pre("a", v => { seen.push(v.player, v.forwardMove); });
  fire(t.host, { related: false });
  fire(t.host, { related: false, move: false });
  assert.deepEqual(seen, [null, 1, null, 0]);
  assert.equal(t.host.logs.filter(l => l.includes("player hop missed")).length, 1);
});

test("the view expires after the handler", () => {
  const t = setup();
  let kept;
  t.pre("a", v => { kept = v; });
  fire(t.host);
  assert.throws(() => kept.velocity, /expired borrowed view/);
});

test("an unavailable binding warns once and returns null", () => {
  const t = setup(["a"], { unavailable: true });
  assert.equal(t.pre("a", () => {}), null);
  assert.equal(t.post("a", () => {}), null);
  assert.equal(t.host.logs.filter(l => l.includes("this hook is unavailable")).length, 1);
});
