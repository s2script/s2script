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
