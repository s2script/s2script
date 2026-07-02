// @s2script/cs2 — the injected game package. CS2 identifiers live ONLY in this file (never in core).
// The generated field accessors (schema.generated.js) run BEFORE this file (concatenated ahead of it by
// scripts/package-addon.sh) and set globalThis.__s2pkg_cs2_schema; this file applies the generated
// CCSPlayerPawn accessors to Pawn.prototype and keeps the behavioral entry point (Pawn.forSlot).
// Offsets are resolved live (Slice 3) and cached by the core OffsetCache; nothing is baked.
(function () {
  var EntityRef = __s2require("@s2script/entity").EntityRef;
  var schema = globalThis.__s2pkg_cs2_schema;   // set by schema.generated.js

  function Pawn(ref) { this.ref = ref; }
  if (schema) schema.applyAccessors(Pawn.prototype, "CCSPlayerPawn");   // health, friction, controller, ...

  // slot -> controller entity (index slot+1) -> m_hPlayerPawn handle -> pawn EntityRef.
  Pawn.forSlot = function (slot) {
    var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (PAWN_HANDLE < 0) return null;
    var ctrlIndex = slot + 1;
    var ctrl = new EntityRef(ctrlIndex, __s2_ent_current_serial(ctrlIndex));
    if (!ctrl.isValid()) return null;
    var handle = ctrl.readInt32(PAWN_HANDLE);
    if (handle === null) return null;
    var decoded = __s2_handle_decode(handle >>> 0);
    var pawn = new EntityRef(decoded[0], decoded[1]);
    return pawn.isValid() ? new Pawn(pawn) : null;
  };

  globalThis.__s2pkg_cs2 = { Pawn: Pawn };
})();
