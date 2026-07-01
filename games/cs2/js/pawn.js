// @s2script/cs2 — provisional pawn.health accessor (Slice 3). CS2 names live here, never in core.
// Loaded at boot via s2script_core_load_cs2 (real plugin loading is Slice 4).
(function () {
  var HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth");
  var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");

  function Pawn(ent) { this.ent = ent; }
  Pawn.prototype = {
    get health() { return __s2_ent_read_i32(this.ent, HEALTH); },
    set health(v) {
      __s2_ent_write_i32(this.ent, HEALTH, v);
      __s2_ent_state_changed(this.ent, HEALTH); // fold the network state-change into the setter
    },
  };

  // slot -> controller entity -> m_hPlayerPawn handle -> pawn CEntityInstance.
  // CS2 convention: player controllers occupy entity indices slot+1 (confirmed in the live gate).
  function pawnForSlot(slot) {
    if (HEALTH < 0 || PAWN_HANDLE < 0) return null;
    var controller = __s2_entity_by_index(slot + 1);
    if (!controller) return null;
    var handle = __s2_ent_read_i32(controller, PAWN_HANDLE);
    var pawnEnt = __s2_deref_handle(handle);
    return pawnEnt ? new Pawn(pawnEnt) : null;
  }

  globalThis.cs2 = { Pawn: Pawn, pawnForSlot: pawnForSlot, HEALTH_OFFSET: HEALTH };
})();
