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

  // --- Slice 5C.2: the Player (controller) model ---
  function Player(ref) { this.ref = ref; }                       // ref = the CONTROLLER EntityRef
  if (schema) schema.applyAccessors(Player.prototype, "CCSPlayerController");  // team, score, ping, ...

  // slot is 0-based (CPlayerSlot); the controller entity index is slot+1.
  Object.defineProperty(Player.prototype, "slot", {
    get: function () { return this.ref.index - 1; }, enumerable: true, configurable: true,
  });

  // player.pawn -> the typed body via m_hPlayerPawn (shadows the raw generated `pawn` = m_hPawn).
  Object.defineProperty(Player.prototype, "pawn", {
    get: function () {
      var off = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
      if (off < 0) return null;
      var h = this.ref.readHandle(off);
      return h ? new Pawn(h) : null;
    }, enumerable: true, configurable: true,
  });

  var MAX_PLAYERS = 64;
  // CS2 pre-allocates all 64 controller entities, so isValid() (entity-exists) does NOT distinguish an
  // occupied slot from an empty one (m_iConnected reads 0 for both; verified live). The clean, schema-readable
  // occupancy signal is that an occupied slot's controller has a valid player pawn (m_hPlayerPawn). This yields
  // in-game (spawned) players; connected-but-pawnless (dead/spectating) is deferred to the engine-identity/
  // connection follow. Offsets stay live-resolved (layout-is-data).
  Player.fromSlot = function (slot) {
    var idx = slot + 1;                                          // controller entity index
    var ref = new EntityRef(idx, __s2_ent_current_serial(idx));
    if (!ref.isValid()) return null;                            // the controller entity must exist
    var poff = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (poff < 0 || ref.readHandle(poff) === null) return null; // occupied iff the controller has a live pawn
    return new Player(ref);
  };
  Player.all = function () {
    var out = [];
    for (var s = 0; s < MAX_PLAYERS; s++) { var p = Player.fromSlot(s); if (p) out.push(p); }
    return out;
  };

  // pawn.controller -> the typed Player via m_hController (shadows the raw generated `controller`).
  Object.defineProperty(Pawn.prototype, "controller", {
    get: function () {
      var off = __s2_schema_offset("CBasePlayerPawn", "m_hController");
      if (off < 0) return null;
      var h = this.ref.readHandle(off);
      return h ? new Player(h) : null;
    }, enumerable: true, configurable: true,
  });

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

  globalThis.__s2pkg_cs2 = { Pawn: Pawn, Player: Player };
})();
