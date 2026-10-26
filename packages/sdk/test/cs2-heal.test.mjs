/**
 * pawn.heal (games/cs2/js/pawn.js): the cap, overheal, and the cases that must not write. Runs the
 * real shipped bundle in a vm context over a fake schema, like cs2-engine-calls.test.mjs.
 */
import { test } from "node:test";
import assert from "node:assert";
import vm from "node:vm";
import { installClientHost } from "./client-host.mjs";
import { cs2AddonBundle } from "./cs2-addon.mjs";

const OFFSETS = { "CBaseEntity.m_iHealth": 8, "CBaseEntity.m_iMaxHealth": 16 };

function makeHost() {
  function EntityRef(index, id) { this.index = index; this.id = id; this.fields = new Map(); this.writes = []; }
  EntityRef.prototype.readInt32 = function (o) { return this.stale ? null : (this.fields.get(o) ?? 0); };
  EntityRef.prototype.writeInt32 = function (o, v) {
    if (this.stale) return false;
    this.fields.set(o, v);
    this.writes.push([o, v]);
    return true;
  };
  EntityRef.prototype.notifyStateChanged = function () {};
  const ctx = {
    __s2require: (n) => n === "@s2script/sdk/entity" ? { EntityRef }
      : n === "@s2script/sdk/math" ? { Vector: function () {} }
      : n === "@s2script/sdk/events" ? {} : null,
    __s2_schema_offset: (cls, field) => OFFSETS[`${cls}.${field}`] ?? -1,
    __s2pkg_entity: { Entity: { findByClass: () => [] } },
    console: { log: () => {} },
  };
  ctx.globalThis = ctx;
  installClientHost(ctx);
  vm.createContext(ctx);
  vm.runInContext(cs2AddonBundle, ctx);
  const pawn = (health, max) => {
    const ref = new EntityRef(1, 1);
    ref.fields.set(8, health);
    ref.fields.set(16, max);
    return new ctx.__s2pkg_cs2.Pawn(ref);
  };
  return { pawn };
}

test("heal adds health up to maxHealth and returns the new health", () => {
  const { pawn } = makeHost();
  const p = pawn(40, 100);
  assert.equal(p.heal(25.9), 65);
  assert.equal(p.heal(100), 100);
  assert.deepEqual(p.ref.writes, [[8, 65], [8, 100]]);
});

test("opts.max overheals; health above the cap is never lowered", () => {
  const { pawn } = makeHost();
  const p = pawn(90, 100);
  assert.equal(p.heal(50, { max: 150 }), 140);
  assert.equal(p.heal(10), 140, "already above maxHealth: unchanged");
  assert.deepEqual(p.ref.writes, [[8, 140]]);
});

test("a dead or stale pawn is not healed", () => {
  const { pawn } = makeHost();
  const dead = pawn(0, 100);
  assert.equal(dead.heal(10), null);
  assert.deepEqual(dead.ref.writes, []);
  const stale = pawn(50, 100);
  stale.ref.stale = true;
  assert.equal(stale.heal(10), null);
});

test("heal refuses a negative or non-finite amount", () => {
  const { pawn } = makeHost();
  const p = pawn(50, 100);
  for (const bad of [-1, NaN, Infinity, "5"]) assert.throws(() => p.heal(bad), /finite amount/);
  assert.deepEqual(p.ref.writes, []);
});
