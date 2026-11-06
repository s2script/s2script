const test = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const file = join(__dirname, "weapon.js");
const LAYOUT = {
  CBaseEntity: { m_nSubclassID: 1532, m_flAnimTime: 1544 },
  CCSWeaponBaseVData: { m_iMaxClip1: 1232, m_nPrice: 1804, m_bIsFullAuto: 1844, m_flCycleTime: 1856 },
  CFiringModeFloat: { m_flValues: 0 },
};

function load(layout) {
  const lookups = [];
  const sandbox = {
    __s2pkg_cs2: {},
    __s2_schema_offset(cls, field) {
      lookups.push(cls + "." + field);
      const c = layout[cls];
      return c && field in c ? c[field] : -1;
    },
  };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(readFileSync(file, "utf8"), sandbox, { filename: file });
  return { Weapon: sandbox.__s2pkg_cs2.Weapon, lookups };
}

function ref(vdata) {
  const reads = [];
  const r = {
    live: true,
    reads,
    isValid() { return this.live; },
  };
  for (const name of ["readInt32Via", "readBoolVia", "readFloat32Via"]) {
    r[name] = function (path, off) {
      reads.push([name, Array.from(path), off]);
      if (!this.live || vdata === null) return null;
      return off in vdata ? vdata[off] : null;
    };
  }
  return r;
}

test("data reads VData fields through the pointer that follows m_nSubclassID", () => {
  const { Weapon } = load(LAYOUT);
  const r = ref({ 1232: 30, 1804: 2700, 1844: true, 1856: 0.1, 1860: 0.25 });
  const data = new Weapon(r).data;
  assert.equal(Weapon.dataUnavailableReason, null);
  assert.equal(data.maxClip1, 30);
  assert.equal(data.price, 2700);
  assert.equal(data.isFullAuto, true);
  assert.equal(data.cycleTime, 0.1);
  assert.equal(data.cycleTimeAlt, 0.25);
  assert.deepEqual(r.reads[0], ["readInt32Via", [1536], 1232]);
  assert.deepEqual(r.reads[3], ["readFloat32Via", [1536], 1856]);
  assert.deepEqual(r.reads[4], ["readFloat32Via", [1536], 1860]);
  assert.equal(data.damage, null, "a field missing from the schema reads null without a read");
  assert.equal(r.reads.length, 5);
});

test("a changed CBaseEntity layout disables data with a named reason", () => {
  const { Weapon } = load({ ...LAYOUT, CBaseEntity: { m_nSubclassID: 1532, m_flAnimTime: 1552 } });
  const r = ref({ 1232: 30 });
  assert.equal(new Weapon(r).data, null);
  assert.match(Weapon.dataUnavailableReason, /m_flAnimTime - m_nSubclassID = 20, expected 12/);
  assert.equal(r.reads.length, 0, "no read through a guessed pointer");
});

test("a schema miss is not cached and a stale weapon or null VData pointer reads null", () => {
  const layout = { ...LAYOUT, CBaseEntity: {} };
  const { Weapon } = load(layout);
  const r = ref({ 1232: 30 });
  assert.equal(new Weapon(r).data, null);
  assert.match(Weapon.dataUnavailableReason, /not in the schema/);
  layout.CBaseEntity = LAYOUT.CBaseEntity;
  const data = new Weapon(r).data;
  assert.equal(data.maxClip1, 30);
  assert.equal(Weapon.dataUnavailableReason, null);
  r.live = false;
  assert.equal(data.maxClip1, null);
  assert.equal(new Weapon(r).data, null);
  assert.equal(new Weapon(ref(null)).data.maxClip1, null);
});
