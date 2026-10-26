// cs2.weapon-can-use.v1, cs2.weapon-can-switch.v1 and cs2.weapon-drop.v1 conformance, driven through
// the real adapter sources over the recording S2 facade (recording-host.js).
const test = require("node:test");
const assert = require("node:assert/strict");
const { createHost, adapterSource } = require("./recording-host.js");

const Continue = 0, Changed = 1, Handled = 2, Stop = 3;
const PAWN = { ref: { index: 3, id: 33 }, controller: { slot: 0 } };
const pawns = { maxPlayers: 4, forSlot: s => (s === 0 ? PAWN : null) };
const weapon = ref => (ref ? { wrapped: ref } : null);
const WEAPON = { index: 7, id: 77 };

const GATES = [
  ["weapon-can-use.js", "weaponCanUse", "weaponCanUse"],
  ["weapon-can-switch.js", "weaponCanSwitch", "weaponCanSwitch"],
];

function setup(file, fn, key, returns, contexts = ["a"], opts = {}) {
  const host = createHost({ name: fn, returns, suppression: "generic", scratch: [], pre: true, post: false, ...opts });
  const source = adapterSource(file);
  const mounted = {};
  for (const name of contexts) mounted[name] = host.mount(name, [source]);
  return {
    host,
    api: name => mounted[name].sandbox.__s2pkg_cs2_adapters[key],
    on: (name, handler) => mounted[name].sandbox.__s2pkg_cs2_adapters[key].subscribe(handler, pawns, weapon),
  };
}

function fire(host, { engine = true, related = true, weaponRef = WEAPON } = {}) {
  const calls = [];
  const result = host.dispatch({
    fields: { weapon: () => weaponRef },
    original: () => engine,
    hiddenReferencedBy: (...args) => { calls.push(args); return related && args[1] === PAWN.ref; },
  });
  return { result, calls };
}

function vote(allowed, action) {
  return v => { if (allowed !== undefined) v.allowed = allowed; return action; };
}

for (const [file, fn, key] of GATES) {
  const gate = () => setup(file, fn, key, "bool");

  test(`${key}: no vote leaves the engine's answer`, () => {
    const t = gate();
    t.on("a", () => Continue);
    t.on("a", vote(false, Continue));
    t.on("a", vote(undefined, Changed));
    assert.deepEqual(fire(t.host, { engine: false }).result, { returnValue: false, skipped: false });
    assert.deepEqual(fire(t.host, { engine: true }).result, { returnValue: true, skipped: false });
  });

  test(`${key}: the view hops to the player and wraps the weapon`, () => {
    const t = gate();
    const seen = [];
    t.on("a", v => { seen.push(v.player, v.weapon, v.allowed); v.allowed = "x"; seen.push(v.allowed); });
    const { calls } = fire(t.host);
    assert.deepEqual(seen, [PAWN.controller, { wrapped: WEAPON }, true, true]);
    assert.deepEqual(calls[0], ["weaponServices", PAWN.ref, "CBasePlayerPawn", "m_pWeaponServices"]);
    assert.equal(t.host.logs.filter(l => l.includes("allowed write refused")).length, 1);
  });

  test(`${key}: a missed hop gives a null player, warned once`, () => {
    const t = gate();
    const players = [];
    t.on("a", v => { players.push(v.player, v.weapon); });
    fire(t.host, { related: false, weaponRef: null });
    fire(t.host, { related: false });
    assert.deepEqual(players, [null, null, null, { wrapped: WEAPON }]);
    assert.equal(t.host.logs.filter(l => l.includes("player hop missed")).length, 1);
  });

  test(`${key}: Handled denies by default and skips the original`, () => {
    const t = gate();
    t.on("a", vote(undefined, Handled));
    assert.deepEqual(fire(t.host).result, { returnValue: false, skipped: true });
  });

  test(`${key}: Changed with a write votes; any false wins across plugins`, () => {
    const t = setup(file, fn, key, "bool", ["a", "b"]);
    t.on("a", vote(true, Changed));
    assert.deepEqual(fire(t.host, { engine: false }).result, { returnValue: true, skipped: true });
    t.on("b", vote(false, Handled));
    assert.deepEqual(fire(t.host, { engine: true }).result, { returnValue: false, skipped: true });
  });

  test(`${key}: Stop ends delivery and a throwing handler is Continue`, () => {
    const t = gate();
    let later = 0;
    t.on("a", v => { v.allowed = false; throw new Error("boom"); });
    t.on("a", vote(true, Stop));
    t.on("a", () => { later++; return Handled; });
    assert.deepEqual(fire(t.host, { engine: false }).result, { returnValue: true, skipped: true });
    assert.equal(later, 0);
  });

  test(`${key}: an unavailable binding names the reason once and returns null`, () => {
    const t = setup(file, fn, key, "bool", ["a"], { unavailable: true });
    assert.equal(t.on("a", () => Continue), null);
    assert.equal(t.on("a", () => Continue), null);
    assert.equal(t.host.logs.filter(l => l.includes("this hook is unavailable")).length, 1);
  });
}

const drop = (contexts) => setup("weapon-drop.js", "weaponDrop", "weaponDrop", "void", contexts);
const fireDrop = host => host.dispatch({
  fields: { weapon: () => WEAPON },
  original: () => undefined,
  hiddenReferencedBy: (...args) => args[1] === PAWN.ref,
});

test("weaponDrop: Continue and Changed let the drop happen", () => {
  const t = drop();
  const seen = [];
  t.on("a", v => { seen.push(v.player, v.weapon); return Continue; });
  t.on("a", () => Changed);
  assert.deepEqual(fireDrop(t.host), { returnValue: undefined, skipped: false });
  assert.deepEqual(seen, [PAWN.controller, { wrapped: WEAPON }]);
  assert.ok(t.host.trace.includes("original"));
});

test("weaponDrop: Handled skips the original, Stop ends delivery", () => {
  const t = drop(["a", "b"]);
  let later = 0;
  t.on("a", () => Handled);
  t.on("b", () => Stop);
  t.on("a", () => { later++; return Continue; });
  assert.deepEqual(fireDrop(t.host), { returnValue: undefined, skipped: true });
  assert.equal(later, 0);
  assert.ok(t.host.trace.includes("original-skipped"));
});

test("weaponDrop: a throwing handler is Continue", () => {
  const t = drop();
  t.on("a", () => { throw new Error("boom"); });
  assert.deepEqual(fireDrop(t.host), { returnValue: undefined, skipped: false });
  assert.equal(t.host.logs.filter(l => l.includes("weapons.onDrop handler threw")).length, 1);
});
