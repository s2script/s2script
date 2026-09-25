// legacy.damage.v1 conformance: the historical SDKHook OnTakeDamage/OnTakeDamagePost behaviour,
// driven through the real adapter source over the recording S2 facade (recording-host.js), plus
// Weapon.setAmmo over the generated clip1 accessor.
const test = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const { createHost, adapterSource } = require("./recording-host.js");

const SOURCE = adapterSource("damage.js");
const Continue = 0, Changed = 1, Handled = 2, Stop = 3;

// Entities are plain {index, id}; `dead` is the set of host ids the books no longer hold.
function setup(contexts = ["a"], options = {}) {
  const host = createHost({ name: "takeDamageOld", returns: "void", suppression: "generic", scratch: [], pre: true, post: true,
    unavailable: options.unavailable });
  const dead = new Set();
  const mounted = {};
  for (const name of contexts) {
    const providers = {};
    const ctx = host.mount(name, [SOURCE], {
      __s2_sdkhook_provider_register(type, provider) {
        if (providers[type]) throw new Error("already provided");
        providers[type] = provider;
      },
      __s2_ent_ref_valid: (index, id) => !dead.has(id),
    });
    ctx.providers = providers;
    mounted[name] = ctx;
  }
  const hook = (name, type, entity, cb) => mounted[name].providers[type].hook(entity.index, entity.id, cb);
  const unhook = (name, type, entity, cb) => mounted[name].providers[type].unhook(entity.index, entity.id, cb);
  return {
    host, mounted, dead,
    pre: (name, entity, cb) => hook(name, "OnTakeDamage", entity, cb),
    post: (name, entity, cb) => hook(name, "OnTakeDamagePost", entity, cb),
    unhook,
  };
}

const VICTIM = { index: 5, id: 105 };
const OTHER = { index: 6, id: 106 };
const ATTACKER = { index: 1, id: 101 };
const INFLICTOR = { index: 9, id: 109 };

// One native TakeDamageOld call. `result` is the opaque optional result storage (null or an object).
function mountDamage(t, { victim = VICTIM, damage = 50, damageType = 2, attacker = ATTACKER, inflictor = INFLICTOR,
  info = true, result = null } = {}) {
  const native = {
    records: { info: { present: info, values: { attacker, inflictor, damage, damageType },
      writable: ["damage"], storage: { damage: "f32" } } },
    fields: { victim: () => victim },
    result,
    seenByOriginal: undefined,
  };
  native.original = () => {
    native.seenByOriginal = { damage: native.records.info.values.damage, result: native.result };
    t.host.trace.push(`original:${native.records.info.values.damage}`);
  };
  return {
    native,
    fire: () => t.host.dispatch(native),
    nativeDamage: () => native.records.info.values.damage,
  };
}

test("DamageInfo keeps CS2 names but expires after the synchronous callback", () => {
  const t = setup();
  let saved;
  const seen = [];
  assert.equal(t.pre("a", VICTIM, v => {
    saved = v;
    seen.push([v.damage, v.damageType, v.attacker, v.inflictor, v.victim]);
    v.damage /= 2;
  }), true);
  const damage = mountDamage(t, { damage: 50 });
  damage.fire();
  assert.deepEqual(seen, [[50, 2, ATTACKER, INFLICTOR, VICTIM]]);
  assert.equal(damage.nativeDamage(), 25);
  assert.deepEqual(Object.keys(saved), ["damage", "damageType", "attacker", "inflictor", "victim"]);
  for (const key of ["damage", "damageType", "attacker", "inflictor", "victim"])
    assert.throws(() => saved[key], /expired borrowed view/, key);
  assert.throws(() => { saved.damage = 1; }, /expired borrowed view/);
});

test("PRE writes commit before the original; the original always runs exactly once", () => {
  const t = setup();
  t.pre("a", VICTIM, v => { v.damage = 7; });
  const damage = mountDamage(t);
  damage.fire();
  assert.deepEqual(t.host.trace.filter(e => e.startsWith("commit") || e.startsWith("original")),
    ["commit:info.damage=7", "original", "original:7"]);
});

test("POST is readonly: sees the committed damage, assignment is ignored without poisoning, return ignored", () => {
  const t = setup();
  const seen = [];
  t.pre("a", VICTIM, v => { v.damage = 30; });
  t.post("a", VICTIM, v => { seen.push(v.damage); v.damage = 99; seen.push(v.damage); return Handled; });
  const damage = mountDamage(t);
  damage.fire();
  assert.deepEqual(seen, [30, 30]);
  assert.equal(damage.nativeDamage(), 30);
  assert.equal(t.host.logs.some(l => l.includes("subscriber decision")), false, "POST never returns a decision or poisons");
});

test("Handled blocks by zeroing after the fan-out: later handlers still run and the original still runs", () => {
  const t = setup(["a", "b"]);
  const ran = [];
  t.pre("a", VICTIM, v => { ran.push(`a:${v.damage}`); return Handled; });
  t.pre("b", VICTIM, v => { ran.push(`b:${v.damage}`); v.damage = 80; return Continue; });
  const damage = mountDamage(t, { damage: 40 });
  damage.fire();
  assert.deepEqual(ran, ["a:40", "b:40"]);
  assert.equal(damage.nativeDamage(), 0);
  assert.deepEqual(damage.native.seenByOriginal, { damage: 0, result: null }, "block-to-zero, never a skipped original");
  assert.equal(t.host.trace.includes("original-skipped"), false);
});

test("Stop ends delivery and blocks; Changed alone does not block", () => {
  const t = setup(["a", "b"]);
  const ran = [];
  t.pre("a", VICTIM, () => { ran.push("a"); return Stop; });
  t.pre("b", VICTIM, () => { ran.push("b"); return Handled; });
  const d1 = mountDamage(t, { damage: 40 });
  d1.fire();
  assert.deepEqual(ran, ["a"]);
  assert.equal(d1.nativeDamage(), 0);
  const u = setup();
  u.pre("a", VICTIM, v => { v.damage = 12; return Changed; });
  const d2 = mountDamage(u, { damage: 40 });
  d2.fire();
  assert.equal(d2.nativeDamage(), 12);
  const w = setup();
  w.pre("a", VICTIM, () => 7); // out-of-range numbers are Continue, never Stop
  w.pre("a", VICTIM, () => "3");
  const d3 = mountDamage(w, { damage: 40 });
  d3.fire();
  assert.equal(d3.nativeDamage(), 40);
});

test("per-entity filtering and exact subscription order across plugin contexts", () => {
  const t = setup(["a", "b"]);
  const ran = [];
  t.pre("a", VICTIM, () => { ran.push("a1"); });
  t.pre("b", OTHER, () => { ran.push("b-other"); });
  t.pre("b", VICTIM, () => { ran.push("b1"); });
  t.pre("a", VICTIM, () => { ran.push("a2"); });
  mountDamage(t).fire();
  assert.deepEqual(ran, ["a1", "b1", "a2"]);
  ran.length = 0;
  mountDamage(t, { victim: OTHER }).fire();
  assert.deepEqual(ran, ["b-other"]);
  ran.length = 0;
  mountDamage(t, { victim: null }).fire();
  assert.deepEqual(ran, [], "an unadoptable victim runs nobody");
});

test("later handlers see earlier accepted writes; a throwing handler is Continue and keeps its write", () => {
  const t = setup(["a", "b"]);
  const seen = [];
  t.pre("a", VICTIM, v => { v.damage = 60; throw new Error("boom"); });
  t.pre("b", VICTIM, v => { seen.push(v.damage); });
  const damage = mountDamage(t, { damage: 10 });
  damage.fire();
  assert.deepEqual(seen, [60]);
  assert.equal(damage.nativeDamage(), 60);
  assert.ok(t.host.logs.some(l => l.includes("OnTakeDamage handler threw")));
});

test("a non-finite damage write is refused without discarding the handler's other writes", () => {
  const t = setup(["a", "b"]);
  const seen = [];
  t.pre("a", VICTIM, v => { v.damage = 9; v.damage = NaN; v.damage = Infinity; v.damage = 1e39; seen.push(v.damage); });
  t.pre("b", VICTIM, v => { seen.push(v.damage); });
  const damage = mountDamage(t, { damage: 40 });
  damage.fire();
  assert.deepEqual(seen, [9, 9]);
  assert.equal(damage.nativeDamage(), 9);
  assert.equal(t.host.logs.filter(l => l.includes("damage write refused")).length, 3);
});

test("attacker/inflictor/victim are adopted entity handles; absent handles and a null info degrade", () => {
  const t = setup();
  const seen = [];
  t.pre("a", VICTIM, v => { seen.push([v.attacker, v.inflictor, v.victim, v.damage, v.damageType]); });
  mountDamage(t, { attacker: null, inflictor: null }).fire();
  mountDamage(t, { info: false }).fire();
  assert.deepEqual(seen, [[null, null, VICTIM, 50, 2], [null, null, VICTIM, 0, 0]]);
});

test("a view retained across await is expired", async () => {
  const t = setup();
  let later;
  t.pre("a", VICTIM, async v => {
    await null;
    try { later = v.damage; } catch (e) { later = e.message; }
  });
  mountDamage(t).fire();
  await new Promise(r => { Promise.resolve().then(() => Promise.resolve()).then(r); });
  assert.match(later, /expired borrowed view/);
});

test("the optional result storage is never exposed and reaches the original unchanged (null and non-null)", () => {
  const t = setup();
  const keys = [];
  t.pre("a", VICTIM, v => { keys.push(Object.keys(v).join(",")); v.damage = 5; });
  const storage = { opaque: true };
  for (const result of [null, storage]) {
    const damage = mountDamage(t, { result });
    damage.fire();
    assert.equal(damage.native.seenByOriginal.result, result);
  }
  assert.deepEqual(keys, ["damage,damageType,attacker,inflictor,victim", "damage,damageType,attacker,inflictor,victim"]);
});

test("nested damage: the busy context is skipped, others are delivered, the outer view stays valid", () => {
  const t = setup(["a", "b"]);
  const log = [];
  let inner;
  t.pre("a", VICTIM, v => {
    log.push(`a-outer:${v.damage}`);
    inner = mountDamage(t, { damage: 3 });
    inner.fire(); // e.g. a plugin dealing damage from inside its damage hook
    v.damage = 11;
    log.push(`a-after:${v.damage}`);
  });
  t.pre("b", VICTIM, v => { log.push(`b:${v.damage}`); });
  const outer = mountDamage(t, { damage: 20 });
  outer.fire();
  assert.deepEqual(log, ["a-outer:20", "b:3", "a-after:11", "b:11"]);
  assert.equal(inner.nativeDamage(), 3);
  assert.equal(outer.nativeDamage(), 11);
});

test("SDKUnhook removes the first matching registration; stale entities are pruned on the next hook", () => {
  const t = setup();
  const ran = [];
  const cb = () => { ran.push("cb"); };
  t.pre("a", VICTIM, cb);
  t.pre("a", VICTIM, cb);
  assert.equal(t.unhook("a", "OnTakeDamage", VICTIM, cb), true);
  mountDamage(t).fire();
  assert.deepEqual(ran, ["cb"]);
  assert.equal(t.unhook("a", "OnTakeDamagePost", VICTIM, cb), false, "type is part of the identity");
  assert.equal(t.unhook("a", "OnTakeDamage", VICTIM, cb), true);
  assert.equal(t.unhook("a", "OnTakeDamage", VICTIM, cb), false);
  const api = t.mounted.a.sandbox.__s2pkg_cs2_adapters.damage;
  t.pre("a", OTHER, cb);
  t.dead.add(OTHER.id);
  t.pre("a", VICTIM, cb);
  assert.equal(api.count(), 1, "the destroyed entity's registration was disposed");
  assert.equal(t.host.subscriptions.filter(s => !s.disposed).length, 1);
});

test("the adapter registers both SDKHook damage providers; an unavailable binding refuses once per context", () => {
  const t = setup(["a"], { unavailable: true });
  assert.deepEqual(Object.keys(t.mounted.a.providers).sort(), ["OnTakeDamage", "OnTakeDamagePost"]);
  assert.equal(t.pre("a", VICTIM, () => {}), false);
  assert.equal(t.post("a", VICTIM, () => {}), false);
  assert.equal(t.host.logs.filter(l => l.includes("damage hooks are unavailable")).length, 1);
});

// --- Weapon.setAmmo: generated schema access for clip1, numeric and stale guards, no ammo native.
function fixtureWeapon() {
  const writes = [];
  const ref = {
    live: true,
    isValid() { return this.live; },
    invalidate() { this.live = false; },
  };
  const schema = {
    applyAccessors(proto, cls) {
      assert.equal(cls, "CCSWeaponBase");
      Object.defineProperty(proto, "clip1", {
        get() { return writes.length ? writes[writes.length - 1] : 0; },
        set(v) { if (this.ref.isValid()) writes.push(v); },
        configurable: true,
      });
    },
  };
  const sandbox = { __s2pkg_cs2_schema: schema, __s2pkg_cs2: {} };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  const file = join(__dirname, "..", "weapon.js");
  vm.runInContext(readFileSync(file, "utf8"), sandbox, { filename: file });
  const weapon = new sandbox.__s2pkg_cs2.Weapon(ref);
  weapon.writes = writes;
  return weapon;
}

test("setAmmo uses the generated clip1 accessor, ignores reserve, and stale/non-numeric calls fail", () => {
  const weapon = fixtureWeapon();
  assert.equal(weapon.setAmmo(30), true);
  assert.equal(weapon.setAmmo(12, 90), true, "reserve is accepted but deferred");
  assert.deepEqual(weapon.writes, [30, 12]);
  assert.equal(weapon.setAmmo("30"), false);
  assert.equal(weapon.setAmmo(NaN), false);
  assert.equal(weapon.setAmmo(Infinity), false);
  weapon.ref.invalidate();
  assert.equal(weapon.setAmmo(10), false);
  assert.deepEqual(weapon.writes, [30, 12], "no write on a refused call");
  assert.equal(/__s2_ammo|ammo_/.test(readFileSync(join(__dirname, "..", "weapon.js"), "utf8")), false, "no ammo native");
});
