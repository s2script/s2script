const test = require("node:test");
const assert = require("node:assert/strict");
const { mount, cleanup } = require("./gamekit-fake.js");

// Schema offsets the weapon card reads. Values only need to be stable and distinct.
const OFFSETS = {
  "CBasePlayerWeapon.m_pReserveAmmo": 100,
  "CBasePlayerPawn.m_pWeaponServices": 200,
  "CPlayer_WeaponServices.m_iAmmo": 300,
};

let nextId = 1;
// A fake CS2 weapon: the generated accessors the card reads, over a fake EntityRef.
function weapon(def, o = {}) {
  const w = {
    clip1: o.clip === undefined ? -1 : o.clip,
    reserve: o.reserve === undefined ? 0 : o.reserve,
    live: true,
    attributeManager: { item: { itemDefinitionIndex: def } },
    data: o.data === null ? null : Object.assign({ maxClip1: o.max === undefined ? -1 : o.max, gearSlot: o.gear === undefined ? null : o.gear, primaryAmmoType: o.ammoType === undefined ? null : o.ammoType }, o.data || {}),
  };
  w.ref = {
    index: 100 + nextId, id: nextId++,
    isValid: () => w.live,
    readInt32: (off) => (off === OFFSETS["CBasePlayerWeapon.m_pReserveAmmo"] ? w.reserve : null),
  };
  return w;
}

// A fake player per slot: alive or dead, a pawn with weapons, an active weapon and ammo counts.
function players(m) {
  const bySlot = {};
  globalThis.__s2_schema_offset = (cls, field) => {
    const v = OFFSETS[cls + "." + field];
    return v === undefined ? -1 : v;
  };
  globalThis.__s2pkg_cs2.Player = { _fromSlotUnchecked: (slot) => bySlot[slot] || null };
  return {
    put(slot, spec) {
      const ammo = spec.ammo || {};
      const pawn = {
        weapons: spec.weapons,
        activeWeapon: spec.active,
        health: 100,
        ref: {
          isValid: () => true,
          readUInt16Via: (path, off) => {
            assert.deepEqual(path, [OFFSETS["CBasePlayerPawn.m_pWeaponServices"]]);
            return ammo[(off - OFFSETS["CPlayer_WeaponServices.m_iAmmo"]) / 2] || 0;
          },
        },
      };
      bySlot[slot] = { pawnIsAlive: spec.alive !== false, controllingBot: false, pawn };
      return pawn;
    },
    get: (slot) => bySlot[slot],
  };
}

test.afterEach(cleanup);

test("the weapon card is held by one plugin, and calls throw until it is claimed", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const W = m.GameHud.weapon;
  assert.throws(() => W.auto("all"), /claim the region first/);
  assert.equal(W.claim("ttt"), true);
  assert.equal(W.claim("ttt"), true, "claiming again keeps the claim");
  assert.equal(W.isClaimed(), true);
  assert.equal(m.claims["gamekit:weapon"], true);
  assert.equal(m.hidden(0, "gk_wpn"), true, "claiming clears whatever a previous owner left");
  W.release();
  assert.equal(W.isClaimed(), false);
  assert.equal(m.claims["gamekit:weapon"], undefined);
  // Held by another plugin's context.
  globalThis.__s2_ui_pool_claim("gamekit:weapon", 1);
  assert.equal(W.claim(), false);
});

test("set paints name, clip / reserve, tone and the slot list, then hide hides it", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const W = m.GameHud.weapon;
  W.claim();
  W.set(1, {
    name: "{red}Golden Gun", clip: 2, reserve: 0, tone: "red",
    slots: { primary: "AK-47", secondary: "{red}Golden Gun", knife: "Knife", active: "secondary" },
  });
  assert.equal(m.hidden(1, "gk_wpn"), false);
  assert.equal(m.hidden(0, "gk_wpn"), true, "slot 0 was not in the audience");
  assert.equal(m.text(1, "gk_wpn_name"), "Golden Gun");
  assert.equal(m.has(1, "gk_wpn_name", "gk-t-red"), true, "a leading tag colours the name");
  assert.equal(m.text(1, "gk_wpn_clip"), "2");
  assert.equal(m.text(1, "gk_wpn_res"), "/ 0");
  assert.equal(m.has(1, "gk_wpn_clip", "gk-t-red"), true);
  assert.equal(m.hidden(1, "gk_wpn_ammo"), false);
  assert.equal(m.hidden(1, "gk_wpn_slots"), false);
  assert.equal(m.text(1, "gk_wpn_s0_t"), "AK-47");
  assert.equal(m.text(1, "gk_wpn_s1_t"), "Golden Gun");
  assert.equal(m.has(1, "gk_wpn_s1", "gk-sel"), true);
  assert.equal(m.has(1, "gk_wpn_s0", "gk-sel"), false);
  assert.equal(m.hidden(1, "gk_wpn_s3"), true, "no grenades row");

  W.set(1, { name: "Knife" });
  assert.equal(m.hidden(1, "gk_wpn_ammo"), true, "no clip: name only");
  assert.equal(m.hidden(1, "gk_wpn_slots"), true, "no slots: list hidden");
  assert.equal(m.has(1, "gk_wpn_name", "gk-t-red"), false);

  W.hide(1);
  assert.equal(m.hidden(1, "gk_wpn"), true);
  assert.throws(() => W.set(1, {}), /name is required/);
});

test("auto fills the card from the pawn and tracks shots, reloads and switches", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const ak = weapon(7, { clip: 30, reserve: 90, max: 30, gear: 0 });
  const usp = weapon(61, { clip: 12, reserve: 24, max: 12 });
  const knife = weapon(42);
  const pawn = P.put(0, { weapons: [ak, usp, knife], active: ak });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  assert.equal(m.frameSubs, 1);
  m.frame();
  assert.equal(m.hidden(0, "gk_wpn"), false);
  assert.equal(m.text(0, "gk_wpn_name"), "AK-47");
  assert.equal(m.text(0, "gk_wpn_clip"), "30");
  assert.equal(m.text(0, "gk_wpn_res"), "/ 90");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false);
  assert.equal(m.text(0, "gk_wpn_s0_t"), "AK-47");
  assert.equal(m.text(0, "gk_wpn_s1_t"), "USP-S");
  assert.equal(m.text(0, "gk_wpn_s2_t"), "Knife");
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), true);
  assert.equal(m.hidden(1, "gk_wpn"), true, "slot 1 is not on auto");

  // A frame with nothing changed sends nothing.
  const before = m.drives;
  m.frame();
  assert.equal(m.drives, before);

  // Shooting: the clip follows, and turns red at or under 25%.
  ak.clip1 = 8;
  m.frame();
  assert.equal(m.text(0, "gk_wpn_clip"), "8");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false, "8 of 30 is above 25%");
  ak.clip1 = 7;
  m.frame();
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), true, "7 of 30 is at or under 25%");

  // Reload: clip refills from reserve, tone goes back.
  ak.clip1 = 30; ak.reserve = 67;
  m.frame();
  assert.equal(m.text(0, "gk_wpn_clip"), "30");
  assert.equal(m.text(0, "gk_wpn_res"), "/ 67");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false);

  // Switch to the knife: name only, knife slot highlighted.
  pawn.activeWeapon = knife;
  m.frame();
  assert.equal(m.text(0, "gk_wpn_name"), "Knife");
  assert.equal(m.hidden(0, "gk_wpn_ammo"), true);
  assert.equal(m.has(0, "gk_wpn_s2", "gk-sel"), true);
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), false);
});

test("auto shows a grenade count and picks up new weapons", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const glock = weapon(4, { clip: 20, reserve: 120, max: 20 });
  const flash = weapon(43, { ammoType: 14 });
  const he = weapon(44, { ammoType: 13 });
  const pawn = P.put(2, { weapons: [glock, flash, he], active: flash, ammo: { 14: 2, 13: 1 } });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto("all");
  m.frame();
  assert.equal(m.text(2, "gk_wpn_name"), "Flashbang");
  assert.equal(m.hidden(2, "gk_wpn_count"), false);
  assert.equal(m.text(2, "gk_wpn_count"), "×2");
  assert.equal(m.hidden(2, "gk_wpn_ammo"), true, "grenades have no clip");
  assert.equal(m.text(2, "gk_wpn_s3_t"), "Flashbang ×2 · HE Grenade");
  assert.equal(m.hidden(0, "gk_wpn"), true, "slot 0 has no player: hidden");

  // Pick up a rifle: the next inventory read adds it without a switch.
  pawn.weapons = [weapon(9, { clip: 5, reserve: 30, max: 5 }), glock, flash, he];
  for (let i = 0; i < 8; i++) m.frame();
  assert.equal(m.text(2, "gk_wpn_s0_t"), "AWP");
  assert.equal(m.hidden(2, "gk_wpn_s0"), false);
});

test("auto hides the card while dead and shows it again on respawn", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const deagle = weapon(1, { clip: 7, reserve: 35, max: 7 });
  P.put(1, { weapons: [deagle], active: deagle });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto([1]);
  m.frame();
  assert.equal(m.hidden(1, "gk_wpn"), false);
  P.get(1).pawnIsAlive = false;
  m.frame();
  assert.equal(m.hidden(1, "gk_wpn"), true, "dead or spectating: nothing shows");
  P.get(1).pawnIsAlive = true;
  m.frame();
  assert.equal(m.hidden(1, "gk_wpn"), false);
  assert.equal(m.text(1, "gk_wpn_clip"), "7", "a full repaint after the hide");
  assert.equal(m.hidden(1, "gk_wpn_ammo"), false);
});

test("options.name renames items and hideSlots drops the list", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const shop = weapon(64, { clip: 1, reserve: 0, max: 8 });
  const p250 = weapon(36, { clip: 13, reserve: 26, max: 13 });
  P.put(0, { weapons: [shop, p250], active: shop });
  const W = m.GameHud.weapon;
  W.claim();
  const seen = [];
  W.auto("all", {
    name: (def, entity) => { seen.push(def); return entity === shop ? "{red}Golden Gun" : null; },
    hideSlots: true,
  });
  m.frame();
  assert.equal(m.text(0, "gk_wpn_name"), "Golden Gun");
  assert.equal(m.has(0, "gk_wpn_name", "gk-t-red"), true);
  assert.ok(seen.includes(64) && seen.includes(36));
  assert.equal(m.hidden(0, "gk_wpn_slots"), true);
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), true, "1 of 8 is low");

  // A throwing name function keeps the default name.
  W.auto("all", { name: () => { throw new Error("boom"); } });
  m.frame();
  assert.equal(m.text(0, "gk_wpn_name"), "R8 Revolver");
  assert.equal(m.text(0, "gk_wpn_s1_t"), "R8 Revolver \u00b7 P250", "the slot list is back, both pistols in one row");
});

test("set takes a player out of auto, auto puts them back, release stops the frames", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const mp9 = weapon(34, { clip: 30, reserve: 120, max: 30 });
  P.put(0, { weapons: [mp9], active: mp9 });
  P.put(1, { weapons: [mp9], active: mp9 });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto("all");
  m.frame();
  W.set(0, { name: "Custom", clip: 1, reserve: 1 });
  m.frame();
  assert.equal(m.text(0, "gk_wpn_name"), "Custom", "auto leaves a manual player alone");
  assert.equal(m.text(1, "gk_wpn_name"), "MP9");
  W.auto(0);
  m.frame();
  assert.equal(m.text(0, "gk_wpn_name"), "MP9");

  W.release();
  assert.equal(m.frameSubs, 0, "release drops the frame subscription");
  assert.equal(m.hidden(0, "gk_wpn"), true);
  assert.equal(m.hidden(1, "gk_wpn"), true);
  assert.throws(() => W.set(0, { name: "x" }), /claim the region first/);
});

test("a refused paint is retried in full on the next frame", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const m4 = weapon(16, { clip: 30, reserve: 90, max: 30 });
  P.put(0, { weapons: [m4], active: m4 });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  m.refuse.add(0);
  m.frame();
  m.refuse.delete(0);
  m.frame();
  assert.equal(m.hidden(0, "gk_wpn"), false);
  assert.equal(m.text(0, "gk_wpn_clip"), "30");
  assert.equal(m.text(0, "gk_wpn_name"), "M4A4");
});

test("a disconnect forgets the slot", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const nova = weapon(35, { clip: 8, reserve: 32, max: 8 });
  P.put(0, { weapons: [nova], active: nova });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  m.frame();
  m.disconnect(0);
  assert.equal(m.frameSubs, 0, "no player left on auto");
});

test("a low-ammo tone and a slot highlight do not survive a weapon with no clip or a dropped slot", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const ak = weapon(7, { clip: 2, reserve: 90, max: 30 });
  const usp = weapon(61, { clip: 12, reserve: 24, max: 12 });
  const knife = weapon(42);
  const pawn = P.put(0, { weapons: [ak, usp, knife], active: ak });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  m.frame();
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), true);
  pawn.activeWeapon = knife;
  m.frame();
  pawn.activeWeapon = usp;
  m.frame();
  assert.equal(m.text(0, "gk_wpn_clip"), "12");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false, "the knife cleared the AK's low tone");

  // Drop the active rifle, then pick one up while holding the pistol.
  pawn.activeWeapon = ak;
  m.frame();
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), true);
  pawn.weapons = [usp, knife];
  pawn.activeWeapon = usp;
  m.frame();
  assert.equal(m.hidden(0, "gk_wpn_s0"), true);
  pawn.weapons = [weapon(9, { clip: 5, reserve: 30, max: 5 }), usp, knife];
  for (let i = 0; i < 8; i++) m.frame();
  assert.equal(m.hidden(0, "gk_wpn_s0"), false);
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), false, "the returned row is not highlighted");
  assert.equal(m.has(0, "gk_wpn_s1", "gk-sel"), true);
});
