// @s2script/cs2 — the injected game package.  CS2 schema/game identifiers live ONLY in this file
// (never in core): the shim reads this file at load and hands it to core via
// `s2script_core_register_package` (→ core's `register_injected_package`).  Core NEVER embeds a game
// file — the boundary gate forbids `include_str!(games/…)` in core/src — so the name-leak gate stays
// green.  Core evaluates this source per plugin context, where it sets `globalThis.__s2pkg_cs2 =
// { Pawn }`, which core's `__s2require` returns for `require("@s2script/cs2")`.
//
// Offsets are NOT resolved at load time — the schema isn't populated until a map loads (long after
// a plugin loads).  `Pawn.forSlot()` resolves them lazily on each call; once the schema is ready
// the core OffsetCache caches the hit so the repeated lookup is free.
(function () {
  var EntityRef = require("@s2script/std").EntityRef;

  function Pawn(ref, healthOff) { this.ref = ref; this.healthOff = healthOff; }
  Pawn.prototype = {
    get health() { return this.ref.readInt32(this.healthOff); },        // number | null
    set health(v) {
      if (this.ref.writeInt32(this.healthOff, v)) this.ref.notifyStateChanged(this.healthOff);
    },
  };

  // slot -> controller entity (index slot+1) -> m_hPlayerPawn handle -> pawn EntityRef.
  // CS2 convention: player controllers occupy entity indices slot+1 (confirmed in the live gate).
  // Offsets are resolved here, on every call, so that the first call after the map loads succeeds.
  Pawn.forSlot = function (slot) {
    var HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth");
    var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (HEALTH < 0 || PAWN_HANDLE < 0) return null;

    var ctrlIndex = slot + 1;
    var ctrl = new EntityRef(ctrlIndex, __s2_ent_current_serial(ctrlIndex));
    if (!ctrl.isValid()) return null;

    var handle = ctrl.readInt32(PAWN_HANDLE);           // the m_hPlayerPawn CEntityHandle uint32
    if (handle === null) return null;
    var decoded = __s2_handle_decode(handle >>> 0);      // [index, serial]
    var pawn = new EntityRef(decoded[0], decoded[1]);
    return pawn.isValid() ? new Pawn(pawn, HEALTH) : null;
  };

  globalThis.__s2pkg_cs2 = { Pawn: Pawn };
})();
