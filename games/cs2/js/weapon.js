// @s2script/cs2 — the Weapon entity object (CCSWeaponBase). CS2 identifiers live ONLY in the CS2 game
// package (never in core). Concatenated by scripts/package-addon.sh AFTER schema.generated.js (which sets
// globalThis.__s2pkg_cs2_schema) and BEFORE pawn.js (whose acquisition getters reference Weapon).
// A weapon IS entity-backed (CCSWeaponBase <- CBaseEntity), so Weapon is EntityRef-backed + serial-gated,
// exactly like Pawn/Player. Cross-refs (weapon.owner -> Pawn) resolve LAZILY via globalThis.__s2pkg_cs2 at
// call time, since pawn.js loads after this file. Offsets are live-resolved by the generated accessors.
(function () {
  var schema = globalThis.__s2pkg_cs2_schema;   // set by schema.generated.js (loaded before this file)

  function Weapon(ref) { this.ref = ref; }
  if (schema) schema.applyAccessors(Weapon.prototype, "CCSWeaponBase");   // clip1, clip2, fallbackPaintKit, ownerEntity, ...

  // weapon.isValid() — serial-gated liveness (delegates to the backing EntityRef).
  Weapon.prototype.isValid = function () { return this.ref.isValid(); };

  // weapon.paintKit — ergonomic alias for the generated fallbackPaintKit (the weapon skin id).
  Object.defineProperty(Weapon.prototype, "paintKit", {
    get: function () { return this.fallbackPaintKit; },
    set: function (v) { this.fallbackPaintKit = v; },
    enumerable: true, configurable: true,
  });

  // weapon.owner — the holding Pawn (m_hOwnerEntity -> a serial-gated EntityRef, wrapped in Pawn). null if
  // unowned (on the ground) / stale. Pawn resolved lazily (pawn.js loads after this file).
  Object.defineProperty(Weapon.prototype, "owner", {
    get: function () {
      var h = this.ownerEntity;   // generated CBaseEntity accessor -> EntityRef | null
      if (!h) return null;
      var Pawn = globalThis.__s2pkg_cs2.Pawn;
      return Pawn ? new Pawn(h) : null;
    },
    enumerable: true, configurable: true,
  });

  // weapon.setAmmo(clip, reserve?) — set the magazine (clip1) via the generated setter. `reserve` is
  // deferred (m_pReserveAmmo layout unverified) — accepted but ignored. Returns false on a stale ref or a
  // non-numeric / non-finite clip (no write performed). Ordinary generated schema access: no ammo native.
  Weapon.prototype.setAmmo = function (clip, reserve) {
    if (typeof clip !== "number" || !isFinite(clip) || !this.ref.isValid()) return false;
    this.clip1 = clip;
    return true;
  };

  // weapon.remove() — owned weapons go through the owner's WeaponServices removal method.
  // That method handles active/last-weapon transitions, removes inventory membership, and calls
  // UTIL_Remove itself. A second EntityRef.remove is unnecessary. Unowned weapons use UTIL_Remove
  // directly. The native void-call receipt is undefined on success and null on failure; this
  // wrapper reports whether removal was scheduled, not whether deferred deletion has completed.
  // An owned weapon must remain intact if its service cannot be reached: destroying it without
  // the inventory transition leaves the active weapon and client prediction inconsistent.
  // The descriptor is resolved by pawn.js after this file, so read it lazily here.
  Weapon.prototype.remove = function () {
    if (!this.ref.isValid()) return false;
    var owner = this.owner;
    var calls = globalThis.__s2pkg_cs2_calls;
    var call = calls && calls.removePlayerItem;
    // The descriptor's receiver.via follows pawn.m_pWeaponServices. The weapon ARG must remain
    // the EntityRef itself (receiver wrappers alone are unwrapped by engineCall).
    if (owner) {
      // Owner resolution/validation can run plugin code. Recheck the weapon last: a stale
      // entity argument otherwise marshals to nullptr without aborting the native invocation.
      if (!call || !owner.ref.isValid() || !this.ref.isValid()) return false;
      return call(owner, this.ref) === undefined;
    }
    return this.ref.remove();
  };

  // weapon.data — the weapon's static VData (CCSWeaponBaseVData: max clip, reserve max, price, damage, ...),
  // shared by every weapon of the same class. The entity's m_pSubclassVData pointer is NOT reflected, so its
  // offset is derived from the reflected neighbours instead of a borrowed constant: CBaseEntity lays out
  // m_nSubclassID (a 4-byte CUtlStringToken), then the 8-byte VData pointer, then m_flAnimTime. The derivation
  // only holds while m_flAnimTime - m_nSubclassID == 12 and the pointer slot is 8-aligned; on any other layout
  // the accessor degrades to null (named reason in `Weapon.dataUnavailableReason`) instead of reading a guess.
  // Field offsets inside the VData are live schema lookups (they walk into CBasePlayerWeaponVData).
  // Read-only: the VData is shared by every weapon of that class, so a write would be a global mutation.
  var vdataOff = null;             // null = not yet derived; -1 = layout check failed
  var vdataReason = null;
  function subclassVDataOffset() {
    if (vdataOff !== null) return vdataOff;
    var sub = __s2_schema_offset("CBaseEntity", "m_nSubclassID");
    var anim = __s2_schema_offset("CBaseEntity", "m_flAnimTime");
    if (sub < 0 || anim < 0) {
      // Not cached: like the generated accessors, a miss is retried on the next read.
      vdataReason = "CBaseEntity.m_nSubclassID or m_flAnimTime is not in the schema";
      return -1;
    }
    vdataReason = (anim - sub !== 12 || (sub + 4) % 8 !== 0)
      ? "CBaseEntity layout changed: m_flAnimTime - m_nSubclassID = " + (anim - sub) + ", expected 12"
      : null;
    vdataOff = vdataReason ? -1 : sub + 4;
    return vdataOff;
  }

  var vdataFieldOffs = {};
  function vdataField(field) {
    var o = vdataFieldOffs[field];
    if (o === undefined) {
      o = __s2_schema_offset("CCSWeaponBaseVData", field);
      if (o >= 0) vdataFieldOffs[field] = o;
    }
    return o;
  }

  // [property, schema field, reader, firing mode?]. A CFiringModeFloat wraps m_flValues, a
  // float32[2] (primary mode, then the alt mode: scoped / silencer-on / burst), so its two halves are
  // separate properties; the 4th entry picks the element.
  var VDATA_FIELDS = [
    ["maxClip1", "m_iMaxClip1", "readInt32Via"],
    ["maxClip2", "m_iMaxClip2", "readInt32Via"],
    ["defaultClip1", "m_iDefaultClip1", "readInt32Via"],
    ["slot", "m_iSlot", "readInt32Via"],
    ["weaponType", "m_WeaponType", "readInt32Via"],
    ["gearSlot", "m_GearSlot", "readInt32Via"],
    ["price", "m_nPrice", "readInt32Via"],
    ["killAward", "m_nKillAward", "readInt32Via"],
    ["primaryReserveAmmoMax", "m_nPrimaryReserveAmmoMax", "readInt32Via"],
    ["secondaryReserveAmmoMax", "m_nSecondaryReserveAmmoMax", "readInt32Via"],
    ["isFullAuto", "m_bIsFullAuto", "readBoolVia"],
    ["isMelee", "m_bMeleeWeapon", "readBoolVia"],
    ["numBullets", "m_nNumBullets", "readInt32Via"],
    ["cycleTime", "m_flCycleTime", "readFloat32Via", 0],
    ["cycleTimeAlt", "m_flCycleTime", "readFloat32Via", 1],
    ["maxSpeed", "m_flMaxSpeed", "readFloat32Via", 0],
    ["maxSpeedAlt", "m_flMaxSpeed", "readFloat32Via", 1],
    ["damage", "m_nDamage", "readInt32Via"],
    ["headshotMultiplier", "m_flHeadshotMultiplier", "readFloat32Via"],
    ["armorRatio", "m_flArmorRatio", "readFloat32Via"],
    ["range", "m_flRange", "readFloat32Via"],
  ];

  var firingValuesOff = null;
  function firingModeValuesOffset() {
    if (firingValuesOff === null) {
      var o = __s2_schema_offset("CFiringModeFloat", "m_flValues");
      if (o < 0) return o;
      firingValuesOff = o;
    }
    return firingValuesOff;
  }

  function WeaponData(ref) { this.ref = ref; }
  VDATA_FIELDS.forEach(function (f) {
    var prop = f[0], field = f[1], reader = f[2], mode = f[3];
    Object.defineProperty(WeaponData.prototype, prop, {
      get: function () {
        var p = subclassVDataOffset(), o = vdataField(field);
        if (mode !== undefined) {
          var values = firingModeValuesOffset();
          o = (o < 0 || values < 0) ? -1 : o + values + 4 * mode;
        }
        if (p < 0 || o < 0) return null;
        return this.ref[reader]([p], o);   // null on a stale ref or a null VData pointer
      },
      enumerable: true, configurable: true,
    });
  });

  // weapon.data — null when the layout check failed or the weapon is stale. A live weapon whose VData
  // pointer is null still returns an object; its fields then read null.
  Object.defineProperty(Weapon.prototype, "data", {
    get: function () {
      if (subclassVDataOffset() < 0 || !this.ref.isValid()) return null;
      return new WeaponData(this.ref);
    },
    enumerable: true, configurable: true,
  });

  // Weapon.dataUnavailableReason — why weapon.data is null on this build, or null when the layout check passed.
  Object.defineProperty(Weapon, "dataUnavailableReason", {
    get: function () { subclassVDataOffset(); return vdataReason; },
    enumerable: true, configurable: true,
  });

  // Weapon.fromEntity(ref) — wrap a raw weapon EntityRef; null if ref is null.
  Weapon.fromEntity = function (ref) { return ref ? new Weapon(ref) : null; };

  // Weapon.findAll(className) — every live entity of `className` as a Weapon (e.g. "weapon_ak47").
  Weapon.findAll = function (className) {
    var entity = __s2require("@s2script/sdk/entity");
    var refs = (entity && entity.Entity) ? entity.Entity.findByClass(String(className)) : [];
    var out = [];
    for (var i = 0; i < refs.length; i++) out.push(new Weapon(refs[i]));
    return out;
  };

  globalThis.__s2pkg_cs2 = Object.assign({}, globalThis.__s2pkg_cs2, { Weapon: Weapon });
})();
