/**
 * Particles.create (games/cs2/js/pawn.js): the keyvalues it spawns info_particle_system with, how
 * the viewer list becomes a transmit rule, and the inputs the handle sends. Runs the real shipped
 * bundle in a vm context over recording fakes, like cs2-engine-calls.test.mjs.
 */
import { test } from "node:test";
import assert from "node:assert";
import vm from "node:vm";
import { installClientHost } from "./client-host.mjs";
import { cs2AddonBundle } from "./cs2-addon.mjs";

function makeHost({ createFails = false, spawnFails = false } = {}) {
  const created = [];
  const transmit = [];
  const timers = [];
  function EntityRef(index, id) { this.index = index; this.id = id; this.inputs = []; }
  EntityRef.prototype.acceptInput = function (input, value, activator) {
    this.inputs.push([input, value, activator]);
    return true;
  };
  EntityRef.prototype.remove = function () { this.removed = true; return true; };
  EntityRef.prototype.teleport = function (origin, angles) { this.placed = [origin, angles]; return true; };
  // Records the keyvalues and whether the entity was already placed when it spawned.
  EntityRef.prototype.spawn = function (kv) {
    created.find((c) => c.ref === this).kv = kv;
    this.placedAtSpawn = !!this.placed;
    return !spawnFails;
  };
  const ctx = {
    __s2require: (n) => n === "@s2script/sdk/entity" ? { EntityRef }
      : n === "@s2script/sdk/math" ? { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; } }
      : n === "@s2script/sdk/events" ? {} : null,
    __s2_schema_offset: () => -1,
    __s2pkg_entity: {
      Entity: { findByClass: () => [] },
      createEntity: (cls, kv) => {
        if (createFails) return null;
        assert.equal(kv, undefined, "a keyvalue create would spawn before the teleport");
        const ref = new EntityRef(100 + created.length, 1);
        created.push({ cls, kv: null, ref });
        return ref;
      },
    },
    __s2pkg_transmit: { Transmit: { setVisibleTo: (ref, slots) => { transmit.push([ref.index, slots]); return true; } } },
    __s2pkg_timers: { delay: (ms) => new Promise((res) => timers.push([ms, res])) },
    console: { log: () => {} },
  };
  ctx.globalThis = ctx;
  installClientHost(ctx);
  vm.createContext(ctx);
  vm.runInContext(cs2AddonBundle, ctx);
  return { Particles: ctx.__s2pkg_cs2.Particles, created, transmit, timers, EntityRef };
}

test("create spawns an active info_particle_system and teleports it to the origin", () => {
  const h = makeHost();
  const p = h.Particles.create("particles/x.vpcf", { x: 1, y: 2.5, z: -3 });
  assert.equal(h.created.length, 1);
  assert.equal(h.created[0].cls, "info_particle_system");
  assert.deepEqual({ ...h.created[0].kv }, { effect_name: "particles/x.vpcf", start_active: 1 });
  assert.deepEqual(JSON.parse(JSON.stringify(p.ref.placed)), [[1, 2.5, -3], null]);
  assert.ok(p.ref.placedAtSpawn, "placed before it spawns, so the effect starts at the origin");
  assert.equal(h.transmit.length, 0, "no visibleTo -> no transmit rule");
  assert.equal(p.ref, h.created[0].ref);
});

test("angles go to the teleport; start:false and extra keyvalues reach the spawn", () => {
  const h = makeHost();
  const p = h.Particles.create("e.vpcf", [0, 0, 0], { angles: [10, 20, 30], start: false, keyvalues: { cpoint1: "a" } });
  assert.deepEqual({ ...h.created[0].kv }, { effect_name: "e.vpcf", start_active: 0, cpoint1: "a" });
  assert.deepEqual(JSON.parse(JSON.stringify(p.ref.placed)), [[0, 0, 0], [10, 20, 30]]);
});

test("visibleTo takes slots or players, drops out-of-range and duplicates", () => {
  const h = makeHost();
  const p = h.Particles.create("e.vpcf", [0, 0, 0], { visibleTo: [3, { slot: 5 }, 3, 64, -1] });
  assert.deepEqual(h.transmit.map(([i, s]) => [i, Array.from(s)]), [[100, [3, 5]]]);
  p.setVisibleTo([]);
  assert.deepEqual(Array.from(h.transmit[1][1]), [], "an empty list hides it from everyone");
});

test("parent sends SetParent with the parent as activator; start/stop are inputs", () => {
  const h = makeHost();
  const parent = new h.EntityRef(7, 1);
  const p = h.Particles.create("e.vpcf", [0, 0, 0], { parent });
  p.stop(); p.start();
  assert.deepEqual(p.ref.inputs.map((x) => x[0]), ["SetParent", "Stop", "Start"]);
  assert.equal(p.ref.inputs[0][1], "!activator");
  assert.equal(p.ref.inputs[0][2], parent);
});

test("lifetime removes the entity after that many seconds", async () => {
  const h = makeHost();
  const p = h.Particles.create("e.vpcf", [0, 0, 0], { lifetime: 2.5 });
  assert.equal(h.timers.length, 1);
  assert.equal(h.timers[0][0], 2500);
  assert.ok(!p.ref.removed);
  h.timers[0][1]();
  await new Promise((r) => setImmediate(r));
  assert.ok(p.ref.removed);
});

test("a failed create returns null; bad arguments throw", () => {
  assert.equal(makeHost({ createFails: true }).Particles.create("e.vpcf", [0, 0, 0]), null);
  const failed = makeHost({ spawnFails: true });
  assert.equal(failed.Particles.create("e.vpcf", [0, 0, 0]), null);
  assert.ok(failed.created[0].ref.removed, "a failed spawn removes the created entity");
  const h = makeHost();
  assert.throws(() => h.Particles.create("", [0, 0, 0]), /effect must be/);
  assert.throws(() => h.Particles.create("e.vpcf", "0 0 0"), /origin must be/);
  assert.equal(h.created.length, 0);
});
