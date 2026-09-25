// legacy.hud-click.v1 conformance over the recording S2 facade: deliveries run inside the native
// PRE and before the original, the adapter never suppresses, handlers see the host's copied
// button id, and a nested click is an ordinary synchronous nested dispatch.
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

const SOURCE = adapterSource("hud-click.js");

function setup(contexts = ["a"]) {
  const host = createHost({ name: "customHudClicked", returns: "void", suppression: "none", scratch: [], pre: true, post: false });
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [SOURCE]);
  return { host, api: name => mounted[name].sandbox.__s2pkg_cs2_adapters.hudClick };
}

// The host copies the engine string before JavaScript runs; `native.text` models the engine
// object, which is mutated afterwards to show the copy is what handlers read.
function click(host, player, native) {
  const copied = String(native.text);
  return host.dispatch({
    fields: { player: () => player, buttonId: () => copied },
    original: () => { host.trace.push(`engine-sees:${native.text}`); return undefined; },
  });
}

test("handlers run in native PRE before the original and the final action is Continue", () => {
  const t = setup(["a", "b"]);
  t.api("a").subscribe(c => { t.host.trace.push(`a:${c.buttonId}:${c.player}`); return 2; });
  t.api("b").subscribe(c => { t.host.trace.push(`b:${c.buttonId}`); });
  const result = click(t.host, "ctl-3", { text: "Dismiss" });
  assert.equal(result.skipped, false, "a handler returning Handled cannot suppress map click handling");
  assert.deepEqual(t.host.trace, ["native-pre:customHudClicked", "a:Dismiss:ctl-3", "b:Dismiss", "original", "engine-sees:Dismiss", "native-return:undefined"]);
});

test("handlers read the host copy of the button id", () => {
  const t = setup();
  const native = { text: "Accept" };
  const seen = [];
  t.api("a").subscribe(c => { native.text = "mutated"; seen.push(c.buttonId); });
  click(t.host, "ctl", native);
  assert.deepEqual(seen, ["Accept"]);
});

test("an unresolved clicker is delivered as null; a throwing handler is named and Continue", () => {
  const t = setup(["a", "b"]);
  const seen = [];
  t.api("a").subscribe(c => { seen.push(c.player); throw new Error("boom"); });
  t.api("b").subscribe(c => { seen.push(`b:${c.buttonId}`); });
  assert.equal(click(t.host, null, { text: "x" }).skipped, false);
  assert.deepEqual(seen, [null, "b:x"]);
  assert.ok(t.host.logs.some(l => l.includes("onCustomHudClicked handler threw")));
});

test("a synchronous nested click is its own dispatch; the busy context is skipped inside it", () => {
  const t = setup(["a", "b"]);
  const seen = [];
  t.api("a").subscribe(c => {
    seen.push(`a:${c.buttonId}`);
    if (c.buttonId === "outer") click(t.host, "ctl", { text: "inner" });
    seen.push(`a-after:${c.buttonId}`);
  });
  t.api("b").subscribe(c => { seen.push(`b:${c.buttonId}`); });
  click(t.host, "ctl", { text: "outer" });
  assert.deepEqual(seen, ["a:outer", "b:inner", "a-after:outer", "b:outer"]);
});

test("a subscription registered during a click does not receive that click", () => {
  const t = setup();
  const seen = [];
  t.api("a").subscribe(() => {
    seen.push("first");
    if (seen.length === 1) t.api("a").subscribe(() => { seen.push("late"); });
  });
  click(t.host, "ctl", { text: "x" });
  assert.deepEqual(seen, ["first"]);
  click(t.host, "ctl", { text: "y" });
  assert.deepEqual(seen, ["first", "first", "late"]);
});

test("an unavailable binding is named once and subscribe returns null", () => {
  const host = createHost({ name: "customHudClicked", returns: "void", suppression: "none", scratch: [], pre: true, unavailable: true });
  const ctx = host.mount("a", [SOURCE]);
  const api = ctx.sandbox.__s2pkg_cs2_adapters.hudClick;
  assert.equal(api.subscribe(() => {}), null);
  assert.equal(api.subscribe(() => {}), null);
  assert.equal(host.logs.filter(l => l.includes("click routing is unavailable")).length, 1);
});
