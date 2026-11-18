const test = require("node:test");
const assert = require("node:assert/strict");
const { mount, cleanup } = require("./gamekit-fake.js");

// Schema offsets the weapon card reads. Values only need to be stable and distinct.
const ICON = "file://{images}/icons/equipment/";
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

// auto reads each player once every 4 frames, its turn set by its slot. One poll() is one read of
// every player on auto.
function poll(m) { for (let i = 0; i < 4; i++) m.frame(); }

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
  assert.equal(m.text(1, "gk_wpn_s0_i0_t"), "AK-47");
  assert.equal(m.text(1, "gk_wpn_s1_i0_t"), "Golden Gun");
  assert.equal(m.has(1, "gk_wpn_s1_i0", "gk-on"), true, "a string row is bright when its row is active");
  assert.equal(m.has(1, "gk_wpn_s0_i0", "gk-on"), false);
  assert.equal(m.hidden(1, "gk_wpn_s0_i0_ic"), true, "a string row has no icon");
  assert.equal(m.hidden(1, "gk_wpn_s0_i1"), true);
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
  poll(m);
  assert.equal(m.hidden(0, "gk_wpn"), false);
  assert.equal(m.text(0, "gk_wpn_name"), "AK-47");
  assert.equal(m.text(0, "gk_wpn_clip"), "30");
  assert.equal(m.text(0, "gk_wpn_res"), "/ 90");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false);
  assert.equal(m.text(0, "gk_wpn_icon"), ICON + "ak47.svg");
  assert.equal(m.text(0, "gk_wpn_s0_i0_ic"), ICON + "ak47.svg");
  assert.equal(m.hidden(0, "gk_wpn_s0_i0_t"), true, "a stock item shows its icon, not its name");
  assert.equal(m.text(0, "gk_wpn_s1_i0_ic"), ICON + "usp_silencer.svg");
  assert.equal(m.text(0, "gk_wpn_s2_i0_ic"), ICON + "knife.svg");
  assert.equal(m.has(0, "gk_wpn_s0_i0", "gk-on"), true);
  assert.equal(m.has(0, "gk_wpn_s1_i0", "gk-on"), false);
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), true);
  assert.equal(m.hidden(1, "gk_wpn"), true, "slot 1 is not on auto");

  // A read with nothing changed sends nothing.
  const before = m.drives;
  poll(m);
  assert.equal(m.drives, before);

  // Shooting: the clip follows, and turns red at or under 25%.
  ak.clip1 = 8;
  poll(m);
  assert.equal(m.text(0, "gk_wpn_clip"), "8");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false, "8 of 30 is above 25%");
  ak.clip1 = 7;
  poll(m);
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), true, "7 of 30 is at or under 25%");

  // Reload: clip refills from reserve, tone goes back.
  ak.clip1 = 30; ak.reserve = 67;
  poll(m);
  assert.equal(m.text(0, "gk_wpn_clip"), "30");
  assert.equal(m.text(0, "gk_wpn_res"), "/ 67");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false);

  // Switch to the knife: name only, knife slot highlighted.
  pawn.activeWeapon = knife;
  poll(m);
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
  poll(m);
  assert.equal(m.text(2, "gk_wpn_name"), "Flashbang");
  assert.equal(m.hidden(2, "gk_wpn_count"), false);
  assert.equal(m.text(2, "gk_wpn_count"), "×2");
  assert.equal(m.hidden(2, "gk_wpn_ammo"), true, "grenades have no clip");
  assert.equal(m.text(2, "gk_wpn_s3_i0_ic"), ICON + "flashbang.svg");
  assert.equal(m.text(2, "gk_wpn_s3_i0_t"), "\u00d72", "the count sits beside the icon");
  assert.equal(m.has(2, "gk_wpn_s3_i0", "gk-on"), true, "the grenade in hand is marked");
  assert.equal(m.text(2, "gk_wpn_s3_i1_ic"), ICON + "hegrenade.svg");
  assert.equal(m.hidden(2, "gk_wpn_s3_i1_t"), true);
  assert.equal(m.has(2, "gk_wpn_s3_i1", "gk-on"), false);
  assert.equal(m.hidden(0, "gk_wpn"), true, "slot 0 has no player: hidden");

  // Pick up a rifle: the next inventory read adds it without a switch.
  pawn.weapons = [weapon(9, { clip: 5, reserve: 30, max: 5 }), glock, flash, he];
  for (let i = 0; i < 4; i++) poll(m);
  assert.equal(m.text(2, "gk_wpn_s0_i0_ic"), ICON + "awp.svg");
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
  poll(m);
  assert.equal(m.hidden(1, "gk_wpn"), false);
  P.get(1).pawnIsAlive = false;
  poll(m);
  assert.equal(m.hidden(1, "gk_wpn"), true, "dead or spectating: nothing shows");
  P.get(1).pawnIsAlive = true;
  poll(m);
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
  poll(m);
  assert.equal(m.text(0, "gk_wpn_name"), "Golden Gun");
  assert.equal(m.has(0, "gk_wpn_name", "gk-t-red"), true);
  assert.ok(seen.includes(64) && seen.includes(36));
  assert.equal(m.hidden(0, "gk_wpn_slots"), true);
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), true, "1 of 8 is low");

  // A throwing name function keeps the default name.
  W.auto("all", { name: () => { throw new Error("boom"); } });
  poll(m);
  assert.equal(m.text(0, "gk_wpn_name"), "R8 Revolver");
  assert.equal(m.text(0, "gk_wpn_s1_i0_ic"), ICON + "revolver.svg", "the slot list is back, both pistols in one row");
  assert.equal(m.text(0, "gk_wpn_s1_i1_ic"), ICON + "p250.svg");
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
  poll(m);
  W.set(0, { name: "Custom", clip: 1, reserve: 1 });
  poll(m);
  assert.equal(m.text(0, "gk_wpn_name"), "Custom", "auto leaves a manual player alone");
  assert.equal(m.text(1, "gk_wpn_name"), "MP9");
  W.auto(0);
  poll(m);
  assert.equal(m.text(0, "gk_wpn_name"), "MP9");

  W.release();
  assert.equal(m.frameSubs, 0, "release drops the frame subscription");
  assert.equal(m.hidden(0, "gk_wpn"), true);
  assert.equal(m.hidden(1, "gk_wpn"), true);
  assert.throws(() => W.set(0, { name: "x" }), /claim the region first/);
});

test("a refused paint is retried in full on the next read", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const m4 = weapon(16, { clip: 30, reserve: 90, max: 30 });
  P.put(0, { weapons: [m4], active: m4 });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  m.refuse.add(0);
  poll(m);
  m.refuse.delete(0);
  poll(m);
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
  poll(m);
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
  poll(m);
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), true);
  pawn.activeWeapon = knife;
  poll(m);
  pawn.activeWeapon = usp;
  poll(m);
  assert.equal(m.text(0, "gk_wpn_clip"), "12");
  assert.equal(m.has(0, "gk_wpn_clip", "gk-t-red"), false, "the knife cleared the AK's low tone");

  // Drop the active rifle, then pick one up while holding the pistol.
  pawn.activeWeapon = ak;
  poll(m);
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), true);
  pawn.weapons = [usp, knife];
  pawn.activeWeapon = usp;
  poll(m);
  assert.equal(m.hidden(0, "gk_wpn_s0"), true);
  pawn.weapons = [weapon(9, { clip: 5, reserve: 30, max: 5 }), usp, knife];
  for (let i = 0; i < 4; i++) poll(m);
  assert.equal(m.hidden(0, "gk_wpn_s0"), false);
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), false, "the returned row is not highlighted");
  assert.equal(m.has(0, "gk_wpn_s1", "gk-sel"), true);
});

test("a row named active while empty is highlighted once it fills", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const W = m.GameHud.weapon;
  W.claim();
  W.set(0, { name: "Fists", slots: { secondary: "Glock-18", active: "primary" } });
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), false);
  W.set(0, { name: "AK-47", clip: 30, reserve: 90, slots: { primary: "AK-47", secondary: "Glock-18", active: "primary" } });
  assert.equal(m.hidden(0, "gk_wpn_s0"), false);
  assert.equal(m.has(0, "gk_wpn_s0", "gk-sel"), true);
});

test("a shared row marks the item in hand: knife and Zeus flip within one read", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  // Inventory order has the Zeus first; the row still reads knife, then Zeus.
  const zeus = weapon(31, { clip: 1, reserve: 0, max: 1 });
  const knife = weapon(42);
  const pawn = P.put(0, { weapons: [zeus, knife], active: knife });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  poll(m);
  assert.equal(m.text(0, "gk_wpn_s2_i0_ic"), ICON + "knife.svg");
  assert.equal(m.text(0, "gk_wpn_s2_i1_ic"), ICON + "taser.svg");
  assert.equal(m.has(0, "gk_wpn_s2", "gk-sel"), true, "the row keeps its accent edge");
  assert.equal(m.has(0, "gk_wpn_s2_i0", "gk-on"), true, "Knife bright");
  assert.equal(m.has(0, "gk_wpn_s2_i1", "gk-on"), false, "Zeus dim");
  assert.equal(m.hidden(0, "gk_wpn_ammo"), true);

  pawn.activeWeapon = zeus;
  poll(m);
  assert.equal(m.has(0, "gk_wpn_s2_i0", "gk-on"), false, "Knife dim");
  assert.equal(m.has(0, "gk_wpn_s2_i1", "gk-on"), true, "Zeus bright");
  assert.equal(m.has(0, "gk_wpn_s2", "gk-sel"), true);
  assert.equal(m.text(0, "gk_wpn_clip"), "1", "the Zeus keeps its clip");
  assert.equal(m.text(0, "gk_wpn_icon"), ICON + "taser.svg");
});

test("grenades sit in buy order and the one in hand is marked by entity, not by name", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const decoy = weapon(47, { ammoType: 17 });
  const he = weapon(44, { ammoType: 13 });
  const smoke = weapon(45, { ammoType: 15 });
  const flash = weapon(43, { ammoType: 14 });
  const pawn = P.put(0, { weapons: [decoy, he, smoke, flash], active: smoke });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  poll(m);
  const icons = [0, 1, 2, 3].map((j) => m.text(0, "gk_wpn_s3_i" + j + "_ic"));
  assert.deepEqual(icons, ["flashbang", "smokegrenade", "hegrenade", "decoy"].map((n) => ICON + n + ".svg"));
  assert.deepEqual([0, 1, 2, 3].map((j) => m.has(0, "gk_wpn_s3_i" + j, "gk-on")), [false, true, false, false]);
  pawn.activeWeapon = decoy;
  poll(m);
  assert.deepEqual([0, 1, 2, 3].map((j) => m.has(0, "gk_wpn_s3_i" + j, "gk-on")), [false, false, false, true]);

  // Two pistols renamed to the same thing: only the one in hand is marked.
  const a = weapon(36, { clip: 13, reserve: 26, max: 13 });
  const b = weapon(36, { clip: 13, reserve: 26, max: 13 });
  pawn.weapons = [a, b];
  pawn.activeWeapon = b;
  W.auto(0, { name: () => "Sidearm" });
  poll(m);
  assert.equal(m.text(0, "gk_wpn_s1_i0_t"), "Sidearm");
  assert.equal(m.text(0, "gk_wpn_s1_i1_t"), "Sidearm");
  assert.equal(m.has(0, "gk_wpn_s1_i0", "gk-on"), false);
  assert.equal(m.has(0, "gk_wpn_s1_i1", "gk-on"), true);
  assert.equal(m.hidden(0, "gk_wpn_s3"), true, "the grenades row is gone");
});

test("a row past four items collapses to +N and keeps the item in hand visible", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const nades = [43, 45, 44, 46, 47, 81].map((def, i) => weapon(def, { ammoType: i }));
  const pawn = P.put(0, { weapons: nades, active: nades[0] });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  poll(m);
  assert.equal(m.text(0, "gk_wpn_s3_more"), "+2");
  assert.equal(m.hidden(0, "gk_wpn_s3_more"), false);
  assert.equal(m.text(0, "gk_wpn_s3_i3_ic"), ICON + "molotov.svg");
  pawn.activeWeapon = nades[5];   // the fire bomb: sixth in the row, no stock icon
  poll(m);
  assert.equal(m.text(0, "gk_wpn_s3_i3_t"), "Fire Bomb", "the item in hand takes the last shown place");
  assert.equal(m.hidden(0, "gk_wpn_s3_i3_ic"), true);
  assert.equal(m.has(0, "gk_wpn_s3_i3", "gk-on"), true);
});

test("set marks an item with { items, active }, takes icons, and options.icon overrides auto", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const W = m.GameHud.weapon;
  W.claim();
  W.set(0, {
    name: "Knife", icon: "file://{images}/x.svg",
    slots: { primary: "AK-47", knife: { items: ["Knife", { text: "Zeus", icon: "file://{images}/z.svg" }], active: 0 }, active: "knife" },
  });
  assert.equal(m.text(0, "gk_wpn_icon"), "file://{images}/x.svg");
  assert.equal(m.text(0, "gk_wpn_s2_i0_t"), "Knife");
  assert.equal(m.text(0, "gk_wpn_s2_i1_t"), "Zeus");
  assert.equal(m.text(0, "gk_wpn_s2_i1_ic"), "file://{images}/z.svg");
  assert.equal(m.has(0, "gk_wpn_s2_i0", "gk-on"), true);
  assert.equal(m.has(0, "gk_wpn_s2_i1", "gk-on"), false);
  assert.equal(m.has(0, "gk_wpn_s0_i0", "gk-on"), false, "a string row that is not active is not marked");
  W.set(0, { name: "Zeus", slots: { knife: { items: ["Knife", "Zeus"], active: 1 }, active: "knife" } });
  assert.equal(m.has(0, "gk_wpn_s2_i0", "gk-on"), false);
  assert.equal(m.has(0, "gk_wpn_s2_i1", "gk-on"), true);
  assert.equal(m.hidden(0, "gk_wpn_s2_i1_ic"), true, "the icon went with the plain item");
  assert.equal(m.hidden(0, "gk_wpn_icon"), true);

  const P = players(m);
  const ak = weapon(7, { clip: 30, reserve: 90, max: 30 });
  P.put(1, { weapons: [ak], active: ak });
  W.auto(1, { icon: (def) => (def === 7 ? "file://{images}/custom.svg" : null) });
  poll(m);
  assert.equal(m.text(1, "gk_wpn_s0_i0_ic"), "file://{images}/custom.svg");
  assert.equal(m.text(1, "gk_wpn_icon"), "file://{images}/custom.svg");
});

test("auto spreads players across frames: each is read once every 4 frames", () => {
  const m = mount({ files: ["gamekit-weapon.js"], slots: [0, 1, 2, 3, 4, 5, 6, 7] });
  const P = players(m);
  const guns = {};
  for (let s = 0; s < 8; s++) {
    guns[s] = weapon(7, { clip: 30, reserve: 90, max: 30 });
    P.put(s, { weapons: [guns[s]], active: guns[s] });
  }
  const W = m.GameHud.weapon;
  W.claim();
  W.auto("all");
  m.frame();
  for (let s = 0; s < 8; s++) assert.equal(m.text(s, "gk_wpn_clip"), "30", "a player not read yet is read at once");

  // Count reads of the active weapon per frame.
  let reads = 0;
  for (let s = 0; s < 8; s++) {
    const pawn = P.get(s).pawn;
    const w = pawn.activeWeapon;
    delete pawn.activeWeapon;
    Object.defineProperty(pawn, "activeWeapon", { get: () => { reads++; return w; }, configurable: true });
  }
  const perFrame = [];
  for (let f = 0; f < 8; f++) { reads = 0; m.frame(); perFrame.push(reads); }
  assert.deepEqual(perFrame, [2, 2, 2, 2, 2, 2, 2, 2], "8 players over 4 turns: 2 a frame");

  // A shot shows within 4 frames.
  guns[5].clip1 = 29;
  let seenAt = -1;
  for (let f = 0; f < 4 && seenAt < 0; f++) { m.frame(); if (m.text(5, "gk_wpn_clip") === "29") seenAt = f; }
  assert.ok(seenAt >= 0, "the new count is painted within one turn");
});

test("auto keeps one slot list until the inventory or the item in hand changes", () => {
  const m = mount({ files: ["gamekit-weapon.js"] });
  const P = players(m);
  const ak = weapon(7, { clip: 30, reserve: 90, max: 30 });
  const knife = weapon(42);
  const pawn = P.put(0, { weapons: [ak, knife], active: ak });
  const W = m.GameHud.weapon;
  W.claim();
  W.auto(0);
  poll(m);
  let lists = 0;
  const weapons = pawn.weapons;
  delete pawn.weapons;
  Object.defineProperty(pawn, "weapons", { get: () => { lists++; return weapons; }, configurable: true });
  for (let i = 0; i < 8; i++) { ak.clip1--; poll(m); }
  assert.equal(m.text(0, "gk_wpn_clip"), "22");
  assert.equal(lists, 2, "8 reads: the inventory is read on every 4th");
  lists = 0;
  pawn.activeWeapon = knife;
  poll(m);
  assert.equal(lists, 1, "a switch reads the inventory at once");
  assert.equal(m.has(0, "gk_wpn_s2", "gk-sel"), true);
});
