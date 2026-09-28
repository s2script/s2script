// legacy.acquire.v1 conformance: the preflight vectors from
// .superpowers/sdd/2026-09-22-game-package-boundary/task-6-adapter-preflight.md, driven through
// the real adapter source over the recording S2 facade (recording-host.js).
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

const SOURCE = adapterSource("acquire.js");
const Continue = 0, Changed = 1, Handled = 2, Stop = 3;

function setup(contexts = ["a"]) {
  const host = createHost({ name: "canAcquire", returns: "i32", suppression: "generic", scratch: ["result"], pre: true, post: true });
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [SOURCE]);
  const pawns = { maxPlayers: 0, forSlot: () => null };
  const api = name => mounted[name].sandbox.__s2pkg_cs2_adapters.acquire;
  return {
    host,
    pre: (name, handler) => api(name).subscribe("pre", handler, pawns),
    post: (name, handler) => api(name).subscribe("post", handler, pawns),
    api,
  };
}

function fire(host, { engine = 0, method = 7, defIndex = 42, peer, related } = {}) {
  return host.dispatch({
    fields: { method: () => method, item: () => (defIndex === null ? null : Object.freeze({ defIndex })) },
    original: () => engine,
    peer,
    hiddenReferencedBy: related || (() => false),
  });
}

function vote(result, action) {
  return v => { if (result !== undefined) v.result = result; return action; };
}

test("no PRE vote: the original runs and its result stands; POST sees it unskipped", () => {
  const t = setup();
  const seen = [];
  t.pre("a", () => Continue);
  t.post("a", v => { seen.push([v.result, v.skipped]); });
  assert.deepEqual(fire(t.host, { engine: 3 }), { returnValue: 3, skipped: false });
  assert.deepEqual(seen, [[3, false]]);
  assert.ok(t.host.trace.includes("original"));
});

test("Changed deny with engine Allow: original runs and the deny wins after it", () => {
  const t = setup();
  t.pre("a", vote(6, Changed));
  const seen = [];
  t.post("a", v => { seen.push(v.result); });
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 6, skipped: false });
  assert.deepEqual(seen, [6], "POST observers see the effective return");
  assert.deepEqual(t.host.trace.filter(e => e === "original" || e.startsWith("override")), ["original", "override:6"]);
});

test("Changed Allow with engine deny: the engine deny stands with no override", () => {
  const t = setup();
  t.pre("a", vote(0, Changed));
  assert.deepEqual(fire(t.host, { engine: 6 }), { returnValue: 6, skipped: false });
  assert.equal(t.host.trace.some(e => e.startsWith("override")), false, "override only when it changes the result");
});

test("Handled/Stop outrank an earlier Changed deny (action class before registration time)", () => {
  const t = setup(["a", "b"]);
  t.pre("a", vote(6, Changed));
  t.pre("b", vote(2, Handled));
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 2, skipped: true });
  assert.ok(t.host.trace.includes("original-skipped"));
});

test("first deny wins among Handled votes, stable across plugin contexts", () => {
  const t = setup(["a", "b"]);
  t.pre("a", vote(2, Handled));
  t.pre("b", vote(3, Handled));
  assert.deepEqual(fire(t.host), { returnValue: 2, skipped: true });
  const u = setup(["a", "b"]);
  u.pre("a", vote(0, Handled)); // an Allow vote never beats a later deny
  u.pre("b", vote(9, Handled));
  assert.deepEqual(fire(u.host), { returnValue: 9, skipped: true });
});

test("Handled/Stop without a result write is the implicit deny 1", () => {
  const t = setup();
  t.pre("a", () => Handled);
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 1, skipped: true });
  const u = setup(["a", "b"]);
  u.pre("a", vote(5, Continue)); // an earlier handler's write is not THIS handler's write
  u.pre("b", () => Stop);
  assert.deepEqual(fire(u.host, { engine: 0 }), { returnValue: 1, skipped: true });
});

test("Continue after writing result casts no vote; later votes and the engine decide", () => {
  const t = setup(["a", "b"]);
  t.pre("a", vote(5, Continue));
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 0, skipped: false });
  t.pre("b", v => { assert.equal(v.result, 5, "the shared result is visible to later handlers"); return Changed; });
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 5, skipped: false }, "Changed votes the CURRENT result");
});

test("Stop ends delivery; later handlers never run", () => {
  const t = setup(["a", "b"]);
  const ran = [];
  t.pre("a", v => { ran.push("a"); v.result = 4; return Stop; });
  t.pre("b", () => { ran.push("b"); return Handled; });
  assert.deepEqual(fire(t.host), { returnValue: 4, skipped: true });
  assert.deepEqual(ran, ["a"]);
});

test("the engine result participates only if the original ran", () => {
  const t = setup();
  t.pre("a", vote(6, Changed));
  // A higher KHook peer supersedes the original: our proposal must not override its return.
  assert.deepEqual(fire(t.host, { engine: 0, peer: { skip: true, returnValue: 0 } }), { returnValue: 0, skipped: true });
  assert.equal(t.host.trace.some(e => e.startsWith("override")), false);
});

test("PRE suppression plus a POST observer: POST fires skipped with a read-only result", () => {
  const t = setup();
  t.pre("a", vote(8, Handled));
  const seen = [];
  t.post("a", v => { v.result = 0; seen.push([v.result, v.skipped, v.method, v.defIndex]); });
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 8, skipped: true });
  assert.deepEqual(seen, [[8, true, 7, 42]]);
});

test("view: method, defIndex (fallback 0), PRE-only result writes, refused non-i32 writes", () => {
  const t = setup();
  const seen = [];
  t.pre("a", v => {
    seen.push([v.method, v.defIndex, v.result, v.skipped]);
    v.result = "not a number";
    seen.push(v.result);
    v.result = 3.9; // legacy i32 conversion truncates toward zero
    return Changed;
  });
  assert.deepEqual(fire(t.host, { engine: 0, defIndex: null }), { returnValue: 3, skipped: false });
  assert.deepEqual(seen, [[7, 0, 0, false], 0]);
  assert.ok(t.host.logs.some(l => l.includes("result write refused")));
});

test("a throwing handler is Continue and keeps its accepted write for later handlers", () => {
  const t = setup(["a", "b"]);
  t.pre("a", v => { v.result = 6; throw new Error("boom"); });
  t.pre("b", () => Changed);
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 6, skipped: false });
  assert.ok(t.host.logs.some(l => l.includes("onCanAcquire handler threw")));
});

test("player is resolved lazily through the hidden receiver relation; a miss is null and logged once per context", () => {
  const t = setup();
  const refA = { index: 1, id: 11 }, refB = { index: 2, id: 22 };
  const pawns = {
    maxPlayers: 3,
    forSlot: s => (s === 0 ? { ref: refA, controller: "ctl-A" } : s === 1 ? { ref: refB, controller: "ctl-B" } : null),
  };
  const asked = [];
  const players = [];
  t.api("a").subscribe("pre", v => { players.push(v.player); }, pawns);
  const related = (position, ref, cls, field) => { asked.push([position, ref.id, cls, field]); return ref === refB; };
  fire(t.host, { related });
  assert.deepEqual(players, ["ctl-B"]);
  assert.deepEqual(asked, [["itemServices", 11, "CBasePlayerPawn", "m_pItemServices"], ["itemServices", 22, "CBasePlayerPawn", "m_pItemServices"]]);
  asked.length = 0;
  t.api("a").subscribe("pre", () => Continue, pawns);
  fire(t.host, { related: (...args) => { asked.push(args); return false; } });
  assert.equal(asked.length, 2, "only the reader of view.player pays for the hop");
  fire(t.host, { related: () => { throw new Error("expired"); } });
  assert.deepEqual(players.slice(1), [null, null]);
  assert.equal(t.host.logs.filter(l => l.includes("player hop missed")).length, 1);
});

test("nested same-function dispatch skips the busy context and the outer frame stays valid", () => {
  const t = setup();
  const log = [];
  t.pre("a", v => {
    log.push("outer");
    const inner = fire(t.host, { engine: 5 }); // e.g. giveNamedItem from inside the gate
    log.push(`inner:${inner.returnValue}`);
    v.result = 9;
    log.push(`method:${v.method}`);
    return Handled;
  });
  assert.deepEqual(fire(t.host, { engine: 0 }), { returnValue: 9, skipped: true });
  assert.deepEqual(log, ["outer", "inner:5", "method:7"]);
});

test("an unavailable binding is named once and subscribe returns null", () => {
  const host = createHost({ name: "canAcquire", returns: "i32", suppression: "generic", scratch: ["result"], pre: true, post: true, unavailable: true });
  const ctx = host.mount("a", [SOURCE]);
  const api = ctx.sandbox.__s2pkg_cs2_adapters.acquire;
  assert.equal(api.subscribe("pre", () => 0, { maxPlayers: 0, forSlot: () => null }), null);
  assert.equal(api.subscribe("post", () => 0, { maxPlayers: 0, forSlot: () => null }), null);
  assert.equal(host.logs.filter(l => l.includes("pickup gate is unavailable")).length, 1);
  assert.equal(api.status(), "available", "the adapter itself registered");
});
