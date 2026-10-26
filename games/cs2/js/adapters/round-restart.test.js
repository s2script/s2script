// cs2.round-restart.v1 conformance, driven through the real adapter source over the recording S2
// facade (recording-host.js).
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

function setup(contexts = ["a"], opts = {}) {
  const host = createHost({ name: "roundRestart", returns: "void", suppression: "none", scratch: [], pre: true, post: true, ...opts });
  const source = adapterSource("round-restart.js");
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [source]);
  const api = name => mounted[name].sandbox.__s2pkg_cs2_adapters.roundRestart;
  return {
    host,
    pre: (name, handler) => api(name).subscribe("pre", handler),
    post: (name, handler) => api(name).subscribe("post", handler),
  };
}

function fire(host, order) {
  host.dispatch({ fields: {}, original: () => { order.push("original"); } });
}

test("PRE runs before the round resets and POST after, with no arguments", () => {
  const t = setup(["a", "b"]);
  const order = [];
  t.pre("a", (...args) => { order.push("a-pre:" + args.length); });
  t.pre("b", () => { order.push("b-pre"); });
  t.post("a", (...args) => { order.push("a-post:" + args.length); });
  fire(t.host, order);
  assert.deepEqual(order, ["a-pre:0", "b-pre", "original", "a-post:0"]);
});

test("a handler's return never stops the restart", () => {
  const t = setup();
  const order = [];
  t.pre("a", () => 3);
  fire(t.host, order);
  assert.deepEqual(order, ["original"]);
  assert.ok(!t.host.trace.includes("original-skipped"));
});

test("a throwing handler is logged and later handlers still run", () => {
  const t = setup();
  const order = [];
  t.pre("a", () => { throw new Error("boom"); });
  t.pre("a", () => { order.push("next"); });
  t.post("a", () => { throw new Error("late"); });
  fire(t.host, order);
  assert.deepEqual(order, ["next", "original"]);
  assert.equal(t.host.logs.filter(l => l.includes("handler threw")).length, 2);
});

test("an unavailable binding warns once and returns null", () => {
  const t = setup(["a"], { unavailable: true });
  assert.equal(t.pre("a", () => {}), null);
  assert.equal(t.post("a", () => {}), null);
  assert.equal(t.host.logs.filter(l => l.includes("this hook is unavailable")).length, 1);
});
