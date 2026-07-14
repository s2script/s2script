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
  var Weapon = globalThis.__s2pkg_cs2.Weapon;   // set by weapon.js (concatenated before this file)

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

  // player.changeTeam(team) — move this player's controller between teams (Spectator=1/T=2/CT=3) via the
  // sig-resolved CCSPlayerController::ChangeTeam engine op (serial-gated; no-op if stale/unresolved).
  Player.prototype.changeTeam = function (team) {
    __s2_player_change_team(this.ref.index, this.ref.serial, team | 0);
  };
  // player.spectate() — move this player to the Spectator team (SM parity; = changeTeam(1)).
  Player.prototype.spectate = function () {
    __s2_player_change_team(this.ref.index, this.ref.serial, 1);
  };

  // Player.target(pattern, callerSlot, filterImmunity) -> Player[] — SM target-string resolution.
  //   "#<userid>" -> that player; "@all" -> allConnected; "@me" -> the caller (empty from console);
  //   otherwise a case-insensitive name match (exact wins, else all partials). Empty on no match.
  //   filterImmunity (default false): drop targets the caller can't act on (admin immunity); used by
  //   the destructive base commands. Degrades to no-filter if @s2script/admin isn't loaded.
  Player.target = function (pattern, callerSlot, filterImmunity) {
    if (typeof pattern !== "string" || pattern.length === 0) return [];
    var res;
    if (pattern === "@all") {
      res = Player.allConnected();
    } else if (pattern === "@me") {
      if (typeof callerSlot !== "number" || callerSlot < 0) return [];
      var me = Player._fromSlotUnchecked(callerSlot);
      res = me ? [me] : [];
    } else if (pattern.charAt(0) === "#") {
      var uid = parseInt(pattern.slice(1), 10);
      if (isNaN(uid)) return [];
      var p = Player.fromUserId(uid);
      res = p ? [p] : [];
    } else {
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
      res = exact.length ? exact : partial;
    }
    if (filterImmunity && typeof globalThis.__s2_admin_can_target === "function") {
      var ct = globalThis.__s2_admin_can_target, out = [];
      for (var k = 0; k < res.length; k++) if (ct(callerSlot | 0, res[k].slot)) out.push(res[k]);
      return out;
    }
    return res;
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

  // --- Item / weapon manipulation slice (Task 4): pawn.giveNamedItem/weapons/stripWeapons/
  // dropActiveWeapon/removeWeapon — over the Task-1 ops (__s2_give_named_item/
  // __s2_entity_subobj_vcall/__s2_remove_player_item) + EntityRef.readHandleVector (Task 1).
  // Offsets are live-resolved via __s2_schema_offset (never baked, self-healing); __s2_schema_offset
  // walks the base-class chain (schema_find_field in the shim), so passing "CCSPlayer_WeaponServices"
  // still resolves m_hMyWeapons even though it's declared on the base CPlayer_WeaponServices.

  // pawn.giveNamedItem(name) — give this pawn a weapon/item by classname (CsItem.AK47 or a raw "weapon_*"
  // string). Returns the created Weapon, or null if the ItemServices ptr is unresolved / failed / stale.
  Pawn.prototype.giveNamedItem = function (name) {
    var off = __s2_schema_offset("CBasePlayerPawn", "m_pItemServices");
    if (off < 0) return null;
    var ref = __s2_give_named_item(this.ref.index, this.ref.serial, off, String(name));
    return ref ? new Weapon(ref) : null;
  };

  // pawn.emitSound(name, opts?) — play a named CS2 SoundEvent from this pawn (its serial-gated
  // EntityRef is the source entity; a stale ref emits nothing -> 0). opts = { recipients?: slots[],
  // volume?: [0,1] } — same as Sound.emit minus entity. Returns the engine sound GUID or 0.
  Pawn.prototype.emitSound = function (name, opts) {
    var pkg = globalThis.__s2pkg_sound;
    if (!pkg || !pkg.Sound) return 0;
    var o = opts || {};
    return pkg.Sound.emit(name, { entity: this.ref, recipients: o.recipients, volume: o.volume });
  };

  // pawn.activeWeapon — the currently-deployed weapon (m_hActiveWeapon on WeaponServices), as a Weapon.
  // null if unresolved / none / stale.
  Object.defineProperty(Pawn.prototype, "activeWeapon", {
    get: function () {
      var ws = this.weaponServices;               // nav wrapper (may be null)
      var h = ws ? ws.activeWeapon : null;        // -> EntityRef | null
      return h ? new Weapon(h) : null;
    },
    enumerable: true, configurable: true,
  });

  // pawn.weapons — this pawn's held weapons (m_hMyWeapons, a CUtlVector<CHandle> on the WeaponServices
  // sub-object), each decoded + serial-gated into a live Weapon. [] if offsets/chain unresolved / stale.
  Object.defineProperty(Pawn.prototype, "weapons", {
    get: function () {
      var wsOff = __s2_schema_offset("CBasePlayerPawn", "m_pWeaponServices");
      var vecOff = __s2_schema_offset("CCSPlayer_WeaponServices", "m_hMyWeapons");
      if (wsOff < 0 || vecOff < 0) return [];
      var refs = this.ref.readHandleVector([wsOff], vecOff, 64);
      var out = [];
      for (var i = 0; i < refs.length; i++) out.push(new Weapon(refs[i]));
      return out;
    },
    enumerable: true, configurable: true,
  });

  // pawn.removeWeapon(weapon) — remove ONE Weapon (delegates to the Weapon.remove atom: unequip via
  // RemovePlayerItem + destroy via UTIL_Remove). false if the weapon is absent/stale.
  Pawn.prototype.removeWeapon = function (weapon) {
    return weapon ? weapon.remove() : false;
  };

  // pawn.stripWeapons() / pawn.disarm() — remove ALL held weapons by folding over Weapon.remove(). `ws` is
  // a snapshot (each Weapon is independent + serial-gated), so mutating m_hMyWeapons mid-loop is safe.
  // Returns true iff every weapon removed.
  Pawn.prototype.stripWeapons = function () {
    var ws = this.weapons;
    var ok = true;
    for (var i = 0; i < ws.length; i++) { if (!ws[i].remove()) ok = false; }
    return ok;
  };
  Pawn.prototype.disarm = function () { return this.stripWeapons(); };   // destroy-all alias

  // pawn.dropActiveWeapon() — still DEFERRED (always false). A true DROP spawns the weapon as a world
  // pickup, which CANNOT be composed from removeWeapon/remove (those DESTROY the weapon); it needs the
  // real CCSPlayer_ItemServices::DropActivePlayerWeapon function. Task 2's live disasm spike found the
  // borrowed vtable index 24 resolves, on this pinned libserver.so, to a GiveNamedItem-overload THUNK
  // (not DropActivePlayerWeapon) — calling through would pass an entity ptr as GiveNamedItem's
  // `const char* name` (an unsafe read, violating degrade-never-crash). Stays UNWIRED until the correct
  // function is self-resolved by SIGNATURE (a follow-up RE spike — NOT a borrowed vtable index).
  Pawn.prototype.dropActiveWeapon = function () { return false; };

  // --- Player fire control: the effective "can't fire" gate is m_flNextAttack (a GameTime_t, seconds) on
  // the CCSPlayer_WeaponServices SUB-OBJECT, reached via the m_pWeaponServices pointer. Written through the
  // write-chain primitive (writeFloat32Via). The fire check is server-authoritative, so the raw write blocks
  // the shot — no notifyStateChanged needed. It's a time gate the engine advances past: a durable block is a
  // large `seconds` or a per-OnGameFrame refresh (the caller's policy).
  function fireGateOffsets() {
    var wsOff = __s2_schema_offset("CBasePlayerPawn", "m_pWeaponServices");
    var naOff = __s2_schema_offset("CCSPlayer_WeaponServices", "m_flNextAttack");
    return (wsOff < 0 || naOff < 0) ? null : { ws: wsOff, na: naOff };
  }
  function nowGameTime() {
    var Server = __s2require("@s2script/server").Server;
    var t = Server ? Server.gameTime : 0;
    return (typeof t === "number") ? t : 0;
  }

  // pawn.nextAttack — the current m_flNextAttack (seconds), or null if unresolved/stale. Read companion to
  // blockFiring (verifies the write landed).
  Object.defineProperty(Pawn.prototype, "nextAttack", {
    get: function () {
      var o = fireGateOffsets();
      return o ? this.ref.readFloat32Via([o.ws], o.na) : null;
    },
    enumerable: true, configurable: true,
  });

  // pawn.blockFiring(seconds?) — block ALL weapon fire for `seconds` (default ~effectively-indefinite).
  // Writes m_flNextAttack = gameTime + seconds. Returns false if unresolved/stale.
  Pawn.prototype.blockFiring = function (seconds) {
    var o = fireGateOffsets();
    if (!o) return false;
    var dur = (typeof seconds === "number" && isFinite(seconds)) ? seconds : 1e9;
    return this.ref.writeFloat32Via([o.ws], o.na, nowGameTime() + dur);
  };

  // pawn.allowFiring() — clear the block (m_flNextAttack = now). Returns false if unresolved/stale.
  Pawn.prototype.allowFiring = function () {
    var o = fireGateOffsets();
    if (!o) return false;
    return this.ref.writeFloat32Via([o.ws], o.na, nowGameTime());
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

  // pawn.buttons — the live "buttons held" mask (low 32 bits as a Number, so bitwise edge-detection
  // works), via the same movement-services pointer chain the center-menu poller below uses (readButtons)
  // — kept identical (CBasePlayerPawn -> m_pMovementServices -> CPlayer_MovementServices.m_nButtons ->
  // CInButtonState.m_pButtonStates[0]) so the two never drift. IN_USE = 32 (in_buttons.h). 0 if the
  // chain/ref is unreadable (stale ref, or before the schema is warm).
  Object.defineProperty(Pawn.prototype, "buttons", {
    get: function () {
      var msPtrOff = __s2_schema_offset("CBasePlayerPawn", "m_pMovementServices");
      var btnOff = __s2_schema_offset("CPlayer_MovementServices", "m_nButtons");
      var btnStateOff = __s2_schema_offset("CInButtonState", "m_pButtonStates");
      if (msPtrOff < 0 || btnOff < 0 || btnStateOff < 0) return 0;
      var v = this.ref.readUInt64Via([msPtrOff], btnOff + btnStateOff);   // index 0 of m_pButtonStates[3]
      return v === null ? 0 : Number(v & 0xFFFFFFFFn);
    },
    configurable: true
  });

  // pawn.aimTrace(opts?) — trace from the pawn's eyes along its view angles: "what is this player
  // looking at". The engine-generic ray-trace (CNavPhysicsInterface::TraceShape) lives in
  // @s2script/trace; this composes the CS2 eye position + eyeAngles. Eye = the body world origin +
  // the standing view-offset (~64u; m_vecViewOffset isn't a generated accessor, so a constant — a
  // crouched eye (~46u) is close enough since the aim DIRECTION from eyeAngles dominates the trace).
  // Ignores the pawn's own entity by default (don't self-hit). Returns a TraceHit, or null if the
  // body transform / eye angles are unreadable (stale ref). CS2 field names stay in this game layer.
  Pawn.prototype.aimTrace = function (opts) {
    var s = this.sceneNode; var o = s ? s.absOrigin : null;
    var a = this.eyeAngles;
    if (!o || !a) return null;
    var eye = { x: o.x, y: o.y, z: o.z + 64 };   // Trace.ray reads .x/.y/.z (plain object is fine)
    return globalThis.__s2pkg_trace.Trace.ray(eye, a, (opts && opts.distance) || 8192, {
      mask: opts && opts.mask,
      ignoreEntity: (opts && opts.ignoreEntity !== undefined) ? opts.ignoreEntity : this.ref
    });
  };

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
    var frozenMoveType = {};   // slot -> pre-freeze m_MoveType, when menu.freezePlayer captured it (restore on close)
    var MOVETYPE_NONE = 0;     // MoveType_t::MOVETYPE_NONE (funcommands sm_freeze parity)
    // Each show_survival_respawn_status frame shows for ~this long; we re-send every tick to keep the menu
    // up, so a short TTL means it self-clears within ~TTL once we STOP re-sending (on close/select) — the
    // old 5s made the menu linger for seconds after selecting. Integer (the event field is int), > the
    // frame interval (~16ms @64tick) so no flicker.
    var MENU_TTL = 1;

    // Freeze the player's movement while a freezePlayer menu is open (buttons still register, so WASD nav
    // still works). Capture the current moveType so close() can restore it. No-op if already frozen / no pawn.
    function freezeIfRequested(session) {
      if (!session.menu.freezePlayer || frozenMoveType[session.slot] !== undefined) return;
      var p = Player.fromSlot(session.slot), pawn = p && p.pawn;
      if (!pawn) return;
      var mt = pawn.moveType;
      if (mt === null || mt === MOVETYPE_NONE) return;   // unreadable or already frozen — don't capture/restore
      frozenMoveType[session.slot] = mt;
      pawn.moveType = MOVETYPE_NONE;
    }
    function unfreeze(slot) {
      if (frozenMoveType[slot] === undefined) return;
      var mt = frozenMoveType[slot]; delete frozenMoveType[slot];
      var p = Player.fromSlot(slot), pawn = p && p.pawn;
      if (pawn) pawn.moveType = mt;                       // gone/dead pawn -> nothing to restore (respawn defaults)
    }

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
    function escapeHtml(s) { return ("" + s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
    // A fixed HEADER / CONTENT / FOOTER layout for the center HUD. CS2 center HTML is one flowing text
    // block with no reserved regions, so the only way to keep the footer visible is to BOUND the content:
    // header(1) + a fixed MAX_VISIBLE-row scrolled window + footer(1) = a constant line count that always
    // leaves the footer on-screen (a long list scrolls WITHIN the window instead of pushing the footer off).
    // Font tiers (CS2MenuManager parity): fontSize-m (header) > fontSize-sm (content) > fontSize-s (footer).
    function renderHtml(session) {
      var v = session.view(), MAX_VISIBLE = 6;
      // HEADER.
      var head = "<font class='fontSize-m' color='#ffd700'>" + escapeHtml(v.title) + "</font>";
      // CONTENT — a fixed-height window scrolled to keep the cursor visible.
      var total = v.lines.length, cursorIdx = 0;
      for (var c = 0; c < total; c++) { if (v.lines[c].cursor) { cursorIdx = c; break; } }
      var start = 0;
      if (total > MAX_VISIBLE) {
        start = cursorIdx - (MAX_VISIBLE >> 1);
        if (start < 0) start = 0;
        if (start + MAX_VISIBLE > total) start = total - MAX_VISIBLE;
      }
      var end = (start + MAX_VISIBLE < total) ? start + MAX_VISIBLE : total;
      var body = "";
      for (var i = start; i < end; i++) {
        var l = v.lines[i], color = l.cursor ? "#00ff00" : "#cccccc", mark = l.cursor ? "&#9654; " : "";
        body += "<br><font class='fontSize-sm' color='" + color + "'>" + mark + escapeHtml(l.text) + "</font>";
      }
      // FOOTER (always the last line): the WASD control legend + page + scroll indicators folded in (so they
      // cost no content rows). The button mapping is CS2-specific, so this legend lives in this renderer.
      var foot = "<br><font class='fontSize-s' color='#8a8a8a'>[W/S] Move &nbsp; [E] Select";
      if (v.pageCount > 1) foot += " &nbsp; " + (v.page + 1) + "/" + v.pageCount;
      var scroll = (start > 0 ? "&#9650;" : "") + (end < total ? "&#9660;" : "");
      if (scroll) foot += " &nbsp; " + scroll;
      foot += "</font>";
      return head + body + foot;
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
          if (!s._ended) Events.fireToClient(sl, "show_survival_respawn_status", { loc_token: renderHtml(s), duration: MENU_TTL, userid: getUserId(sl) });
        }
      });
    }
    function stopPollIfIdle() {
      for (var k in centerSessions) { if (centerSessions[k]) return; }
      if (pollSub) { pollSub.dispose(); pollSub = null; }   // OnGameFrame.subscribe() -> { dispose() }
    }
    globalThis.__s2pkg_menu.Menu.registerRenderer(globalThis.__s2pkg_menu.MenuStyle.Center, {
      open: function (session) {
        // Seed prevMask with the CURRENT button mask (not 0) so a button still held from the action that
        // opened this menu (e.g. E pressed to select the parent category) is NOT seen as a fresh rising
        // edge on the next poll — otherwise chaining menus with E "bleeds through" and instantly confirms.
        centerSessions[session.slot] = session; prevMask[session.slot] = readButtons(session.slot); freezeIfRequested(session); ensurePoll();
        Events.fireToClient(session.slot, "show_survival_respawn_status", { loc_token: renderHtml(session), duration: MENU_TTL, userid: getUserId(session.slot) });
      },
      update: function (session) { /* no-op: the next poll tick re-fires with the current view */ },
      close: function (slot) {
        delete centerSessions[slot]; delete prevMask[slot]; unfreeze(slot); stopPollIfIdle();
        Events.fireToClient(slot, "show_survival_respawn_status", { loc_token: " ", duration: MENU_TTL, userid: getUserId(slot) });   // clear
      },
    });
  })();

  // --- CS2 vote-tally renderer: the live center HTML for @s2script/votes (NON-freezing; no input). ---
  // @s2script/votes (the vote model + chat ballot/capture + lifecycle + registerTallyRenderer seam) stays
  // engine-generic; this registers the live-tally display backend into it, reusing the same
  // show_survival_respawn_status/loc_token path as the center menu renderer above (re-sent each tick since
  // CS2 paints that event's HTML for one frame only). Unlike the menu, there is no input/freeze — the vote
  // itself is captured via chat digits (@s2script/votes), so this is purely a per-tick display.
  (function () {
    if (!globalThis.__s2pkg_votes) return;   // votes module present?
    var Events = globalThis.__s2pkg_events.Events;
    var OnGameFrame = globalThis.__s2pkg_frame.OnGameFrame;
    var tallies = {};   // slot -> current tally (VoteTally), while a live-tally vote is active
    var pollSub = null; // lazy OnGameFrame subscription (armed only while >=1 tally is showing)
    var MENU_TTL = 1;   // same self-clearing-TTL discipline as the center menu renderer above

    // The engine userid for the slot (CS2's client-side handler for this event filters per-player on
    // userid — same reason the center menu renderer above resolves it per fireToClient call).
    function getUserId(slot) { return __s2_client_userid(slot | 0); }
    // Verbatim copy of the center menu renderer's escapeHtml (that one is local to its own IIFE above,
    // not shared) — keep both in sync if this ever changes.
    function escapeHtml(s) { return ("" + s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }

    function renderTallyHtml(t) {
      var html = "<font class='fontSize-m' color='#ffd700'>" + escapeHtml(t.question) + "</font>";
      for (var i = 0; i < t.options.length; i++) {
        var o = t.options[i];
        html += "<br><font class='fontSize-sm' color='#cccccc'>" + (i + 1) + ". " + escapeHtml(o.label) + " — " + o.count + "</font>";
      }
      html += "<br><font class='fontSize-s' color='#8a8a8a'>" + t.total + " voted &nbsp; " + t.secondsLeft + "s</font>";
      return html;
    }
    function ensurePoll() {
      if (pollSub) return;
      pollSub = OnGameFrame.subscribe(function () {
        for (var slot in tallies) {
          var sl = slot | 0;
          Events.fireToClient(sl, "show_survival_respawn_status", { loc_token: renderTallyHtml(tallies[slot]), duration: MENU_TTL, userid: getUserId(sl) });
        }
      });
    }
    function stopIfIdle() {
      for (var k in tallies) { if (tallies[k]) return; }
      if (pollSub) { pollSub.dispose(); pollSub = null; }   // OnGameFrame.subscribe() -> { dispose() }
    }
    function clearTally(slot) {
      delete tallies[slot]; stopIfIdle();
      Events.fireToClient(slot, "show_survival_respawn_status", { loc_token: " ", duration: MENU_TTL, userid: getUserId(slot) });   // wipe
    }
    globalThis.__s2pkg_votes.Vote.registerTallyRenderer({
      show: function (slot, tally) { tallies[slot] = tally; ensurePoll(); },
      clear: clearTally,
    });
    // Defensive self-heal (blocking review finding): a voter who disconnects mid-vote is removed from
    // the core vote's tally (@s2script/votes' own Clients.onDisconnect deletes st.votes[slot]), but the
    // core's clear-at-end pass only walks __s2_vote_eligibleSlots() AT END TIME, which excludes anyone
    // who already left — so tallies[slot] here would otherwise never be deleted, the OnGameFrame poll
    // would never go idle, and the stale/frozen HTML would keep being fireToClient'd every frame forever,
    // including to whichever future client is later assigned that same slot (Client is documented as NOT
    // serial/userId-gated). Subscribe here too so the renderer self-heals without depending on core
    // notifying it per-slot; fireToClient no-ops for an already-gone slot (GetLegacyGameEventListener
    // resolves null), matching the existing menu-close-on-disconnect precedent above.
    globalThis.__s2pkg_clients.Clients.onDisconnect(function (c) { if (tallies[c.slot]) clearTally(c.slot); });
  })();

  // pickPlayer(adminSlot, onPicked): a target-picker Center menu over connected players (the adminmenu
  // framework's shared player-picker). The item info is the userid (stable across the pick), re-resolved
  // via Player.fromUserId on select so a player who left in the meantime -> a graceful skip (never a stale
  // handle/pointer crossing the menu selection).
  function pickPlayer(adminSlot, onPicked) {
    var Menu = globalThis.__s2pkg_menu.Menu, MenuStyle = globalThis.__s2pkg_menu.MenuStyle;
    var m = new Menu("Select a player");
    m.style = MenuStyle.Center;
    m.freezePlayer = true;
    var players = Player.allConnected();
    for (var i = 0; i < players.length; i++) {
      var p = players[i];
      m.addItem(String(p.userId), (p.playerName || ("slot " + p.slot)));
    }
    m.onSelect(function (e) {
      var target = Player.fromUserId(parseInt(e.info, 10));
      if (!target) { globalThis.__s2pkg_chat.Chat.toSlot(adminSlot, "Player no longer available"); return; }
      onPicked(target);
    });
    m.display(adminSlot, 30);
  }

  // --- Beam: a CEnvBeam point-to-point line. CS2 schema names live HERE (never in core). Composes the
  //     engine-generic createEntity/spawn/teleport/remove primitive (@s2script/entity) + raw schema
  //     writes on the created ref. Offsets are re-resolved per call (never cached at module scope) —
  //     the same self-healing convention as the rest of this file.
  var RENDERMODE_TRANSALPHA = 4;   // RenderMode_t::kRenderTransAlpha (verify at the live gate)
  function beamPackRGBA(c) {
    return ((c[0] & 255) | ((c[1] & 255) << 8) | ((c[2] & 255) << 16) | ((c[3] & 255) << 24)) >>> 0;
  }
  function beamWriteEnd(ref, end) {
    var o = __s2_schema_offset("CBeam", "m_vecEndPos");
    if (o < 0) return false;
    var ok = ref.writeFloat32(o, end.x) && ref.writeFloat32(o + 4, end.y) && ref.writeFloat32(o + 8, end.z);
    if (ok) ref.notifyStateChanged(o);
    return !!ok;
  }
  var Beam = {
    // Draw a point-to-point beam (env_beam) from start to end. Returns a handle, or null if the entity
    // couldn't be created. The beam is game-world-owned (NOT auto-removed on plugin unload) — the caller
    // owns cleanup via handle.remove().
    draw: function (start, end, opts) {
      opts = opts || {};
      var ref = globalThis.__s2pkg_entity.createEntity("env_beam");
      if (!ref) return null;
      var rmOff = __s2_schema_offset("CBaseModelEntity", "m_nRenderMode");
      if (rmOff >= 0) ref.writeUInt8(rmOff, RENDERMODE_TRANSALPHA);
      var widthOff = __s2_schema_offset("CBeam", "m_fWidth");
      if (widthOff >= 0) ref.writeFloat32(widthOff, opts.width || 2.0);
      var colorOff = __s2_schema_offset("CBaseModelEntity", "m_clrRender");
      if (colorOff >= 0) ref.writeUInt32(colorOff, beamPackRGBA(opts.color || [255, 0, 0, 255]));
      beamWriteEnd(ref, end);
      ref.teleport([start.x, start.y, start.z]);   // start = the entity's own origin
      ref.spawn();
      return {
        ref: ref,
        update: function (s, e) { ref.teleport([s.x, s.y, s.z]); beamWriteEnd(ref, e); },
        remove: function () { return ref.remove(); }
      };
    }
  };

  // TriggerZone — a runtime trigger_multiple with a programmatic AABB (zones real-trigger backend).
  // create -> configure collision schema -> spawn -> teleport -> Enable/activateCollision -> setModel ->
  // Enable/activateCollision (the arbitrary-box recipe: the post-spawn setModel builds the physics
  // aggregate and activateCollision(=SetCollisionBounds+SetSolid(BBOX)) reshapes it to the box, so the
  // trigger fires OnStartTouch/OnEndTouch). Detection is the caller's (Entity.onOutput on those outputs).
  // Non-solid (players pass through). Game-world-owned; the caller owns remove().
  function collOffset(field) {
    var base = __s2_schema_offset("CBaseModelEntity", "m_Collision");   // embedded CCollisionProperty
    var rel  = __s2_schema_offset("CCollisionProperty", field);
    return (base >= 0 && rel >= 0) ? (base + rel) : -1;
  }
  function writeVecAt(ref, off, x, y, z) {
    if (off < 0) return false;
    var ok = ref.writeFloat32(off, +x) && ref.writeFloat32(off + 4, +y) && ref.writeFloat32(off + 8, +z);
    if (ok) ref.notifyStateChanged(off);
    return !!ok;
  }
  var TriggerZone = {
    // min/max = world-space corners ({x,y,z}). opts (optional): { model?, spawnflags? }.
    // The model is REQUIRED for the recipe to fire touch — any string works (SetModel builds an
    // error-model aggregate that SetSolid reshapes to the box); defaults to "models/error.vmdl".
    create: function (min, max, opts) {
      opts = opts || {};
      var ent = globalThis.__s2pkg_entity;
      var cx = (min.x + max.x) / 2, cy = (min.y + max.y) / 2, cz = (min.z + max.z) / 2;
      var hx = Math.abs(max.x - min.x) / 2, hy = Math.abs(max.y - min.y) / 2, hz = Math.abs(max.z - min.z) / 2;
      var sf = opts.spawnflags != null ? opts.spawnflags : 1;   // spawnflags (default 1 = clients)
      var SOLID_VPHYSICS = 6, COLLISION_GROUP_WEAPON = 14;      // players pass through weapons
      var model = opts.model || "models/error.vmdl";
      var ref = ent.createEntity("trigger_multiple");
      if (!ref) return null;
      // Clear EF_IN_STAGING_LIST(0x4) before DispatchSpawn: a staged entity spawns without proper touch
      // integration, so touch never fires. CEntityIdentity::m_flags via m_pEntity(@0x10) -> m_flags(@48).
      var preSpawnFlags = ref.readInt32Via([16], 48);
      if (preSpawnFlags !== null) ref.writeInt32Via([16], 48, preSpawnFlags & ~4);
      var sfOff = __s2_schema_offset("CBaseEntity", "m_spawnflags"); if (sfOff >= 0) { ref.writeUInt32(sfOff, sf >>> 0); ref.notifyStateChanged(sfOff); }
      var stOff = collOffset("m_nSolidType");    if (stOff >= 0) { ref.writeUInt8(stOff, SOLID_VPHYSICS); ref.notifyStateChanged(stOff); }
      var fsOff = collOffset("m_usSolidFlags");   if (fsOff >= 0) { ref.writeUInt8(fsOff, 0); ref.notifyStateChanged(fsOff); }
      var cgOff = collOffset("m_CollisionGroup"); if (cgOff >= 0) { ref.writeUInt8(cgOff, COLLISION_GROUP_WEAPON); ref.notifyStateChanged(cgOff); }
      // m_vecMins/Maxs are OBB bounds RELATIVE TO ORIGIN — with the origin teleported to the box CENTER,
      // the bounds must be LOCAL ±half (giving world bounds center±half).
      writeVecAt(ref, collOffset("m_vecMins"), -hx, -hy, -hz);
      writeVecAt(ref, collOffset("m_vecMaxs"),  hx,  hy,  hz);
      var dOff = __s2_schema_offset("CBaseTrigger", "m_bDisabled"); if (dOff >= 0) { ref.writeBool(dOff, false); ref.notifyStateChanged(dOff); }
      ref.spawn();                     // DispatchSpawn
      ref.teleport([cx, cy, cz]);      // then teleport to the box center
      ref.acceptInput("Enable");       // arm the trigger
      ref.activateCollision();         // register in the spatial partition
      // The post-spawn SetModel builds the physics aggregate (partition registration alone never fires
      // touch); re-Enable + re-activate after, exactly like the proven path. Do NOT write solid/bounds
      // after SetModel — any such write destroys the model aggregate and touch stops firing.
      ref.setModel(model);
      ref.acceptInput("Enable");
      ref.activateCollision();
      return { ref: ref, center: { x: cx, y: cy, z: cz }, remove: function () { return ref.remove(); } };
    }
  };

  // GameRules — read CCSGameRules via the cs_gamerules proxy's m_pGameRules pointer.
  // Serial-gated at the proxy root (readVia); offsets live-resolved per access (self-healing across map
  // changes — the proxy dies and re-resolves). All getters read null if the proxy is gone.
  function GameRulesView(proxyRef) { this.ref = proxyRef; }
  function grPath() { var o = __s2_schema_offset("CCSGameRulesProxy", "m_pGameRules"); return o < 0 ? null : [o]; }
  function grBool(field)  { return { get: function () { var p = grPath(); if (!p) return null; var o = __s2_schema_offset("CCSGameRules", field); return o < 0 ? null : this.ref.readBoolVia(p, o); } }; }
  function grInt(field)   { return { get: function () { var p = grPath(); if (!p) return null; var o = __s2_schema_offset("CCSGameRules", field); return o < 0 ? null : this.ref.readInt32Via(p, o); } }; }
  function grFloat(field) { return { get: function () { var p = grPath(); if (!p) return null; var o = __s2_schema_offset("CCSGameRules", field); return o < 0 ? null : this.ref.readFloat32Via(p, o); } }; }
  Object.defineProperties(GameRulesView.prototype, {
    warmupPeriod:          grBool("m_bWarmupPeriod"),
    freezePeriod:          grBool("m_bFreezePeriod"),
    roundTime:             grInt("m_iRoundTime"),
    freezeTime:            grInt("m_iFreezeTime"),
    totalRoundsPlayed:     grInt("m_totalRoundsPlayed"),
    gamePhase:             grInt("m_gamePhase"),
    bombPlanted:           grBool("m_bBombPlanted"),
    roundsPlayedThisPhase: grInt("m_nRoundsPlayedThisPhase"),
    gameRestart:           grBool("m_bGameRestart"),
    gameStartTime:         grFloat("m_flGameStartTime"),
    matchWaitingForResume: grBool("m_bMatchWaitingForResume"),
    hasMatchStarted:       grBool("m_bHasMatchStarted")
  });
  var GameRules = {
    get: function () {
      var ent = globalThis.__s2pkg_entity;
      var refs = ent && ent.Entity ? ent.Entity.findByClass("cs_gamerules") : null;
      if (!refs || refs.length === 0) return null;
      return new GameRulesView(refs[0]);
    }
  };

  // CS2 user-message sugar over the generic @s2script/usermessages builder.
  var FFADE_IN = 1, FFADE_OUT = 2, FFADE_MODULATE = 4, FFADE_STAYOUT = 8, FFADE_PURGE = 16;
  function _um(name) { return new (globalThis.__s2pkg_usermessages.UserMessage)(name); }
  var Fade = {
    // opts: { duration, holdTime?, color?, flags? }. duration/holdTime are engine fade units
    // (tuned at the human visual test); color is a packed RGBA fixed32 (default opaque black).
    to: function (slot, opts) {
      opts = opts || {};
      return _um("CUserMessageFade")
        .setInt("duration",  opts.duration  != null ? opts.duration  : 1024)
        .setInt("hold_time", opts.holdTime  != null ? opts.holdTime  : 0)
        .setInt("flags",     opts.flags     != null ? opts.flags     : (FFADE_OUT | FFADE_PURGE))
        .setInt("color",     opts.color     != null ? opts.color     : 0xFF000000)
        .send(slot);
    },
    blind: function (slot, duration) {
      var d = duration != null ? duration : 2000;
      return Fade.to(slot, { duration: d, holdTime: d, flags: FFADE_OUT | FFADE_PURGE, color: 0xFF000000 });
    }
  };
  var Shake = {
    // opts: { amplitude, frequency, duration }. command 0 = start.
    to: function (slot, opts) {
      opts = opts || {};
      return _um("CUserMessageShake")
        .setInt("command",     opts.command   != null ? opts.command   : 0)
        .setFloat("amplitude", opts.amplitude != null ? opts.amplitude : 10.0)
        .setFloat("frequency", opts.frequency != null ? opts.frequency : 1.5)
        .setFloat("duration",  opts.duration  != null ? opts.duration  : 1.0)
        .send(slot);
    }
  };
  // HintText: best-effort. The exact scalar CS2 hint message resolves during the shim/live gate;
  // if a clean TextMsg-family message isn't available this is a no-op-returning send (Fade + Shake
  // are the load-bearing sugar). Field wiring is confirmed/tuned at the live gate.
  var HintText = {
    to: function (slot, text) {
      var m = _um("CUserMessageTextMsg");
      return m.setInt("dest", 4 /* HUD_PRINTCENTER-ish; tuned live */).setString("param", String(text)).send(slot);
    }
  };

  // A small curated set of known-good BUILT-IN CS2 soundevents (convenience + the sound-demo).
  // CS2 soundevent names live exclusively HERE (the game layer), never in core/src. The audible
  // verify is a human-client test (bots have no audio) — tune/extend these names at that gate.
  var Sounds = {
    Ping:       "UI.PlayerPing",
    PingUrgent: "UI.PlayerPingUrgent",
    Ak47Shot:   "Weapon_AK47.Single",
    DeagleShot: "Weapon_DEagle.Single",
  };

  // Merge (not overwrite) — csitem.generated.js (and any other prelude concatenated
  // ahead of this IIFE) may have already populated globalThis.__s2pkg_cs2 (e.g. CsItem).
  globalThis.__s2pkg_cs2 = Object.assign({}, globalThis.__s2pkg_cs2, { Pawn: Pawn, Player: Player, Events: (__s2require("@s2script/events") || {}).Events, ChatColors: ChatColors, Activity: Activity, pickPlayer: pickPlayer, Beam: Beam, GameRules: GameRules, Fade: Fade, Shake: Shake, HintText: HintText, TriggerZone: TriggerZone, Sounds: Sounds });
})();
