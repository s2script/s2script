// @s2script/cs2 — the injected game package. CS2 identifiers live ONLY in this file (never in core).
// The generated field accessors (schema.generated.js) run BEFORE this file (concatenated ahead of it by
// scripts/package-addon.sh) and set globalThis.__s2pkg_cs2_schema; this file applies the generated
// CCSPlayerPawn accessors to Pawn.prototype and keeps the behavioral entry point (Pawn.forSlot).
// Offsets are resolved live (Slice 3) and cached by the core OffsetCache; nothing is baked.
(function () {
  var EntityRef = __s2require("@s2script/sdk/entity").EntityRef;
  var schema = globalThis.__s2pkg_cs2_schema;   // set by schema.generated.js
  var Weapon = globalThis.__s2pkg_cs2.Weapon;   // set by weapon.js (concatenated before this file)

  // --- A5b: engine calls this game package DECLARES (gamedata/cs2/game.cs2.jsonc `calls`) ---
  // The descriptors are registered by the HOST at Load — the shim hands core gamedata/cs2's merged
  // view — under the game package's own reserved owner id. This file never declares one and a
  // plugin cannot: the natives below take a call NAME only, exactly like @s2script/sdk/unsafe's
  // Engine.call, and core supplies the owner. `engineCall(name)` therefore guards ONCE and hands
  // back a plain callable or null, so a descriptor that failed a load-time gate degrades to a
  // no-op the caller can test for, with engineCallStatus(name) naming why.
  //
  // Older core: the natives are absent, every call reports unavailable, and nothing throws.
  //
  // EVERY RULE THE DESCRIPTORS CANNOT EXPRESS IS BELOW, AND EVERY ONE OF THEM IS PINNED. The bounds,
  // the hardcoded arguments, the two drains and their dedupe/consume-before-call/re-check
  // properties, the SwitchTeam->ChangeTeam route and the two null guards a descriptor inverts are
  // tested against the REAL shipped bundle in packages/sdk/test/cs2-engine-calls.test.mjs (offline,
  // over recording fakes — no engine required). These used to be C++/Rust rules with core-side
  // tests; the ops are gone, so that file is where they live now. Change one here, change it there.
  function engineCallStatus(name) {
    return typeof __s2_game_call_status === "function"
      ? __s2_game_call_status(name)
      : "this host is too old to declare game-package engine calls";
  }
  function engineCall(name) {
    if (typeof __s2_game_call_ready !== "function" || !__s2_game_call_ready(name)) return null;
    // A receiverless descriptor (a static engine function) takes NO leading `self` — shifting one
    // off would silently eat the first real argument.
    if (__s2_game_call_receiverless(name)) {
      return function () {
        return __s2_game_call_invoke(name, 0, 0, Array.prototype.slice.call(arguments));
      };
    }
    return function () {
      var args = Array.prototype.slice.call(arguments);
      var self = args.shift();
      // The receiver is an EntityRef, or anything backed by one (Pawn / Player / Weapon all carry
      // `.ref`). Nothing else is accepted: the (index, id) pair is what core gates against the
      // host's books, and a raw pointer never crosses.
      //
      // ASYMMETRY WORTH KNOWING: only the RECEIVER is unwrapped this way. A declared `entity`
      // ARGUMENT is read by core straight off the value's own `.index`/`.id`, so it must be an
      // EntityRef — pass `pawn.ref`, not `pawn`. A wrapper object there silently packs as
      // "no entity" and the engine gets a nullptr.
      var ref = self && (self.ref || self);
      if (!ref || typeof ref.index !== "number") return null;   // no receiver -> no-op
      return __s2_game_call_invoke(name, ref.index, ref.id, args);
    };
  }
  // The eleven descriptors gamedata/cs2/game.cs2.jsonc declares, resolved ONCE here. Resolution is a
  // load-time fact — the shim resolves + validates every descriptor at Load and never retries — so a
  // per-call `engineCall()` would re-derive the same answer on every slay(). Each is a plain callable
  // or `null`; the typed wrappers below test for null and degrade to their documented no-op/false,
  // and `__s2pkg_cs2_calls.status(name)` names why any of them is null on a live server.
  var callCommitSuicide    = engineCall("commitSuicide");
  var callChangeTeam       = engineCall("changeTeam");
  var callSwitchTeam       = engineCall("switchTeam");
  var callTerminateRound   = engineCall("terminateRound");
  var callRespawn          = engineCall("respawn");
  var callSetPawn          = engineCall("setPawn");
  var callGiveNamedItem    = engineCall("giveNamedItem");
  var callRemovePlayerItem = engineCall("removePlayerItem");
  var callGetPlayerMaxSpeed = engineCall("getPlayerMaxSpeed");

  // Internal namespace, same idiom as __s2pkg_cs2_schema / __s2pkg_cs2_nav: NOT part of the
  // @s2script/cs2 public surface. `status` is here so an operator can ask a live server WHY a game
  // call is unavailable, and `call` so a future descriptor needs no new plumbing.
  globalThis.__s2pkg_cs2_calls = {
    call: engineCall,
    status: engineCallStatus,
    // weapon.js is concatenated BEFORE this file, so it cannot capture a callable at evaluation
    // time. It reads this one at CALL time instead, which is why the descriptor is published here.
    removePlayerItem: callRemovePlayerItem,
  };

  function warn(msg) { if (globalThis.console) console.log("[s2script] " + msg); }

  function Pawn(ref) { this.ref = ref; }
  if (schema) schema.applyAccessors(Pawn.prototype, "CCSPlayerPawn");   // health, friction, controller, ...
  var nav = globalThis.__s2pkg_cs2_nav;   // set by nav.generated.js (concatenated ahead of pawn.js)
  if (nav) nav.applyNav(Pawn.prototype, "CCSPlayerPawn");   // sceneNode, weaponServices, movementServices, aimPunchServices

  // --- Slice 5C.2: the Player (controller) model ---
  function Player(ref) { this.ref = ref; }                       // ref = the CONTROLLER EntityRef
  if (schema) schema.applyAccessors(Player.prototype, "CCSPlayerController");  // team, score, ping, ...
  // Controller-sourced pointer-chain wrappers (matchStats). Separate call from the pawn's: applyNav
  // keys on the SOURCE class, so a controller target is invisible unless the controller proto asks.
  if (nav) nav.applyNav(Player.prototype, "CCSPlayerController");

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
    var ref = new EntityRef(idx, __s2_ent_id_for_index(idx));
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
    var ref = new EntityRef(idx, __s2_ent_id_for_index(idx));
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
  // self-resolved CCSPlayerController::ChangeTeam (the `changeTeam` descriptor; serial-gated by the
  // receiver, so a stale ref is a no-op). The 0..3 bound (Unassigned/Spectator/T/CT) is enforced HERE:
  // it used to live in the shim op, and a `calls` descriptor has no way to reject an argument.
  Player.prototype.changeTeam = function (team) {
    var t = team | 0;
    if (t < 0 || t > 3) return;               // Unassigned/Spectator/T/CT only
    if (!callChangeTeam) return;
    callChangeTeam(this, t);
  };
  // player.spectate() — move this player to the Spectator team (SM parity; = changeTeam(1)).
  Player.prototype.spectate = function () { this.changeTeam(1); };

  // player.switchTeam(team) — NON-LETHAL move between T(2)/CT(3) via the self-resolved
  // CCSPlayerController::SwitchTeam: the player stays alive and keeps weapons (vs changeTeam =
  // jointeam semantics). The engine MAY respawn the pawn — re-resolve player.pawn next frame before
  // pawn writes.
  //
  // ORDER MATTERS, and it is the shim op's order verbatim: bound FIRST, then route, then this
  // descriptor's own readiness. team <= 1 (None/Spectator) goes through ChangeTeam because the engine
  // SwitchTeam is CS:GO-lineage T/CT-only (CSSharp/SwiftlyS2 parity) — and routing BEFORE the
  // readiness test is what keeps switchTeam(1) working when SwitchTeam is degraded but ChangeTeam is
  // not. No `calls` construct expresses a dispatch between two descriptors; this is why it is code.
  Player.prototype.switchTeam = function (team) {
    var t = team | 0;
    if (t < 0 || t > 3) return;               // Unassigned/Spectator/T/CT only
    if (t <= 1) { this.changeTeam(t); return; }
    if (!callSwitchTeam) return;
    callSwitchTeam(this, t);
  };

  // player.respawn() — SetPawn then Respawn, same frame (SwiftlyS2/CSSharp sequence). Synchronous:
  // player_spawn runs other plugins before this returns. A player_spawn handler that respawns again
  // no-ops on pawnIsAlive. Same-hook skip covers a hook that re-enters itself.
  Player.prototype.respawn = function () {
    if (this.pawnIsAlive === true) return false;
    if (!callRespawn || !callSetPawn) return false;
    if (!this.ref.isValid()) return false;
    var pawn = this.pawn;
    if (pawn) callSetPawn(this, pawn.ref, true, false);
    callRespawn(this);
    return true;
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

  // pawn.isValid — SourceMod/CSSharp-sense validity: live per the HOST'S BOOKS (+ slot
  // check) AND fully spawned (out of the engine EF_IN_STAGING_LIST). The staging bit is
  // read from the identity SLOT via ref.identityFlags() — the pre-E1 readInt32Via([16],48)
  // instance chain (the changelevel-UAF crash site) is gone. If the flags read is
  // unavailable (null), fall back to liveness (do not over-block a live pawn).
  Object.defineProperty(Pawn.prototype, "isValid", {
    get: function () {
      if (!this.ref.isValid()) return false;
      var flags = this.ref.identityFlags();
      return flags === null ? true : (flags & 4) === 0;   // EF_IN_STAGING_LIST = 1<<2 (CS2 fact)
    },
    enumerable: true, configurable: true,
  });

  // pawn.slay() — kill this pawn via the self-resolved CBasePlayerPawn::CommitSuicide (the
  // `commitSuicide` descriptor; the receiver is serial-gated, so a stale pawn is a no-op).
  //
  // bForce = TRUE is load-bearing, not a default: the function body is an m_fNextSuicideTime rate
  // limiter with a `test bForce; je ret`, so passing false makes slay() a SILENT no-op for any player
  // who died recently. bExplode = false is the plain (non-gib) kill. Neither is caller-supplied.
  Pawn.prototype.slay = function () {
    if (!callCommitSuicide) return;
    callCommitSuicide(this, /*bExplode=*/false, /*bForce=*/true);
  };

  // pawn.maxSpeed — the pawn's CURRENT movement speed cap, via CCSPlayerPawn::GetPlayerMaxSpeed (the
  // `getPlayerMaxSpeed` descriptor; the receiver is serial-gated, so a stale pawn reads null).
  //
  // A getter rather than a field because the engine COMPUTES this — there is no m_flMaxSpeed on
  // CCSPlayerPawn in the schema, so there is nothing to read. `null` (not 0) when the descriptor
  // failed its load-time gate or the ref is stale: 0 is a legitimate speed (a frozen player), so it
  // must not double as "unavailable". engineCallStatus("getPlayerMaxSpeed") names the reason.
  Object.defineProperty(Pawn.prototype, "maxSpeed", {
    get: function () {
      if (!callGetPlayerMaxSpeed) return null;
      var v = callGetPlayerMaxSpeed(this);
      return typeof v === "number" ? v : null;
    }
  });

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
  // dropActiveWeapon/removeWeapon — over the `giveNamedItem`/`removePlayerItem` descriptors declared
  // in gamedata/cs2/game.cs2.jsonc (A5b; they were bespoke shim ops before) plus the Task-1
  // __s2_entity_subobj_vcall op and EntityRef.readHandleVector.
  // Offsets are live-resolved via __s2_schema_offset (never baked, self-healing); __s2_schema_offset
  // walks the base-class chain (schema_find_field in the shim), so passing "CCSPlayer_WeaponServices"
  // still resolves m_hMyWeapons even though it's declared on the base CPlayer_WeaponServices.

  // pawn.giveNamedItem(name) — give this pawn a weapon/item by classname (CsItem.AK47 or a raw "weapon_*"
  // string). Returns the created Weapon, or null if the ItemServices ptr is unresolved / failed / stale.
  //
  // The m_pItemServices hop is the descriptor's `receiver.via` now, so it is live-resolved per invoke
  // by the same cached schema resolver this file used to call directly — an unresolved offset degrades
  // THIS invocation to null and names itself in __s2pkg_cs2_calls.status("giveNamedItem"). The four
  // trailing args (iSubType, pScriptItem, a5, a6) are hardcoded 0 exactly as the shim op passed them
  // and are deliberately not exposed. The returned handle comes back through the books-gated adopt
  // path, so a raw engine pointer can never mint an EntityRef.
  Pawn.prototype.giveNamedItem = function (name) {
    if (!callGiveNamedItem || name == null) return null;
    var ref = callGiveNamedItem(this, String(name), 0, 0, 0, 0);
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

  // Entity ops forwarded from the pawn's serial-gated EntityRef. Thin by design: the engine ops are
  // entity-generic (see @s2script/sdk/entity) and these exist because a pawn is what a plugin
  // actually holds — `pawn.ref.setGravityScale(x)` stays equivalent.
  //
  // Guarded on the METHOD, not on `this.ref`. A Pawn is always constructed with a ref
  // (`function Pawn(ref) { this.ref = ref; }`), so a ref check would be dead code; what can actually
  // be missing is the method itself on an older core, the same skew emitSound guards for via
  // __s2pkg_sound. Staleness needs no guard here — the EntityRef method is serial-gated and returns
  // false on a dead ref, which is precisely the behaviour to pass through.
  //
  // setBodyGroupByName is deliberately NOT forwarded: a model concern rather than a player one. It
  // stays reachable as pawn.ref.setBodyGroupByName(...).
  function forwardToRef(method) {
    return function () {
      var r = this.ref, f = r && r[method];
      return typeof f === "function" ? f.apply(r, arguments) : false;
    };
  }
  // pawn.stopSound(name) — the counterpart to emitSound. No recipients/volume: the engine stops the
  // sound for everyone hearing it.
  Pawn.prototype.stopSound = forwardToRef("stopSound");
  Pawn.prototype.setGravityScale = forwardToRef("setGravityScale");
  Pawn.prototype.applyAbsVelocityImpulse = forwardToRef("applyAbsVelocityImpulse");
  Pawn.prototype.setModelScale = forwardToRef("setModelScale");

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
    var Server = __s2require("@s2script/sdk/server").Server;
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
    var ctrl = new EntityRef(ctrlIndex, __s2_ent_id_for_index(ctrlIndex));
    if (!ctrl.isValid()) return null;
    var handle = ctrl.readInt32(PAWN_HANDLE);
    if (handle === null) return null;
    var d = __s2_handle_adopt(handle >>> 0);
    if (!d) return null;                       // dangling m_hPlayerPawn can never mint a live ref
    var pawn = new EntityRef(d[0], d[1]);
    return pawn.isValid() ? new Pawn(pawn) : null;
  };

  // CS2 chat color control bytes (values from CounterStrikeSharp's ChatColors enum). The PLUGIN composes
  // colored messages with these (SourceMod-parity — color is content, not a native-layer default); just
  // put one at the front of the message (`Green + "hi"`). No leading space needed — @s2script/chat prefixes
  // every line with an invisible zero-width space so the first color byte is never swallowed. Frozen so
  // consumers can't mutate.
  var ChatColors = Object.freeze({
    Default: "\x01", White: "\x01", DarkRed: "\x02", LightPurple: "\x03", Green: "\x04", Olive: "\x05",
    Lime: "\x06", Red: "\x07", Grey: "\x08", Yellow: "\x09", Silver: "\x0A", Blue: "\x0B", DarkBlue: "\x0C",
    BlueGrey: "\x0D", Purple: "\x0E", LightRed: "\x0F", Orange: "\x10"
  });

  // Hand the tag table to core's expander (core/js/colors.js). This is the ONLY direction game
  // colour knowledge may travel: core receives a map, it never holds one. setTable lowercases the
  // keys, so `Green` above is reachable as `{green}` in any phrase file an operator edits.
  // Guarded for an older core that predates the expander.
  if (globalThis.__s2_colors && typeof globalThis.__s2_colors.setTable === "function") {
    globalThis.__s2_colors.setTable(ChatColors);
  }

  // --- Activity.formatSource: SourceMod FormatActivitySource port ---
  // activity.js (concatenated ahead of pawn.js) sets globalThis.__s2_activity = { computeActivitySource, SHOW_ACTIVITY_DEFAULT }.
  var __act = globalThis.__s2_activity;
  // Resolve @s2script/admin lazily + memoized: at IIFE-init the admin prelude may not be registered
  // yet, so eager resolution could abort CS2 module init; by formatSource call-time it is always present.
  var __adminMod = null;
  function __resolveAdmin() {
    if (__adminMod === null) __adminMod = __s2require("@s2script/sdk/admin") || {};
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

  // --- CS2 Menu renderer ---
  // The WASD / show_survival_respawn_status center HTML renderer used to live here. CS2 now paints
  // both MenuStyle.Center and MenuStyle.Chat through the hudkit sheet in menuhud.js (concatenated
  // after components.js). Freeze/cursor/Tab live with that renderer. pickPlayer stays a Menu.

  // --- CS2 vote rail ---
  // The live HTML tally (show_survival_respawn_status) used to live here. CS2 now paints votes as
  // a right-side rail on the shared s2script_lib.xml layout (voterail.js, concatenated after
  // menuhud.js). Same addon / same entity as menus — not a second CustomHudLayout.

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
  // trigger fires OnStartTouch/OnEndTouch). Detection is the caller's (`onOutput` on those outputs).
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
    hasMatchStarted:       grBool("m_bHasMatchStarted"),
    // Round-control slice: m_fRoundStartTime (GameTime_t read as f32 — validated live: ~= gameTime at
    // round_start). timeElapsed/timeRemaining track ENGINE TRUTH: the engine ends the round at
    // roundStartTime + m_iRoundTime (freeze is the first freezeTime seconds of that span, NOT a
    // separate subtraction). An earlier draft mirrored TTT's GetTimeElapsed (which subtracts
    // freezeTime) — live-gate-caught: that made the HUD/round-end fire freezeTime (~15s) early, so
    // setTimeRemaining(30) ended the round in 15s. freezeTime is exposed as its own read, not folded in.
    roundStartTime:        grFloat("m_fRoundStartTime"),
    timeElapsed: { get: function () {
      var st = this.roundStartTime;
      var srv = globalThis.__s2pkg_server;
      var now = srv && srv.Server ? srv.Server.gameTime : null;
      if (st === null || now === null || now === 0) return null;
      return now - st;
    } },
    timeRemaining: { get: function () {
      var rt = this.roundTime, el = this.timeElapsed;
      if (rt === null || el === null) return null;
      return rt - el;
    } }
  });

  // Write m_iRoundTime through the proxy's m_pGameRules chain, then dirty the PROXY at the
  // m_pGameRules offset (a FLAT offset on the proxy root — the TTT/CSSharp
  // SetStateChanged(proxy, "CCSGameRulesProxy", "m_pGameRules") pattern) so the change renetworks.
  // writeInt32Via deliberately does NOT auto-notify; forgetting the notify means the HUD clock never
  // repaints on clients (the live-gate criterion).
  GameRulesView.prototype.setRoundTime = function (seconds) {
    var p = grPath(); if (!p) return false;
    var o = __s2_schema_offset("CCSGameRules", "m_iRoundTime"); if (o < 0) return false;
    if (!this.ref.writeInt32Via(p, o, seconds | 0)) return false;
    this.ref.notifyStateChanged(p[0]);
    return true;
  };
  // TTT SetTimeRemaining: roundTime = elapsed + seconds.
  GameRulesView.prototype.setTimeRemaining = function (seconds) {
    var el = this.timeElapsed; if (el === null) return false;
    return this.setRoundTime(Math.ceil(el + seconds));
  };
  // TTT AddTimeRemaining: roundTime += delta.
  GameRulesView.prototype.addTimeRemaining = function (seconds) {
    var rt = this.roundTime; if (rt === null) return false;
    return this.setRoundTime(rt + Math.ceil(seconds));
  };
  // Force the round to end (CCSGameRules::TerminateRound). Synchronous: round_end reaches other
  // plugins before this returns. onTerminateRound is still bypassed (SourceMod blockhook).
  GameRulesView.prototype.terminateRound = function (reason, delay) {
    var r = reason | 0;
    if (r < 0 || r > 22) { warn("terminate_round: reason " + r + " out of range 0..22 — rejected"); return false; }
    if (!callTerminateRound) return false;
    if (!grPath()) return false;
    if (!this.ref.isValid()) return false;
    var d = (delay === undefined || delay === null) ? 5.0 : +delay;
    callTerminateRound(this, d, r, 0, 0);
    return true;
  };
  var GameRules = (function () {
    // Cache the resolved cs_gamerules proxy (ModSharp/Swiftly cache the gamerules pointer likewise).
    // Serial-gated: on a map change the proxy dies -> isValid() false -> re-scan. Turns get() from an
    // O(N) findByClass scan (~19us) into a single serial check on the hot path (~0.4us).
    var cachedRef = null, cachedView = null;
    return {
      get: function () {
        if (cachedRef && cachedRef.isValid()) return cachedView;
        var ent = globalThis.__s2pkg_entity;
        var refs = ent && ent.Entity ? ent.Entity.findByClass("cs_gamerules") : null;
        if (!refs || refs.length === 0) { cachedRef = null; cachedView = null; return null; }
        cachedRef = refs[0];
        cachedView = new GameRulesView(cachedRef);
        return cachedView;
      }
      ,
      terminateRound: function (reason, delay) {
        var v = this.get();
        return v ? v.terminateRound(reason, delay) : false;
      }
    };
  })();

  // Team scoreboard scores — cs_team_manager entities (≈4: Unassigned/Spectator/T/CT) matched by
  // m_iTeamNum (NEVER by enumeration order), CTeam.m_iScore written flat + notifyStateChanged at the
  // SAME offset (the TTT SetStateChanged(entry, "CTeam", "m_iScore") pattern). Entities are re-found
  // per call (cold path) — deliberately NO cache: team entities die on map change, and TTT's own
  // `_teamManager ??=` cache is a bug we do not replicate.
  var Teams = {
    _find: function (team) {
      var ent = globalThis.__s2pkg_entity;
      if (!ent || !ent.Entity) return null;
      var tno = __s2_schema_offset("CBaseEntity", "m_iTeamNum"); if (tno < 0) return null;
      var refs = ent.Entity.findByClass("cs_team_manager") || [];
      for (var i = 0; i < refs.length; i++) {
        if (refs[i].readUInt8(tno) === (team | 0)) return refs[i];
      }
      return null;
    },
    getScore: function (team) {
      var ref = Teams._find(team); if (!ref) return null;
      var o = __s2_schema_offset("CTeam", "m_iScore"); if (o < 0) return null;
      return ref.readInt32(o);
    },
    setScore: function (team, score) {
      var ref = Teams._find(team); if (!ref) return false;
      var o = __s2_schema_offset("CTeam", "m_iScore"); if (o < 0) return false;
      if (!ref.writeInt32(o, score | 0)) return false;
      ref.notifyStateChanged(o);
      return true;
    },
    addScore: function (team, delta) {
      var cur = Teams.getScore(team); if (cur === null) return false;
      return Teams.setScore(team, cur + (delta | 0));
    }
  };

  // CS2 round-end reasons ("layout is data, semantics are code" — a name<->number mapping is reviewed
  // code). Values HINTed by the CSSharp enum and BINARY-VALIDATED against our build: the engine's
  // `cmp $0x16` bound (max 22 = SurvivalDraw) + every #SFUI_Notice_* switch string present. Gaps
  // 2/3/15 are removed legacy VIP reasons. Closed-loop re-validated at the live gate (terminateRound
  // reason vs the engine-emitted round_end.reason).
  var RoundEndReason = {
    Unknown: 0, TargetBombed: 1, TerroristsEscaped: 4, CTsPreventEscape: 5,
    EscapingTerroristsNeutralized: 6, BombDefused: 7, CTsWin: 8, TerroristsWin: 9,
    RoundDraw: 10, AllHostagesRescued: 11, TargetSaved: 12, HostagesNotRescued: 13,
    TerroristsNotEscaped: 14, GameCommencing: 16, TerroristsSurrender: 17, CTsSurrender: 18,
    TerroristsPlanted: 19, CTsReachedHostage: 20, SurvivalWin: 21, SurvivalDraw: 22
  };
  // cs_win_panel_round final_event values (HINT: TTT/CSSharp usage; validated at the live gate
  // against a natural round end's engine-emitted value).
  var WinPanelFinalEvent = { CTsWin: 2, TerroristsWin: 3 };

  // CS2 user-message sugar over the generic @s2script/usermessages builder.
  // The engine's fade-flag vocabulary, complete. An object rather than five loose consts so the
  // unused members are documentation instead of dead bindings a linter has to be told to ignore —
  // `opts.flags` is a raw number, and this is where a caller finds out what to pass.
  //
  // HONEST TRADE-OFF: this costs typo detection. `no-undef` catches `FFADE_OUTT` on a loose const
  // and CANNOT catch `FFADE.OUTT` on a property. Accepted because the alternative — deleting the
  // three unused names — leaves a partial flag list that is worse documentation than no list, and
  // the object is used exactly twice, both here, where a typo fails visibly at the first fade.
  var FFADE = { IN: 1, OUT: 2, MODULATE: 4, STAYOUT: 8, PURGE: 16 };
  function _um(name) { return new (globalThis.__s2pkg_usermessages.UserMessage)(name); }
  var Fade = {
    // opts: { duration, holdTime?, color?, flags? }. duration/holdTime are engine fade units
    // (tuned at the human visual test); color is a packed RGBA fixed32 (default opaque black).
    to: function (slot, opts) {
      opts = opts || {};
      return _um("CUserMessageFade")
        .setInt("duration",  opts.duration  != null ? opts.duration  : 1024)
        .setInt("hold_time", opts.holdTime  != null ? opts.holdTime  : 0)
        .setInt("flags",     opts.flags     != null ? opts.flags     : (FFADE.OUT | FFADE.PURGE))
        .setInt("color",     opts.color     != null ? opts.color     : 0xFF000000)
        .send(slot);
    },
    blind: function (slot, duration) {
      var d = duration != null ? duration : 2000;
      return Fade.to(slot, { duration: d, holdTime: d, flags: FFADE.OUT | FFADE.PURGE, color: 0xFF000000 });
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
  globalThis.__s2pkg_cs2 = Object.assign({}, globalThis.__s2pkg_cs2, { Pawn: Pawn, Player: Player, Events: (__s2require("@s2script/sdk/events") || {}).Events, ChatColors: ChatColors, Activity: Activity, pickPlayer: pickPlayer, Beam: Beam, GameRules: GameRules, Teams: Teams, RoundEndReason: RoundEndReason, WinPanelFinalEvent: WinPanelFinalEvent, Fade: Fade, Shake: Shake, HintText: HintText, TriggerZone: TriggerZone, Sounds: Sounds });

  // The ctx namespaces this game package contributes (core's generic extension point in
  // prelude.js merges these into every plugin's ctx). Each factory gets the prelude's ledger
  // registrar, so ctx.gameRules.onTerminateRound is torn down at unload like any other
  // subscription. The hook NAMES here must match the `hooks` keys declared for "@s2script/cs2"
  // in gamedata/cs2/game.cs2.jsonc — degrade is graceful (a WARN, never a crash) if they drift,
  // but the handler simply never fires.
  globalThis.__s2pkg_game_ctx = {
    gameRules: function (reg, viaId) {
      return {
        onTerminateRound: function (h) {
          reg(viaId(function () { return __s2_hook_on("@s2script/cs2", "onTerminateRound", h); }));
        },
      };
    },
    players: function (reg, viaId) {
      return {
        onRespawn: function (h) {
          reg(viaId(function () { return __s2_hook_on("@s2script/cs2", "onRespawn", h); }));
        },
      };
    },
    items: function (reg, viaId) {
      var playerHopWarned = false;
      function hopPlayer() {
        if (typeof __s2_hook_self_matches !== "function") return null;
        var off = __s2_schema_offset("CBasePlayerPawn", "m_pItemServices");
        if (off < 0) return null;
        for (var s = 0; s < MAX_PLAYERS; s++) {
          var pawn = Pawn.forSlot(s);
          if (pawn && pawn.ref && __s2_hook_self_matches(pawn.ref, off)) {
            return pawn.controller;
          }
        }
        if (!playerHopWarned) {
          playerHopWarned = true;
          console.log("[s2script] WARN: onCanAcquire player hop missed (ItemServices* matched no live pawn) — view.player is null; the hook still fires");
        }
        return null;
      }
      function readDefIndex() {
        if (typeof __s2_hook_q_u16 !== "function") return 0;
        var n = __s2_hook_q_u16(0, "CEconItemView", "m_iItemDefinitionIndex");
        return typeof n === "number" ? n : 0;
      }
      function wrap(h, isPost) {
        return function (raw) {
          var view = {
            get player() { return hopPlayer(); },
            get defIndex() { return readDefIndex(); },
            get method() { return raw.method; },
            get result() { return raw.result; },
            set result(v) { if (!isPost) raw.result = v; },
            get skipped() { return isPost ? !!raw.skipped : false; },
          };
          return h(view);
        };
      }
      return {
        onCanAcquire: function (h) {
          reg(viaId(function () { return __s2_hook_on("@s2script/cs2", "onCanAcquire", wrap(h, false)); }));
        },
        onCanAcquirePost: function (h) {
          reg(viaId(function () {
            if (typeof __s2_hook_on_post === "function") {
              return __s2_hook_on_post("@s2script/cs2", "onCanAcquire", wrap(h, true));
            }
            return 0;
          }));
        },
      };
    },
  };

  // Load-window free APIs: import { gameRules, players, items } from "@s2script/cs2".
  // Proxies look up the current plugin's merged ctx at call time (same window as command()).
  if (typeof globalThis.__s2_game_ns === "function") {
    globalThis.__s2pkg_cs2.gameRules = globalThis.__s2_game_ns("gameRules");
    globalThis.__s2pkg_cs2.players = globalThis.__s2_game_ns("players");
    globalThis.__s2pkg_cs2.items = globalThis.__s2_game_ns("items");
  }

  // Crash reporter: push the game identity into the engine-generic breadcrumb (spec §5 — the
  // game package supplies the value IN; core never knows the game). Best-effort: absent natives
  // (an older core) degrade silently.
  if (typeof __s2_crash_set_game === "function" && typeof __s2_server_build === "function") {
    __s2_crash_set_game("cs2", __s2_server_build());
  }
})();
