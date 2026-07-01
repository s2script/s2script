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
  function Pawn(ent, healthOff) {
    this.ent = ent;
    this.healthOff = healthOff;
  }
  Pawn.prototype = {
    get health() { return __s2_ent_read_i32(this.ent, this.healthOff); },
    set health(v) {
      __s2_ent_write_i32(this.ent, this.healthOff, v);
      __s2_ent_state_changed(this.ent, this.healthOff); // fold the network state-change into the setter
    },
  };

  // slot -> controller entity -> m_hPlayerPawn handle -> pawn CEntityInstance.
  // CS2 convention: player controllers occupy entity indices slot+1 (confirmed in the live gate).
  // Offsets are resolved here, on every call, so that the first call after the map loads succeeds.
  Pawn.forSlot = function (slot) {
    var HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth");
    var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (HEALTH < 0 || PAWN_HANDLE < 0) return null;
    var controller = __s2_entity_by_index(slot + 1);
    if (!controller) return null;
    var handle = __s2_ent_read_i32(controller, PAWN_HANDLE);
    var pawnEnt = __s2_deref_handle(handle);
    return pawnEnt ? new Pawn(pawnEnt, HEALTH) : null;
  };

  globalThis.__s2pkg_cs2 = { Pawn: Pawn };
})();
