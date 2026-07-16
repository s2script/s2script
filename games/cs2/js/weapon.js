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
  // non-numeric clip (no write performed).
  Weapon.prototype.setAmmo = function (clip, reserve) {
    if (!this.ref.isValid() || typeof clip !== "number") return false;
    this.clip1 = clip;
    return true;
  };

  // weapon.remove() — the complete "take this weapon away" atom: unequip from the owner (RemovePlayerItem)
  // then destroy the entity (UTIL_Remove via EntityRef.remove). Unowned -> just destroy. Serial-gated: a
  // stale weapon is a no-op false. Returns true iff the entity was removed.
  Weapon.prototype.remove = function () {
    if (!this.ref.isValid()) return false;
    var owner = this.owner;
    if (owner) __s2_remove_player_item(owner.ref.index, owner.ref.serial, this.ref.index, this.ref.serial);
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
