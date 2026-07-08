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
  // player.steamId — the client's SteamID64 as a decimal string (engine identity, NOT a schema field);
  // "0" for bots / unauthenticated.
  Object.defineProperty(Player.prototype, "steamId", {
    get: function () { return __s2_client_steamid(this.slot); },
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

  // player.kick(reason?) — disconnect this player (engine KickClient via the client_kick op).
  Player.prototype.kick = function (reason) {
    __s2_client_kick(this.slot, String(reason == null ? "Kicked by admin" : reason));
  };

  // player.setName(name) — overwrite the player's display name (m_iszPlayerName on the controller).
  // Offset live-resolved via __s2_schema_offset (never baked); notifyStateChanged propagates the write.
  // Returns true on success, false if the field is unresolved or the ref is stale.
  Player.prototype.setName = function (name) {
    var off = __s2_schema_offset("CBasePlayerController", "m_iszPlayerName");
    if (off < 0) return false;
    var ok = this.ref.writeString(off, 128, String(name));
    if (ok) this.ref.notifyStateChanged(off);
    return ok;
  };

  // Player.target(pattern, callerSlot) -> Player[] — SourceMod target-string resolution (core set).
  //   "#<userid>" -> that player; "@all" -> allConnected; "@me" -> the caller (empty from console);
  //   otherwise a case-insensitive name match (exact wins, else all partials). Empty on no match.
  Player.target = function (pattern, callerSlot) {
    if (typeof pattern !== "string" || pattern.length === 0) return [];
    if (pattern === "@all") return Player.allConnected();
    if (pattern === "@me") {
      if (typeof callerSlot !== "number" || callerSlot < 0) return [];
      var me = Player._fromSlotUnchecked(callerSlot);   // pawnless-safe, at parity with @all (target the caller alive or dead)
      return me ? [me] : [];
    }
    if (pattern.charAt(0) === "#") {
      var uid = parseInt(pattern.slice(1), 10);
      if (isNaN(uid)) return [];
      var p = Player.fromUserId(uid);
      return p ? [p] : [];
    }
    var needle = pattern.toLowerCase();
    var conn = Player.allConnected();
    var exact = [], partial = [];
    for (var i = 0; i < conn.length; i++) {
      var nm = conn[i].playerName;
      if (typeof nm !== "string") continue;
      var low = nm.toLowerCase();
      if (low === needle) exact.push(conn[i]);
      else if (low.indexOf(needle) !== -1) partial.push(conn[i]);
    }
    return exact.length ? exact : partial;
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

  // pawn.slay() — kill this pawn via the sig-resolved CommitSuicide engine op (serial-gated; no-op if stale).
  Pawn.prototype.slay = function () {
    __s2_pawn_commit_suicide(this.ref.index, this.ref.serial);
  };

  // pawn.setVelocity(x,y,z) — best-effort velocity write (serial-gated). Writes m_vecAbsVelocity's
  // 3 floats + one notifyStateChanged; returns false if the field is unresolved or the ref is stale.
  Pawn.prototype.setVelocity = function (x, y, z) {
    var off = __s2_schema_offset("CBaseEntity", "m_vecAbsVelocity");
    if (off < 0) return false;
    var ok = this.ref.writeFloat32(off, +x) && this.ref.writeFloat32(off + 4, +y) && this.ref.writeFloat32(off + 8, +z);
    if (ok) this.ref.notifyStateChanged(off);
    return !!ok;
  };

  // pawn.moveType — the pawn's MoveType_t (a uint8 enum → not codegen'd, so hand-written). GET reads
  // m_MoveType (null on a stale ref). SET writes BOTH m_MoveType AND m_nActualMoveType (CS2 uses the
  // Type/ActualType pair — one alone may not take) + notifyStateChanged. @s2script/funcommands uses this
  // for noclip (NOCLIP=7 <-> WALK=2) and freeze (NONE=0). MoveType_t (const.h): NONE=0, WALK=2, NOCLIP=7.
  Object.defineProperty(Pawn.prototype, "moveType", {
    get: function () {
      var o = __s2_schema_offset("CBaseEntity", "m_MoveType");
      return o < 0 ? null : this.ref.readUInt8(o);
    },
    set: function (v) {
      var o1 = __s2_schema_offset("CBaseEntity", "m_MoveType");
      var o2 = __s2_schema_offset("CBaseEntity", "m_nActualMoveType");
      if (o1 < 0) return;
      var ok = this.ref.writeUInt8(o1, v | 0);
      if (o2 >= 0) this.ref.writeUInt8(o2, v | 0);
      if (ok) this.ref.notifyStateChanged(o1);
    }
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

  // CS2 chat color control bytes (values from CounterStrikeSharp's ChatColors enum). A message sent to
  // the chat box needs a leading control byte to render; the PLUGIN composes colored messages with these
  // (SourceMod-parity — color is content, not a native-layer default). Frozen so consumers can't mutate.
  var ChatColors = Object.freeze({
    Default: "\x01", White: "\x01", DarkRed: "\x02", LightPurple: "\x03", Green: "\x04", Olive: "\x05",
    Lime: "\x06", Red: "\x07", Grey: "\x08", Yellow: "\x09", Silver: "\x0A", Blue: "\x0B", DarkBlue: "\x0C",
    BlueGrey: "\x0D", Purple: "\x0E", LightRed: "\x0F", Orange: "\x10"
  });

  // --- Activity.formatSource: SourceMod FormatActivitySource port ---
  // activity.js (concatenated ahead of pawn.js) sets globalThis.__s2_activity = { computeActivitySource, SHOW_ACTIVITY_DEFAULT }.
  var __act = globalThis.__s2_activity;
  // Resolve @s2script/admin lazily + memoized: at IIFE-init the admin prelude may not be registered
  // yet, so eager resolution could abort CS2 module init; by formatSource call-time it is always present.
  var __adminMod = null;
  function __resolveAdmin() {
    if (__adminMod === null) __adminMod = __s2require("@s2script/admin") || {};
    return __adminMod;
  }

  var Activity = {
    formatSource: function (actorSlot, recipientSlot) {
      var __a = __resolveAdmin();
      var Admin = __a.Admin, ADMFLAG = __a.ADMFLAG;
      var flags = __act.SHOW_ACTIVITY_DEFAULT;
      var actorReal, actorLabel;
      if (actorSlot < 0) { actorReal = "Console"; actorLabel = "Console"; }
      else {
        var ap = Player.fromSlot(actorSlot);
        // SM FormatActivitySource: unresolvable actor name falls back to "ADMIN", not "".
        actorReal = (ap && ap.playerName) ? ap.playerName : "ADMIN";
        var aAdmin = Admin.forSlot(actorSlot);
        actorLabel = (aAdmin && aAdmin.hasFlags(ADMFLAG.GENERIC)) ? "ADMIN" : "PLAYER";
      }
      var recipientIsAdmin = false, recipientIsRoot = false;
      var rAdmin = Admin.forSlot(recipientSlot);
      if (rAdmin) { recipientIsAdmin = rAdmin.hasFlags(ADMFLAG.GENERIC); recipientIsRoot = rAdmin.hasFlags(ADMFLAG.ROOT); }
      return __act.computeActivitySource(flags, actorLabel, actorReal, recipientIsAdmin, recipientIsRoot, actorSlot === recipientSlot);
    }
  };

  // --- CS2 center menu renderer: WASD input (schema poll) + show_survival_respawn_status HTML ---
  // The ONLY file with CS2 menu facts (button-mask schema fields, the show_survival_respawn_status
  // event/loc_token). @s2script/menu (the model + chat renderer + registerRenderer seam) stays
  // engine-generic; this registers the "center" backend into it.
  (function () {
    if (!globalThis.__s2pkg_menu) return;   // menu module present?
    var Events = globalThis.__s2pkg_events.Events;
    var OnGameFrame = globalThis.__s2pkg_frame.OnGameFrame;
    var centerSessions = {};   // slot -> session
    var prevMask = {};         // slot -> last button mask (edge detect)
    var pollSub = null;        // lazy OnGameFrame subscription (armed only while >=1 center menu is open)

    // IN_* bit values (Source button flags, third_party/hl2sdk/game/shared/in_buttons.h).
    var IN_FORWARD = 8, IN_BACK = 16, IN_USE = 32;

    // The live "buttons held" mask, via the pawn's movement-services pointer:
    //   pawn (root) --m_pMovementServices(ptr)--> CPlayer_MovementServices
    //     .m_nButtons (CInButtonState, embedded @ +80) .m_pButtonStates[0] (uint64, embedded @ +8)
    // Offsets are re-resolved on every call (never cached at module scope) — the same self-healing
    // convention as schema.generated.js/nav.generated.js, so a pawn.js load before the schema is warm
    // degrades to 0 (no input) instead of baking in a permanent -1.
    function readButtons(slot) {
      var p = Player.fromSlot(slot); if (!p) return 0;
      var pawn = p.pawn; if (!pawn) return 0;
      var msPtrOff = __s2_schema_offset("CBasePlayerPawn", "m_pMovementServices");
      var btnOff = __s2_schema_offset("CPlayer_MovementServices", "m_nButtons");
      var btnStateOff = __s2_schema_offset("CInButtonState", "m_pButtonStates");
      if (msPtrOff < 0 || btnOff < 0 || btnStateOff < 0) return 0;
      var s = pawn.ref.readUInt64Via([msPtrOff], btnOff + btnStateOff);   // index 0 of m_pButtonStates[3]
      return (s === null) ? 0 : Number(s);   // menu bits are low -> Number is exact
    }
    // The engine userid for the slot (-1 if unassigned/absent, e.g. a slot that just disconnected).
    // CS2's client-side handler for this event filters per-player on userid (SM's PrintToCenterHtml
    // parity), so every fireToClient call below must carry the real target userid, not the field's
    // zero-value default.
    function getUserId(slot) { return __s2_client_userid(slot | 0); }
    function escapeHtml(s) { return ("" + s).replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
    function renderHtml(session) {
      var v = session.view(), html = "<font class='fontSize-l' color='#ffffff'>" + escapeHtml(v.title) + "</font>";
      for (var i = 0; i < v.lines.length; i++) {
        var l = v.lines[i], color = l.cursor ? "#00ff00" : "#cccccc", mark = l.cursor ? "&#9654; " : "";
        html += "<br><font color='" + color + "'>" + mark + escapeHtml(l.text) + "</font>";
      }
      return html;
    }
    function ensurePoll() {
      if (pollSub) return;
      pollSub = OnGameFrame.subscribe(function () {
        for (var slot in centerSessions) {
          var s = centerSessions[slot]; if (!s || s._ended) continue;
          var sl = slot | 0, mask = readButtons(sl), prev = prevMask[sl] || 0, pressed = mask & ~prev;
          prevMask[sl] = mask;
          if (pressed & IN_FORWARD) s.moveUp();
          else if (pressed & IN_BACK) s.moveDown();
          else if (pressed & IN_USE) s.confirm();
          // Re-send every tick — CS2 paints show_survival_respawn_status's loc_token for one frame only.
          if (!s._ended) Events.fireToClient(sl, "show_survival_respawn_status", { loc_token: renderHtml(s), duration: 5, userid: getUserId(sl) });
        }
      });
    }
    function stopPollIfIdle() {
      for (var k in centerSessions) { if (centerSessions[k]) return; }
      if (pollSub) { pollSub.dispose(); pollSub = null; }   // OnGameFrame.subscribe() -> { dispose() }
    }
    globalThis.__s2pkg_menu.Menu.registerRenderer(globalThis.__s2pkg_menu.MenuStyle.Center, {
      open: function (session) {
        centerSessions[session.slot] = session; prevMask[session.slot] = 0; ensurePoll();
        Events.fireToClient(session.slot, "show_survival_respawn_status", { loc_token: renderHtml(session), duration: 5, userid: getUserId(session.slot) });
      },
      update: function (session) { /* no-op: the next poll tick re-fires with the current view */ },
      close: function (slot) {
        delete centerSessions[slot]; delete prevMask[slot]; stopPollIfIdle();
        Events.fireToClient(slot, "show_survival_respawn_status", { loc_token: "", duration: 0, userid: getUserId(slot) });   // clear
      },
    });
  })();

  globalThis.__s2pkg_cs2 = { Pawn: Pawn, Player: Player, Events: (__s2require("@s2script/events") || {}).Events, ChatColors: ChatColors, Activity: Activity };
})();
