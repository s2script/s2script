// @s2script/cs2 — the injected game package. CS2 identifiers live ONLY in this file (never in core).
// The generated field accessors (schema.generated.js) run BEFORE this file (concatenated ahead of it by
// scripts/package-addon.sh) and set globalThis.__s2pkg_cs2_schema; this file applies the generated
// CCSPlayerPawn accessors to Pawn.prototype and keeps the behavioral entry point (Pawn.forSlot).
// Offsets are resolved live (Slice 3) and cached by the core OffsetCache; nothing is baked.
(function () {
  var EntityRef = __s2require("@s2script/entity").EntityRef;
  var math = __s2require("@s2script/math");
  var Vector = math.Vector, QAngle = math.QAngle;
  var schema = globalThis.__s2pkg_cs2_schema;   // set by schema.generated.js

  function Pawn(ref) { this.ref = ref; }
  if (schema) schema.applyAccessors(Pawn.prototype, "CCSPlayerPawn");   // health, friction, controller, ...
  var nav = globalThis.__s2pkg_cs2_nav;   // set by nav.generated.js (concatenated ahead of pawn.js)
  if (nav) nav.applyNav(Pawn.prototype, "CCSPlayerPawn");   // sceneNode, weaponServices, movementServices, aimPunchServices

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

  // --- Slice 5D.2: engine identity (the connected/pawnless follow promised at Player.fromSlot) ---
  // player.userId — the engine user-id (NOT a schema field); -1 if unassigned/absent.
  Object.defineProperty(Player.prototype, "userId", {
    get: function () { return __s2_client_userid(this.slot); },
    enumerable: true, configurable: true,
  });
  // Construct a Player from a slot when the CONTROLLER entity is valid — pawn NOT required
  // (unlike Player.fromSlot, which pawn-gates for the in-game-only Player.all()).
  Player._fromSlotUnchecked = function (slot) {
    var idx = slot + 1;                                          // controller entity index
    var ref = new EntityRef(idx, __s2_ent_current_serial(idx));
    return ref.isValid() ? new Player(ref) : null;
  };
  // Player.fromUserId(userId) — engine-userid lookup -> Player (pawnless-safe), or null.
  Player.fromUserId = function (userId) {
    var slot = __s2_client_find_by_userid(userId | 0);
    return slot < 0 ? null : Player._fromSlotUnchecked(slot);
  };
  // Player.allConnected() — every CONNECTED player regardless of pawn (the pawnless enumeration),
  // complementing the pawn-gated Player.all(). Uses the engine client list as the occupancy oracle.
  Player.allConnected = function () {
    var out = [];
    for (var s = 0; s < MAX_PLAYERS; s++) {
      if (__s2_client_valid(s)) { var p = Player._fromSlotUnchecked(s); if (p) out.push(p); }
    }
    return out;
  };

  // pawn.origin / pawn.angles -> compat aliases delegating to the generated sceneNode wrapper.
  // (The hand-written pointer-chain reads are superseded by the navgen SceneNode; these aliases
  //  keep backwards-compat for any code that already uses pawn.origin or pawn.angles.)
  Object.defineProperty(Pawn.prototype, "origin", {
    get: function () { var s = this.sceneNode; return s ? s.absOrigin : null; },
    enumerable: true, configurable: true,
  });
  Object.defineProperty(Pawn.prototype, "angles", {
    get: function () { var s = this.sceneNode; return s ? s.absRotation : null; },
    enumerable: true, configurable: true,
  });

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

  globalThis.__s2pkg_cs2 = { Pawn: Pawn, Player: Player, Events: (__s2require("@s2script/events") || {}).Events };
})();
