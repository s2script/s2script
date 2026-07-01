// @s2script/cs2 — provisional pawn.health accessor (Slice 3). CS2 names live here, never in core.
// Loaded at boot via s2script_core_load_cs2 (real plugin loading is Slice 4).
(function () {
  // Offsets are NOT resolved at IIFE-load time — the schema isn't populated until a map loads,
  // which happens long after Load().  pawnForSlot() resolves them lazily on each call; once
  // the schema is ready the OffsetCache (core) caches the hit so the repeated lookup is free.

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
  function pawnForSlot(slot) {
    var HEALTH = __s2_schema_offset("CCSPlayerPawn", "m_iHealth");
    var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (HEALTH < 0 || PAWN_HANDLE < 0) return null;
    var controller = __s2_entity_by_index(slot + 1);
    if (!controller) return null;
    var handle = __s2_ent_read_i32(controller, PAWN_HANDLE);
    var pawnEnt = __s2_deref_handle(handle);
    return pawnEnt ? new Pawn(pawnEnt, HEALTH) : null;
  }

  // cs2.HEALTH_OFFSET is a lazy getter so that reading it (e.g. in the Slice 3 demo log) always
  // returns the current resolved value rather than the -1 captured at IIFE-load time.
  var cs2 = { Pawn: Pawn, pawnForSlot: pawnForSlot };
  Object.defineProperty(cs2, 'HEALTH_OFFSET', {
    get: function () { return __s2_schema_offset('CCSPlayerPawn', 'm_iHealth'); },
    enumerable: true,
    configurable: true,
  });
  globalThis.cs2 = cs2;
})();
