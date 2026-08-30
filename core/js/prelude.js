globalThis.HookResult = { Continue:0, Changed:1, Handled:2, Stop:3 };
globalThis.Priority   = { High:"high", Normal:"normal", Low:"low", Monitor:"monitor" };
globalThis.Phase      = { Pre:"pre", Post:"post" };
(function () {
  const OnGameFrame = {
    subscribe: (fn, opts) => {
      const id = __s2_subscribe("OnGameFrame", fn, opts || {});
      return { dispose: () => __s2_unsubscribe(id) };
    },
  };
  function makeTimerHandle(id) {
    // A handle rather than a bare id: a bare number invites arithmetic on it and gives no place to
    // hang `alive`. kill() is idempotent and safe to call from inside the callback itself.
    return {
      get alive() { return __s2_timer_alive(id); },
      kill: function () { return __s2_timer_kill(id); },
    };
  }
  const timers = {
    delay: (ms) => __s2_delay(ms || 0),
    nextTick: () => __s2_next_tick(),
    nextFrame: () => __s2_next_frame(),
    threadSleep: (ms) => __s2_thread_sleep(ms || 0),
    after: function (ms, fn) {
      if (typeof fn !== "function") throw new TypeError("__s2pkg_timers.after(ms, fn): fn must be a function");
      return makeTimerHandle(__s2_timer_create(ms || 0, fn, false));
    },
    every: function (ms, fn) {
      if (typeof fn !== "function") throw new TypeError("__s2pkg_timers.every(ms, fn): fn must be a function");
      var id = __s2_timer_create(ms || 0, fn, true);
      if (!id) throw new RangeError("__s2pkg_timers.every(ms, fn): ms must be > 0 (a 0ms repeat would starve the frame)");
      return makeTimerHandle(id);
    },
  };
  // --- Slice 4.5: inter-plugin interfaces ---
  function makeIfaceProxy(name) {
    return new Proxy({}, {
      get: function (_t, prop) {
        if (prop === "on")  return function (ev, h) { return __s2_iface_on(name, ev, h); };
        if (prop === "off") return function (ev, h) { return __s2_iface_off(name, ev, h); };
        if (typeof prop !== "string") return undefined;
        return function () {
          var args = Array.prototype.slice.call(arguments);
          return __s2_iface_call(name, prop, args);
        };
      }
    });
  }
  function resolveInterface(name) {
    var kind = __s2_iface_dep_kind(name);
    if (kind === "none") return null;                       // undeclared specifier
    if (kind === "optional" && !__s2_iface_is_published(name)) return null;
    return makeIfaceProxy(name);                             // hard → always a proxy
  }
  globalThis.__s2_require = function (name) {
    var pkg = __s2require(name);                             // first-party @s2script/* module or game package
    if (pkg !== null && pkg !== undefined) return pkg;
    return resolveInterface(name);                          // inter-plugin, or null
  };
  const interfaces = {
    publishInterface: function (name, impl) {
      __s2_iface_publish(name, impl);
      return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
    },
  };
  // --- Slice 5A/5B.2: serial-gated EntityRef (wraps the __s2_ent_ref_* natives; no raw pointer crosses JS) ---
  var K = { I32: 1, F32: 2, BOOL: 3, I8: 4, I16: 5, U8: 6, U16: 7, U32: 8, U64: 9, I64: 10, F64: 11 }; // mirrors core KIND_*
  function EntityRef(index, id) { this.index = index; this.id = id; }
  EntityRef.prototype = {
    isValid:          function ()      { return __s2_ent_ref_valid(this.index, this.id); },
    readInt32:        function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.I32); },
    writeInt32:       function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.I32, v); },
    readFloat32:      function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.F32); },
    writeFloat32:     function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.F32, v); },
    readBool:         function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.BOOL); },
    writeBool:        function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.BOOL, v); },
    readInt8:         function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.I8); },
    readInt16:        function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.I16); },
    readUInt8:        function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.U8); },
    readUInt16:       function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.U16); },
    readUInt32:       function (o)     { return __s2_ent_ref_read(this.index, this.id, o, K.U32); },
    writeInt8:        function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.I8, v); },
    writeInt16:       function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.I16, v); },
    writeUInt8:       function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.U8, v); },
    writeUInt16:      function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.U16, v); },
    writeUInt32:      function (o, v)  { return __s2_ent_ref_write(this.index, this.id, o, K.U32, v); },
    readUInt64:       function (o)         { return __s2_ent_ref_read(this.index, this.id, o, K.U64); },
    readInt64:        function (o)         { return __s2_ent_ref_read(this.index, this.id, o, K.I64); },
    readFloat64:      function (o)         { return __s2_ent_ref_read(this.index, this.id, o, K.F64); },
    readString:       function (o, maxLen) { return __s2_ent_ref_read_string(this.index, this.id, o, maxLen); },
    writeString:      function (o, maxLen, s) { return __s2_ent_ref_write_string(this.index, this.id, o, maxLen, String(s)); },
    readFloats:       function (o, count)  { return __s2_ent_ref_read_floats(this.index, this.id, o, count); },
    readFloatsChain: function (chain, finalOff, count) { return __s2_ent_ref_read_floats_chain(this.index, this.id, chain, finalOff, count); },
    readInt32Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.I32); },
    writeInt32Via: function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.id, c, o, K.I32, v); },
    readInt8Via:   function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.I8); },
    readInt16Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.I16); },
    readUInt8Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.U8); },
    readUInt16Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.U16); },
    readUInt32Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.U32); },
    readFloat32Via:function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.F32); },
    readBoolVia:   function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.BOOL); },
    readUInt64Via: function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.U64); },
    readInt64Via:  function (c, o) { return __s2_ent_ref_read_chain(this.index, this.id, c, o, K.I64); },
    writeFloat32Via:function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.id, c, o, K.F32, v); },
    writeBoolVia:   function (c, o, v) { return __s2_ent_ref_write_chain(this.index, this.id, c, o, K.BOOL, v); },
    readHandleVia: function (c, o) { var h = __s2_ent_ref_read_chain(this.index, this.id, c, o, K.U32);
      if (h === null) return null; var d = __s2_handle_adopt(h >>> 0);
      return d ? new EntityRef(d[0], d[1]) : null; },   // books-adopted; a dangling handle mints null
    readHandle: function (o) {
      var h = __s2_ent_ref_read(this.index, this.id, o, K.U32);
      if (h === null) return null;
      var d = __s2_handle_adopt(h >>> 0);
      return d ? new EntityRef(d[0], d[1]) : null;
    },
    notifyStateChanged: function (offset) { __s2_ent_ref_state_changed(this.index, this.id, offset); },
    // E1: raw CEntityIdentity::m_flags from the identity SLOT (books-gated; never
    // instance memory). null = stale/unavailable. Flag bit meanings are game facts —
    // interpret them in the game package, not here.
    identityFlags: function () { return __s2_ent_identity_flags(this.index, this.id); },
    clearIdentityFlags: function (mask) { return __s2_ent_identity_flags_clear(this.index, this.id, mask >>> 0); },
  };
  // --- Entity-creation lifecycle slice: spawn/teleport/remove over the entity_* engine ops. Kept as
  //     separate prototype assignments (not folded into the object literal above) to minimize the diff. ---
  // EKV marshal: {key: string|number|boolean} -> parallel {keys, types, values} arrays.
  // types: 0=string 1=int 2=float 3=bool; values stringified ("1"/"0" for bool). Inference:
  // integer-in-int32 -> int, other finite number -> float. ANY bad entry (empty key, non-finite,
  // unsupported value type, >256 keys, an over-length key/value string) rejects the WHOLE map
  // (null) — never a partial spawn.
  // EKV_MAX_STRING_LEN: CKV3Arena's CUtlMemoryBlockAllocator::AddPage() aborts the WHOLE process
  // (a real Plat_FatalError -> our tier0-shimmed Plat_ExitProcess -> abort()) once a single
  // string's backing allocation reaches its MaxPossiblePageSize() bound — which computes to
  // exactly 2048 bytes from the SDK's MEMBLOCK_DEFAULT_PAGESIZE(0x800)/MEMBLOCK_MAX_TOTAL_PAGESIZE
  // constants (confirmed live: 2000B keyvalue strings are fine, 2050B reliably aborts the server).
  // Capped here well under that bound so an ordinary plugin bug (e.g. relaying chat/file/JSON
  // content into a spawn keyvalue) fails closed instead of taking down the whole process.
  var EKV_MAX_STRING_LEN = 1024;
  function __s2_ekv_marshal(kv) {
    var keys = [], types = [], values = [];
    var names = Object.keys(kv);
    if (names.length > 256) return null;
    for (var i = 0; i < names.length; i++) {
      var k = names[i];
      if (!k || k.length > EKV_MAX_STRING_LEN) return null;
      var v = kv[k], t = typeof v;
      if (t === "string") {
        if (v.length > EKV_MAX_STRING_LEN) return null;
        types.push(0); values.push(v);
      }
      else if (t === "number") {
        if (!isFinite(v)) return null;
        if (Number.isInteger(v) && v >= -2147483648 && v <= 2147483647) { types.push(1); values.push(String(v)); }
        else { types.push(2); values.push(String(v)); }
      }
      else if (t === "boolean") { types.push(3); values.push(v ? "1" : "0"); }
      else return null;
      keys.push(k);
    }
    return { keys: keys, types: types, values: values };
  }
  EntityRef.prototype.spawn = function (keyvalues) {
    if (keyvalues === undefined || keyvalues === null) return __s2_entity_spawn(this.index, this.id);
    if (typeof keyvalues !== "object") return false;
    var m = __s2_ekv_marshal(keyvalues);
    if (m === null) return false;
    if (m.keys.length === 0) return __s2_entity_spawn(this.index, this.id);
    return __s2_entity_spawn_kv(this.index, this.id, m.keys, m.types, m.values);
  };
  EntityRef.prototype.teleport = function (origin, angles, velocity) {
    return __s2_entity_teleport(this.index, this.id,
      origin ? [origin[0], origin[1], origin[2]] : null,
      angles ? [angles[0], angles[1], angles[2]] : null,
      velocity ? [velocity[0], velocity[1], velocity[2]] : null);
  };
  EntityRef.prototype.remove = function () { return __s2_entity_remove(this.index, this.id); };
  // Zones real-trigger slice: register this entity's collision bounds in the spatial partition so a
  // runtime-created trigger fires touch. Serial-gated; returns false if the op is unavailable/stale.
  EntityRef.prototype.activateCollision = function () { return __s2_collision_activate(this.index, this.id); };
  // Zones real-trigger slice: give this entity a model (and its collision) via CBaseEntity::SetModel.
  // A runtime trigger_multiple needs a model to build the physics volume that fires touch.
  EntityRef.prototype.setModel = function (name) { return __s2_ent_set_model(this.index, this.id, String(name)); };
  // --- Entity-property slice: five engine setters with no usable schema-write equivalent. ---
  // Gravity multiplier. NOT the same as writing m_flGravityScale: the engine setter early-returns on
  // an unchanged value and maintains m_flActualGravityScale, so a raw field write appears to do nothing.
  EntityRef.prototype.setGravityScale = function (scale) {
    return __s2_ent_set_gravity_scale(this.index, this.id, Number(scale));
  };
  // Add velocity, physics-aware. Writing m_vecAbsVelocity directly skips the partition/physics update.
  // Accepts [x,y,z] or a Vector. A zero impulse is a legal no-op.
  EntityRef.prototype.applyAbsVelocityImpulse = function (impulse) {
    return impulse
      ? __s2_ent_apply_abs_velocity_impulse(this.index, this.id, [impulse[0], impulse[1], impulse[2]])
      : false;
  };
  // Stop a sound on this entity — the counterpart to Sound.emit / pawn.emitSound.
  EntityRef.prototype.stopSound = function (name) {
    return __s2_ent_stop_sound(this.index, this.id, String(name));
  };
  // Model body group by name. The schema route is unavailable (m_bodyGroupChoices is a CUtlOrderedMap).
  EntityRef.prototype.setBodyGroupByName = function (name, group) {
    return __s2_ent_set_body_group_by_name(this.index, this.id, String(name), Number(group) | 0);
  };
  // Model scale. Arg shape confirmed by disassembly; the NAME is a catalogue attribution the function
  // body does not itself prove (see the gamedata comment) — safe to call, verify before relying on it.
  EntityRef.prototype.setModelScale = function (scale) {
    return __s2_ent_set_model_scale(this.index, this.id, Number(scale));
  };
  // Targetname (CEntityIdentity::m_name) — e.g. a map trigger's "map_start". null if stale; "" if unnamed.
  Object.defineProperty(EntityRef.prototype, "name", {
    get: function () { var n = __s2_entity_name(this.index, this.id); return n == null ? null : n; }
  });
  // Target (CBaseEntity::m_target) — e.g. a func_button's target entity name. null if stale; "" if unset.
  Object.defineProperty(EntityRef.prototype, "target", {
    get: function () { var t = __s2_entity_target(this.index, this.id); return t == null ? null : t; }
  });
  // World-space position (CGameSceneNode::m_vecAbsOrigin, via CBaseEntity::m_CBodyComponent ->
  // CBodyComponent::m_pSceneNode) — the same chain Pawn.origin/sceneNode.absOrigin resolve for player
  // pawns, here on any entity. All three offsets are live-resolved every call (never baked); no
  // dedicated native — this composes the already-native __s2_schema_offset with readFloatsChain.
  // null if any offset is unresolved, any hop in the chain is null, or the ref is stale/invalid — there
  // is no meaningful "empty" position, so null always means "could not read".
  Object.defineProperty(EntityRef.prototype, "origin", {
    get: function () {
      var bodyOff = __s2_schema_offset("CBaseEntity", "m_CBodyComponent");
      var nodeOff = __s2_schema_offset("CBodyComponent", "m_pSceneNode");
      var absOff  = __s2_schema_offset("CGameSceneNode", "m_vecAbsOrigin");
      if (bodyOff < 0 || nodeOff < 0 || absOff < 0) return null;
      var a = this.readFloatsChain([bodyOff, nodeOff], absOff, 3);
      return a === null ? null : new Vector(a[0], a[1], a[2]);
    }
  });
  // Create a new entity by class name (e.g. "env_beam"). Returns a serial-gated EntityRef, or null.
  // With keyvalues: create + DispatchSpawn(keyvalues) in one call — a non-null result is a LIVE,
  // SPAWNED entity (on spawn failure the entity is removed and null returned).
  function createEntity(className, keyvalues) {
    var ref = __s2_entity_create(String(className));
    if (!ref) return null;
    if (keyvalues !== undefined && keyvalues !== null) {
      // With kv: non-null result = a LIVE, SPAWNED entity. On spawn failure, remove the unspawned
      // entity (hygiene — never strand a half-configured entity) and return null.
      if (!ref.spawn(keyvalues)) { ref.remove(); return null; }
    }
    return ref;
  }
  // Item slice: read a CUtlVector<CHandle> at (ptrOffs chain -> vectorOff) as live serial-gated
  // EntityRefs. Each element is decoded + validated core-side; the raw pointer never crosses to JS.
  EntityRef.prototype.readHandleVector = function (ptrOffs, vectorOff, maxCount) {
    return __s2_entity_read_handle_vector(this.index, this.id, ptrOffs || [], vectorOff, maxCount || 64);
  };
  // Entity-I/O slice: fire an input (e.g. "Kill"/"Ignite"/"FireUser1") via AddEntityIOEvent — the
  // game's own input-firing path (delay 0 = the same-tick I/O pump). value is the input's string
  // argument (Source parses it per the input's field type); activator/caller are optional EntityRefs.
  EntityRef.prototype.acceptInput = function (input, value, activator, caller, delay) {
    return __s2_entity_fire_input(
      this.index, this.id, String(input),
      (value === undefined || value === null) ? "" : String(value),
      activator ? activator.index : -1, activator ? activator.id : -1,
      caller ? caller.index : -1, caller ? caller.id : -1,
      delay || 0);
  };
  // Entity-I/O slice: hook an entity output (e.g. "OnTrigger"/"OnPressed"/"OnStartTouch"). classname/
  // output accept "*" wildcards. handler(ev) may return a HookResult >= Handled to suppress the output
  // (the FireOutputInternal detour supersedes the original call). Dispatch is SYNCHRONOUS.
  var Entity = {
    onOutput: function (classname, output, handler) { return __s2_output_subscribe(String(classname), String(output), handler); },
    // Entity lifecycle listeners: fire when the engine creates/spawns/deletes an entity of `className`
    // ("*" = all). The handler gets (entity, className): `entity` is a serial-gated EntityRef (may be
    // null for a barely-constructed onCreate / a dying onDelete); `className` is always valid.
    onCreate: function (className, handler) { return __s2_entity_listener_on("create", String(className), handler); },
    onSpawn:  function (className, handler) { return __s2_entity_listener_on("spawn",  String(className), handler); },
    onDelete: function (className, handler) { return __s2_entity_listener_on("delete", String(className), handler); },
    // Find every entity whose designer-name (class) exactly matches className. Returns serial-gated
    // EntityRefs (empty array on no-op/degrade). Broadly reusable (gamerules proxy, props, triggers...).
    findByClass: function (className) {
      return __s2_entity_find_by_class(String(className));
    },
  };
  // Inter-plugin wire tagging: an EntityRef crosses the structured-copy (JSON) boundary as a tagged
  // envelope so the target context rehydrates it into a LIVE EntityRef (bound to ITS natives), not
  // plain data. `__s2ref` is the E1 wire key — [index, HOST-id]. Old `__entref__` (engine-serial)
  // blobs deliberately revive as inert plain data (the stale-data contract). Used by iface_to_json /
  // iface_from_json.
  globalThis.__s2_entref_replacer = function (key, value) {
    return (value instanceof EntityRef) ? { __s2ref: [value.index, value.id] } : value;
  };
  globalThis.__s2_entref_reviver = function (key, value) {
    return (value && typeof value === "object" && Array.isArray(value.__s2ref))
      ? new EntityRef(value.__s2ref[0], value.__s2ref[1])
      : value;
  };
  // --- Slice 5C.3: math value types (Vector, QAngle) — pure JS, no engine ops ---
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  Vector.prototype.length = function () { return Math.sqrt(this.x * this.x + this.y * this.y + this.z * this.z); };
  Vector.prototype.toString = function () { return "Vector(" + this.x + ", " + this.y + ", " + this.z + ")"; };
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  QAngle.prototype.toString = function () { return "QAngle(" + this.x + ", " + this.y + ", " + this.z + ")"; };
  // Ray-trace slice: angle -> unit forward-direction vector (x=pitch, y=yaw, per the QAngle
  // convention above). Pure math, no engine ops — lives in @s2script/math since a forward vector
  // is Source-2-generic, not CS2-specific (@s2script/trace's Trace.ray composes it).
  function forwardVector(a) {
    var p = a.x * Math.PI / 180, y = a.y * Math.PI / 180;
    return new Vector(Math.cos(p) * Math.cos(y), Math.cos(p) * Math.sin(y), -Math.sin(p));
  }
  // --- Slice 5D.1: GameEvent constructor (dispatch_game_event constructs new GameEvent(name)
  //     per-plugin from globalThis.__s2pkg_events.GameEvent). ---
  function GameEvent(name) { this.name = name; }
  GameEvent.prototype.getInt        = function (k) { return __s2_event_get_int(k); };
  GameEvent.prototype.getFloat      = function (k) { return __s2_event_get_float(k); };
  GameEvent.prototype.getBool       = function (k) { return __s2_event_get_bool(k); };
  GameEvent.prototype.getString     = function (k) { return __s2_event_get_string(k); };
  GameEvent.prototype.getUint64     = function (k) { return __s2_event_get_uint64(k); };   // decimal string
  GameEvent.prototype.getPlayerSlot = function (k) { return __s2_event_get_player_slot(k); };
  GameEvent.prototype.setInt    = function (k, v) { __s2_event_set_int(k, v | 0); };
  GameEvent.prototype.setFloat  = function (k, v) { __s2_event_set_float(k, v); };
  GameEvent.prototype.setBool   = function (k, v) { __s2_event_set_bool(k, !!v); };
  GameEvent.prototype.setString = function (k, v) { __s2_event_set_string(k, String(v)); };
  GameEvent.prototype.setUint64 = function (k, v) { __s2_event_set_uint64(k, String(v)); };   // decimal string
  // --- Slice 5D.1 Task 2 / 5D.3: Events.on/off/onPre/fire — prelude module object for @s2script/events. ---
  var Events = {
    on:    function (name, handler) { return __s2_event_subscribe(name, handler); },
    off:   function (name, handler) { __s2_event_unsubscribe(name, handler); },
    onPre: function (name, handler) { return __s2_event_subscribe_pre(name, handler); },
    // shared: apply { key: value } to the current event (create must have run). Type-infer as in `fire`.
    _applyFields: function (fields) {
      if (!fields) return;
      for (var k in fields) {
        if (!Object.prototype.hasOwnProperty.call(fields, k)) continue;
        var v = fields[k], t = typeof v;
        if (t === "boolean") __s2_event_set_bool(k, v);
        else if (t === "string") __s2_event_set_string(k, v);
        else if (t === "bigint") __s2_event_set_uint64(k, v.toString());
        else if (t === "number") { if (Number.isInteger(v)) __s2_event_set_int(k, v); else __s2_event_set_float(k, v); }
      }
    },
    // Fire a game event. fields: { key: value }. Runtime type-infer: bool→setBool, string→setString,
    // bigint→setUint64, integer number→setInt, other number→setFloat. Returns the FireEvent result.
    fire:  function (name, fields, dontBroadcast) {
      if (!__s2_event_create(name)) return false;
      this._applyFields(fields);
      return __s2_event_fire(!!dontBroadcast);
    },
    // Fire a game event to ONE client (SourceMod FireToClient parity). Same field type-inference as
    // `fire`. Returns false on any miss (no manager / no pending event / no client / bot).
    // Restrict the event currently being pre-dispatched to these slots. Only meaningful from inside an
    // `onPre` handler, and only when that handler also returns HookResult.Handled — the pair means
    // "suppress the normal broadcast, deliver to exactly these viewers instead". An empty array hides
    // the event from everyone.
    setRecipients: function (slots) { __s2_event_set_recipients(slots || []); },
    fireToClient: function (slot, name, fields) {
      if (!__s2_event_create(name)) return false;
      this._applyFields(fields);
      return __s2_event_fire_to_client(slot | 0);
    },
  };
  // --- Slice 5E.2: config module (typed getters over __s2pkg_config_values; zero-value fallback) ---
  // A dotted key ("section.child") walks nested section objects; a plain key reads the top level.
  function __s2_config_walk(k) {
    var v = globalThis.__s2pkg_config_values;
    if (v == null) return undefined;
    var parts = String(k).split(".");
    for (var i = 0; i < parts.length; i++) {
      if (v == null || typeof v !== "object") return undefined;
      v = v[parts[i]];
    }
    return v;
  }
  var __s2_config = {
    getString: function (k) { var v = __s2_config_walk(k); return v == null ? "" : String(v); },
    getInt:    function (k) { var v = __s2_config_walk(k); return (v == null || typeof v !== "number") ? 0 : (v | 0); },   // int = 32-bit (SourceMod ConVar parity); `v | 0` truncates by design
    getFloat:  function (k) { var v = __s2_config_walk(k); return (v == null || typeof v !== "number") ? 0 : v; },
    getBool:   function (k) { var v = __s2_config_walk(k); return v === true; },
    onChange:  function (h) { __s2_config_on_change(h); },
    readFile:  function (name) { return __s2_config_read_file(String(name)); },
    writeFile: function (name, content) { __s2_config_write_file(String(name), String(content)); },
  };
  // --- Menu primitive (engine-generic): model + pagination + registerRenderer seam. Slot-based. ---
  var MenuStyle = { Chat: "chat", Center: "center" };
  var MenuCancelReason = { Exit: 0, Timeout: 1, Disconnect: 2, NewMenu: 3 };
  var MENU_ITEMS_PER_PAGE = 7;            // chat page size (SM ITEMS_PER_PAGE)
  var __s2_menu_renderers = {};           // style value -> renderer { open, update, close }
  var __s2_menu_activeBySlot = {};        // slot -> session (one active menu per slot, this context)

  function Menu(title) {
    this.title = title || "";
    this.style = MenuStyle.Chat;
    this.exitButton = true;
    this.freezePlayer = false;   // a renderer that supports it (CS2 center) freezes the player while open
    this.items = [];
    this._onSelect = null;
    this._onCancel = null;
  }
  Menu.registerRenderer = function (name, renderer) { __s2_menu_renderers[name] = renderer; };
  Menu.prototype.addItem = function (info, display, opts) {
    this.items.push({ info: String(info), display: String(display), disabled: !!(opts && opts.disabled) });
  };
  Menu.prototype.onSelect = function (fn) { this._onSelect = fn; };
  Menu.prototype.onCancel = function (fn) { this._onCancel = fn; };
  Menu.prototype.display = function (slot, seconds) {
    if (typeof slot !== "number" || slot < 0) return;   // console/invalid is never a menu target
    var renderer = __s2_menu_renderers[this.style] || __s2_menu_renderers[MenuStyle.Chat];
    if (!renderer) { globalThis.console && console.log("[menu] no renderer for style " + this.style); return; }
    // Install THIS session as the active one for the slot BEFORE ending the previous — so prev._end()
    // (which runs the plugin's onCancel synchronously) can't mis-delete us, and a re-entrant display()
    // from that onCancel replaces our map entry, which we then respect instead of clobbering.
    var prev = __s2_menu_activeBySlot[slot];
    var session = new MenuSession(this, slot, renderer, seconds || 0);
    __s2_menu_activeBySlot[slot] = session;
    if (prev) prev._end(MenuCancelReason.NewMenu);        // NewMenu; may re-enter display() for this slot
    if (__s2_menu_activeBySlot[slot] !== session) return; // a re-entrant display won — abandon this one
    session._start();
  };
  Menu.prototype.close = function (slot) {
    var s = __s2_menu_activeBySlot[slot];
    if (s && s.menu === this) s._end(MenuCancelReason.Exit);
  };

  // A live display of one menu to one slot. Owns page/cursor state.
  function MenuSession(menu, slot, renderer, seconds) {
    this.menu = menu; this.slot = slot; this.renderer = renderer; this.seconds = seconds;
    this.page = 0; this.cursor = 0; this._ended = false;
    this._selectable = [];   // indices (into menu.items) that are selectable on the CURRENT page
  }
  // Selectable item indices for a chat page: up to MENU_ITEMS_PER_PAGE, skipping disabled.
  MenuSession.prototype._pageItems = function (page) {
    var out = [], start = page * MENU_ITEMS_PER_PAGE, i = start;
    // NOTE: disabled items still occupy a slot in the on-screen list but get no number.
    for (; i < this.menu.items.length && (i - start) < MENU_ITEMS_PER_PAGE; i++) out.push(i);
    return out;
  };
  MenuSession.prototype.pageCount = function () {
    return Math.max(1, Math.ceil(this.menu.items.length / MENU_ITEMS_PER_PAGE));
  };
  // Center navigable targets on the CURRENT page, in display order: the selectable items, then the
  // Back/Next/Exit controls. The CENTER cursor indexes into THIS list (not just items) so W/S can reach
  // paging + Exit and E confirms them — otherwise a center menu can't paginate or be dismissed.
  MenuSession.prototype._navTargets = function () {
    var t = [], pageItems = this._pageItems(this.page);
    for (var k = 0; k < pageItems.length; k++) { var idx = pageItems[k]; if (!this.menu.items[idx].disabled) t.push({ kind: "item", index: idx }); }
    var pc = this.pageCount();
    if (this.page > 0)        t.push({ kind: "back" });
    if (this.page < pc - 1)   t.push({ kind: "next" });
    if (this.menu.exitButton) t.push({ kind: "exit" });
    return t;
  };
  // Build the resolved view the renderer paints. Assigns chat number keys 1..7 to selectable items,
  // then control keys 8=Back, 9=Next, 0=Exit as applicable. For a Center menu, marks the line under
  // the cursor (an item OR a control) so the renderer can highlight it.
  MenuSession.prototype.view = function () {
    var m = this.menu, pageItems = this._pageItems(this.page), lines = [], keyNum = 1;
    this._selectable = [];
    var nav = (m.style === MenuStyle.Center) ? this._navTargets() : null;
    var cur = (nav && this.cursor >= 0 && this.cursor < nav.length) ? nav[this.cursor] : null;
    for (var k = 0; k < pageItems.length; k++) {
      var idx = pageItems[k], it = m.items[idx], key = null, selectable = false;
      if (!it.disabled) { key = String(keyNum++); selectable = true; this._selectable.push(idx); }
      lines.push({ text: it.display, key: key, selectable: selectable, cursor: !!(cur && cur.kind === "item" && cur.index === idx), index: idx });
    }
    var pc = this.pageCount();
    if (this.page > 0)      lines.push({ text: "Back", key: "8", selectable: false, control: "back", cursor: !!(cur && cur.kind === "back") });
    if (this.page < pc - 1) lines.push({ text: "Next", key: "9", selectable: false, control: "next", cursor: !!(cur && cur.kind === "next") });
    if (m.exitButton)       lines.push({ text: "Exit", key: "0", selectable: false, control: "exit", cursor: !!(cur && cur.kind === "exit") });
    return { title: m.title, lines: lines, page: this.page, pageCount: pc, exit: m.exitButton };
  };
  MenuSession.prototype._start = function () { this.renderer.open(this); if (this.seconds > 0) this._armTimeout(); };
  // Timeout: arm a delay that cancels the session (any renderer). Lazily reads __s2pkg_timers at
  // call time (not module-load time), so this is safe regardless of prelude assignment order.
  MenuSession.prototype._armTimeout = function () {
    var self = this, ms = (this.seconds | 0) * 1000;
    globalThis.__s2pkg_timers.delay(ms).then(function () {
      if (!self._ended) self._end(MenuCancelReason.Timeout);
    });
  };
  MenuSession.prototype._repaint = function () { if (!this._ended) this.renderer.update(this); };
  MenuSession.prototype._end = function (reason) {
    if (this._ended) return; this._ended = true;
    if (__s2_menu_activeBySlot[this.slot] === this) delete __s2_menu_activeBySlot[this.slot];
    this.renderer.close(this.slot);
    if (this.menu._onCancel && (reason === MenuCancelReason.Timeout || reason === MenuCancelReason.Disconnect || reason === MenuCancelReason.NewMenu || reason === MenuCancelReason.Exit))
      { try { this.menu._onCancel({ slot: this.slot, reason: reason }); } catch (e) { globalThis.console && console.log("[menu] onCancel threw: " + e); } }
  };
  MenuSession.prototype._select = function (itemIndex) {
    var it = this.menu.items[itemIndex];
    if (!it || it.disabled) return;
    // mark ended BEFORE the callback so a re-display inside onSelect isn't clobbered
    this._ended = true;
    if (__s2_menu_activeBySlot[this.slot] === this) delete __s2_menu_activeBySlot[this.slot];
    this.renderer.close(this.slot);
    if (this.menu._onSelect) { try { this.menu._onSelect({ slot: this.slot, item: itemIndex, info: it.info, display: it.display }); } catch (e) { globalThis.console && console.log("[menu] onSelect threw: " + e); } }
  };
  // Chat idiom: a number-key pick against the current view's keys.
  MenuSession.prototype.pickNumber = function (n) {
    if (this._ended) return;
    this.view();  // refresh this._selectable for the current page
    var key = String(n);
    if (key === "8" && this.page > 0)                      { this.page--; this.cursor = 0; this._repaint(); return; }
    if (key === "9" && this.page < this.pageCount() - 1)   { this.page++; this.cursor = 0; this._repaint(); return; }
    if (key === "0" && this.menu.exitButton)               { this._end(MenuCancelReason.Exit); return; }
    var slotN = n - 1;
    if (slotN >= 0 && slotN < this._selectable.length) this._select(this._selectable[slotN]);
  };
  // Center idiom: cursor navigation over _navTargets (items + Back/Next/Exit controls).
  MenuSession.prototype.moveUp = function () {
    if (this._ended) return;
    var n = this._navTargets().length; if (!n) return;
    this.cursor = (this.cursor - 1 + n) % n; this._repaint();
  };
  MenuSession.prototype.moveDown = function () {
    if (this._ended) return;
    var n = this._navTargets().length; if (!n) return;
    this.cursor = (this.cursor + 1) % n; this._repaint();
  };
  // Confirm the current cursor target: an item selects; Back/Next paginate (cursor to top); Exit cancels.
  MenuSession.prototype.confirm = function () {
    if (this._ended) return;
    var nav = this._navTargets(); if (this.cursor < 0 || this.cursor >= nav.length) return;
    var t = nav[this.cursor];
    if (t.kind === "item")      this._select(t.index);
    else if (t.kind === "back") { this.page--; this.cursor = 0; this._repaint(); }
    else if (t.kind === "next") { this.page++; this.cursor = 0; this._repaint(); }
    else if (t.kind === "exit") this._end(MenuCancelReason.Exit);
  };
  MenuSession.prototype.cancel = function () { if (!this._ended) this._end(MenuCancelReason.Exit); };
  // --- General UserMessage builder (@s2script/usermessages): accumulate scalar fields, then flush
  // create -> set* -> send in one synchronous burst (the shim holds a single build-then-send target,
  // so there is no cross-message aliasing without an await between). Engine-generic — the message NAME
  // is the caller's; core knows no CS2 message strings. ---
  function UserMessage(name) { this._name = String(name); this._fields = []; }
  UserMessage.prototype.setInt    = function (f, v) { this._fields.push([0, String(f), v]); return this; };
  UserMessage.prototype.setFloat  = function (f, v) { this._fields.push([1, String(f), v]); return this; };
  UserMessage.prototype.setString = function (f, v) { this._fields.push([2, String(f), String(v)]); return this; };
  UserMessage.prototype.setBool   = function (f, v) { this._fields.push([3, String(f), v ? 1 : 0]); return this; };
  UserMessage.prototype.set = function (f, v) {
    if (typeof v === "boolean") return this.setBool(f, v);
    if (typeof v === "string")  return this.setString(f, v);
    if (typeof v === "number")  return Number.isInteger(v) ? this.setInt(f, v) : this.setFloat(f, v);
    return this;
  };
  UserMessage.prototype._flush = function (slotsOrNull) {
    if (__s2_user_message_create(this._name) !== 1) return false;
    for (var i = 0; i < this._fields.length; i++) {
      var fld = this._fields[i];
      if (fld[0] === 0)      __s2_user_message_set_int(fld[1], fld[2]);
      else if (fld[0] === 1) __s2_user_message_set_float(fld[1], fld[2]);
      else if (fld[0] === 2) __s2_user_message_set_string(fld[1], fld[2]);
      else                   __s2_user_message_set_bool(fld[1], fld[2]);
    }
    return __s2_user_message_send(slotsOrNull) === true;
  };
  UserMessage.prototype.send    = function (slots) { return this._flush(Array.isArray(slots) ? slots : [slots]); };
  UserMessage.prototype.sendAll = function () { return this._flush(null); };
  // --- UserMessage interception (usermsg-hook slice). The view is BLOCK-SCOPED: the shim's current-
  // message statics are nulled when the synchronous dispatch returns, so reads after an await (or on a
  // stashed view) return null/[]/"" — never a dangling pointer. Suppression is the HookResult return
  // (>= Handled supersedes the send for EVERY recipient). Plugin-originated sends from inside any JS
  // dispatch do NOT re-trigger these hooks (recursion guard; documented v1 limitation).
  function __s2_umView(name, id) {
    return {
      name: name, id: id,
      get recipients() { return __s2_usermsg_recipients(); },
      get debugString() { return __s2_usermsg_debug(); },
      hasField:   function (p) { return __s2_usermsg_has_field(String(p)) === 1; },
      readInt:    function (p) { return __s2_usermsg_read_int(String(p)); },
      readFloat:  function (p) { return __s2_usermsg_read_float(String(p)); },
      readBool:   function (p) { var v = __s2_usermsg_read_int(String(p)); return v === null ? null : v !== 0; },
      readString: function (p) { return __s2_usermsg_read_string(String(p)); }
    };
  }
  var UserMessages = {
    onPre: function (name, handler) {
      var canonical = __s2_usermsg_on(String(name), function (n, id) { return handler(__s2_umView(n, id)); });
      if (!canonical)
        throw new Error("UserMessages.onPre: cannot resolve message '" + name +
                        "' (unknown name, or the intercept descriptor is degraded — see server log)");
    },
    off: function (name) { __s2_usermsg_off(String(name)); }
  };
  // @s2script/transmit — per-client entity visibility rules (checktransmit slice). Declarative:
  // the native side evaluates rules per snapshot; NO JS runs in the CheckTransmit hot path.
  var Transmit = {
    setVisibleTo: function (entity, viewers) {
      if (!entity || typeof entity.index !== "number" || typeof entity.id !== "number") return false;
      if (!Array.isArray(viewers)) throw new TypeError("viewers must be an array of player slots");
      for (var i = 0; i < viewers.length; i++) {
        var s = viewers[i];
        if (typeof s !== "number" || (s | 0) !== s || s < 0 || s >= 64)
          throw new RangeError("viewer slot out of range [0,64): " + s);
      }
      return __s2_transmit_set(entity.index, entity.id, viewers);
    },
    reset: function (entity) {
      if (!entity || typeof entity.index !== "number" || typeof entity.id !== "number") return false;
      return __s2_transmit_reset(entity.index, entity.id);
    },
    resetAll: function () { __s2_transmit_reset_all(); },
    stats: function () { return __s2_transmit_stats(); }
  };
  // @s2script/sdk/voice — who can hear whom, per (receiver, sender) pair. Declarative for the same
  // reason Transmit is: the SetClientListening hook fires per PAIR, so a JS callback there would run
  // up to 64x64 times per voice refresh. Layered UNDER Client.voiceMuted, which still wins.
  var Voice = {
    setAudibleTo: function (sender, receivers) {
      if (!Array.isArray(receivers)) throw new TypeError("Voice.setAudibleTo: receivers must be an array");
      return __s2_voice_audible_set(sender, receivers);
    },
    reset: function (sender) { return __s2_voice_audible_clear(sender); },
    resetAll: function () { __s2_voice_reset_all(); },
    stats: function () { return __s2_voice_audible_stats(); }
  };
  globalThis.__s2pkg_math       = { Vector: Vector, QAngle: QAngle, forwardVector: forwardVector };
  globalThis.__s2pkg_entity     = { EntityRef: EntityRef, createEntity: createEntity, Entity: Entity };
  globalThis.__s2pkg_transmit  = { Transmit: Transmit };
  globalThis.__s2pkg_voice      = { Voice: Voice };
  globalThis.__s2pkg_usermessages = { UserMessage: UserMessage, UserMessages: UserMessages };
  globalThis.__s2pkg_frame      = { OnGameFrame: OnGameFrame };
  globalThis.__s2pkg_timers     = timers;
  globalThis.__s2pkg_console    = { console: console };
  globalThis.__s2pkg_interfaces = interfaces;
  globalThis.__s2pkg_events     = { GameEvent: GameEvent, Events: Events, HookResult: globalThis.HookResult };
  globalThis.__s2pkg_config     = { config: __s2_config };   // named export `config` (matches the .d.ts: import { config } from "@s2script/config")
  // --- @s2script/translations — SM-style i18n. Phrases: a flat key->text map; the plugin's `seed` is the
  //     in-memory English default; translations/<code>/<name>.phrases.json (read lazily) overrides per language;
  //     an optional root translations/<name>.phrases.json overrides the seed. Fully engine-generic. ---
  var __s2_tr_reg = Object.create(null);   // name -> { def: {k:text}, langs: { code: {k:text}|null } }  (null = tried+absent)
  var __s2_tr_default = "";          // server/console default language code ("" = root/English)
  // Steam cl_language -> folder code ("" = root/English). Unmapped -> "" (default).
  var __s2_TR_CODES = Object.assign(Object.create(null), { english:"", german:"de", russian:"ru", french:"fr", spanish:"es", latam:"es",
    schinese:"zh", tchinese:"zh", portuguese:"pt", brazilian:"pt", polish:"pl", italian:"it", dutch:"nl",
    swedish:"sv", danish:"da", finnish:"fi", norwegian:"no", czech:"cs", hungarian:"hu", turkish:"tr",
    japanese:"ja", koreana:"ko", thai:"th", ukrainian:"uk", bulgarian:"bg", greek:"el", romanian:"ro" });
  function __s2_tr_langCode(clLang) {
    var v = __s2_TR_CODES[String(clLang || "").toLowerCase()];
    return (typeof v === "string") ? v : "";   // non-string (e.g. a "__proto__" chain read) -> default
  }
  function __s2_tr_format(text, args) {
    return String(text).replace(/\{(\d+)\}/g, function (_m, n) {
      var i = (parseInt(n, 10) | 0) - 1;
      if (!args || i < 0 || i >= args.length || args[i] == null) return "";
      // Braces are stripped from SUBSTITUTED values so an argument cannot inject a colour tag
      // (colors.js expands the finished string at output). A player name loses a brace; nobody
      // recolours anyone else's chat. This is blunt, not name-aware: admin free text (a cvar value,
      // a chosen new name) goes through the same strip as collateral damage, so a caller who echoes
      // a free-text argument back to confirm it is echoing the STRIPPED value, not the one actually
      // stored/set — a call site that cares must sanitize braces out of its own input up front so
      // the two match (see basecommands' sm_cvar and playercommands' sm_rename).
      return String(args[i]).replace(/[{}]/g, "");
    });
  }
  function __s2_tr_parse(text) { try { var o = JSON.parse(text); return (o && typeof o === "object") ? o : {}; } catch (e) { console.log("[s2script] WARN: translations file malformed — ignored"); return {}; } }
  function __s2_tr_merge(dst, src) {   // copy own enumerable keys, skipping __proto__ (no by-ref share, no proto pollution)
    for (var k in src) if (Object.prototype.hasOwnProperty.call(src, k) && k !== "__proto__") dst[k] = src[k];
    return dst;
  }
  function __s2_tr_langMap(name, code) {                     // the lazily-read (+cached) map for a code ("" = root override)
    var r = __s2_tr_reg[name]; if (!r) return null;
    if (Object.prototype.hasOwnProperty.call(r.langs, code)) return r.langs[code];   // cached (map or null)
    var text = __s2_translations_read(code, name);           // null if absent/no-op
    var map = (text == null) ? null : __s2_tr_parse(text);
    r.langs[code] = map;
    return map;
  }
  var __s2_translations = {
    load: function (name, seed) {
      name = String(name);
      var hasSeed = !!(seed && typeof seed === "object");
      var def = __s2_tr_merge({}, hasSeed ? seed : {});   // fresh copy, not the caller's ref
      __s2_tr_reg[name] = { def: def, langs: {} };
      var root = __s2_translations_read("", name);           // OPTIONAL root override of the seed
      if (root != null) {
        __s2_tr_merge(def, __s2_tr_parse(root));                                     // root file overrides seed keys
      } else if (!hasSeed) {
        // A plugin with a seed and a missing root file is the normal, correct case (degrades to the
        // in-code English default) and must stay silent. A SEEDLESS load (the "common" convention —
        // no second argument) has nothing to degrade TO: if translations/<name>.phrases.json is also
        // missing, this set is now empty, and every key resolved against it falls all the way through
        // to translate's ultimate fallback — the bare key text, rendered to players with no [SM]
        // prefix, no colour, and nothing in the console to explain why.
        console.log("[s2script] WARN: translations set \"" + name + "\" has no seed and translations/"
          + name + ".phrases.json is missing — every key in it will render as its own key text");
      }
    },
    setDefaultLanguage: function (code) { __s2_tr_default = String(code || ""); },
    translate: function (slot, key) {
      var args = [].slice.call(arguments, 2);
      key = String(key);
      var code = ((slot | 0) < 0) ? __s2_tr_default : __s2_tr_langCode(__s2_client_language(slot | 0));
      // Sweep EVERY loaded set for the client's language FIRST, and only then sweep every set for
      // an English default. The old single pass checked one set's language map and then that same
      // set's default before moving on, so an earlier set's English beat a later set's translation.
      if (code) {
        for (var ln in __s2_tr_reg) {
          if (!Object.prototype.hasOwnProperty.call(__s2_tr_reg, ln)) continue;
          var lm = __s2_tr_langMap(ln, code);
          if (lm && lm[key] != null) return __s2_tr_format(lm[key], args);
        }
      }
      for (var dn in __s2_tr_reg) {
        if (!Object.prototype.hasOwnProperty.call(__s2_tr_reg, dn)) continue;
        var d = __s2_tr_reg[dn].def;
        if (d[key] != null) return __s2_tr_format(d[key], args);
      }
      return key;                                            // ultimate fallback
    },
  };
  globalThis.__s2_tr_format = __s2_tr_format;                 // test hooks (pure)
  globalThis.__s2_tr_langCode = __s2_tr_langCode;
  globalThis.__s2_tr_injectLang = function (name, code, obj) { if (__s2_tr_reg[name]) __s2_tr_reg[name].langs[code] = obj; };  // test hook (bypasses the file read)
  globalThis.__s2pkg_translations = { Translations: __s2_translations };
  globalThis.__s2pkg_menu       = { Menu: Menu, MenuStyle: MenuStyle, MenuCancelReason: MenuCancelReason };
  // --- adminmenu framework: the TopMenu registry (categories/items owned by the registering plugin;
  // onSelect is dispatched to the OWNER's context post-drain — see __s2_topmenu_select). ---
  globalThis.__s2pkg_topmenu = { TopMenu: {
    addCategory: function (name) { __s2_topmenu_add_category(String(name)); },
    addItem: function (category, item) { __s2_topmenu_add_item(String(category), String(item.id), String(item.name), item.flags | 0, item.onSelect); },
    snapshot: function () { return __s2_topmenu_snapshot(); },
    select: function (id, slot) { __s2_topmenu_select(String(id), slot | 0); },
  } };
  // --- Slice 6.1: chat module (toSlot/toAll; toAll loops __s2_client_valid, engine-generic) ---
  // `color` is an OPAQUE leading prefix prepended to every chat message (NOT the console.log reply path,
  // so rcon/server-console output stays clean). Core doesn't know what it means — a game package or plugin
  // sets it to a color control byte (CS2: a ChatColors byte); "" = send raw. This keeps color as CONTENT
  // owned by the caller (SourceMod-parity), never a native-layer default. A message may still embed its own
  // color codes mid-string.
  //
  // A leading ZERO-WIDTH SPACE (U+200B) is prepended to every chat line: a Source 2 chat box mutes a color
  // control byte that sits at index 0, so without a preceding byte the message's first color never renders.
  // Prepending the byte here means a plugin author writes `Green + "hi"` and the colour just lands — they
  // never hand-roll a prefix (SourceMod / CounterStrikeSharp do the same, some with a plain space). We use
  // U+200B rather than a plain " " so the affordance is INVISIBLE — it satisfies the "need a leading byte"
  // rule without shifting every line right by a visible space. Idempotent: a line that ALREADY starts with
  // the ZWSP or a (legacy) plain space is left untouched, so an existing prefix never doubles up. Chat-only
  // — the console.log / replyToConsole path never runs through here, so console output stays byte-clean.
  //
  // Colour TAGS ({green}, {default}) are expanded here too — see core/js/colors.js. The table is
  // supplied by the game package at runtime; core never knows a colour name.
  function __s2_chatLine(msg) { return globalThis.__s2_colors.chatLine(__s2_chat.color, msg); }
  var __s2_chat = {
    color: "",
    toSlot: function (slot, msg) { __s2_client_print(slot | 0, __s2_chatLine(msg)); },
    // slot -1 = broadcast to all in ONE call (the shim routes it to the game's UTIL_ClientPrintAll, which
    // renders true custom color, not team color — SourceMod's PrintToChatAll). NOT a per-slot loop.
    toAll:  function (msg) { __s2_client_print(-1, __s2_chatLine(msg)); },
  };
  globalThis.__s2pkg_chat = { Chat: __s2_chat };   // named export `Chat`
  // --- Slice 6.4: server module (command / isMapValid; engine-generic server control) ---
  var __s2_server = {
    command: function (cmd) { __s2_server_command(String(cmd)); },
    isMapValid: function (map) { return __s2_server_map_valid(String(map)) === 1; },
    getCvar: function (name) { return __s2_cvar_get(String(name)); },                 // "" if absent
    setCvar: function (name, value) { return __s2_cvar_set(String(name), String(value)); },
    // onCvarChange(name|"*", handler) -> { dispose() }. Notify-only: the engine's global change
    // callback runs AFTER the value is applied, so there is nothing to veto.
    onCvarChange: function (name, handler) {
      if (typeof handler !== "function") throw new TypeError("Server.onCvarChange(name, fn): fn must be a function");
      var n = String(name);
      __s2_cvar_on_change(n, handler);
      return { dispose: function () { __s2_cvar_off_change(n); } };
    },
    // Register a plugin-owned ConVar (FakeConVar). Type-checked JS-side; the shim ORs FCVAR_RELEASE.
    // Value reads reuse getCvar; writes reuse setCvar/console. Idempotent (reload-safe); the cvar and
    // its value persist for the process lifetime (SourceMod parity).
    registerCvar: function (name, opts) {
      opts = opts || {};
      var tmap = { bool: 0, int: 1, float: 2, string: 3 };
      var type = tmap[String(opts.type == null ? "string" : opts.type)];
      if (type === undefined) return false;
      var def = opts.default;
      var defStr = (type === 0) ? (def ? "1" : "0")
                                : String(def == null ? (type === 3 ? "" : 0) : def);
      return __s2_convar_register(String(name),
        opts.help == null ? null : String(opts.help),
        opts.flags == null ? 0 : +opts.flags, type, defStr,
        opts.min == null ? null : String(opts.min),
        opts.max == null ? null : String(opts.max)) === 1;
    },
    // Subscribe to map start (the framework event replacing the Server.mapName OnGameFrame poll).
    // Fires on every StartupServer (boot-loaded plugins get the first map); a plugin hot-loaded
    // mid-map should read Server.mapName at load for the CURRENT map. Handlers may be async
    // (fire-and-forget). Auto-ledgered per plugin; torn down on unload.
    onMapStart: function (h) { return __s2_map_start_subscribe(h); },
    get maxPlayers() { return __s2_server_max_clients(); },   // GetMaxClients(); 0 if unavailable
    get mapName() { return __s2_server_map_name(); },         // GetMapName(); "" if unavailable
    get gameTime() { return __s2_server_game_time(); },       // GetGlobals()->curtime; 0 if unavailable
  };
  globalThis.__s2pkg_server = { Server: __s2_server };   // named export `Server`
  // --- Slice 6.6: damage module (Damage.onPre + block-scoped DamageInfo over the current CTakeDamageInfo).
  //     CTakeDamageInfo is a Source 2 engine type (not CS2-specific) -> engine-generic, lives in core. ---
  function DamageInfo() {}
  function __s2_dmg_ref(field) {
    var o = __s2_schema_offset("CTakeDamageInfo", field);
    if (o < 0) return null;
    var h = __s2_damage_read_int(o) >>> 0;
    if (h === 0 || h === 0xFFFFFFFF) return null;            // empty/invalid handle
    var d = __s2_handle_adopt(h);
    return d ? new EntityRef(d[0], d[1]) : null;             // books-adopted; dangling/stale -> null
  }
  Object.defineProperties(DamageInfo.prototype, {
    // m_flDamage: read the damage; SETTING it modifies the live info (set to 0 to block).
    damage: {
      get: function () { var o = __s2_schema_offset("CTakeDamageInfo", "m_flDamage"); return o < 0 ? 0 : __s2_damage_read_float(o); },
      set: function (v) { var o = __s2_schema_offset("CTakeDamageInfo", "m_flDamage"); if (o >= 0) __s2_damage_write_float(o, +v); },
      enumerable: true, configurable: true,
    },
    damageType: {
      get: function () { var o = __s2_schema_offset("CTakeDamageInfo", "m_bitsDamageType"); return o < 0 ? 0 : __s2_damage_read_int(o); },
      enumerable: true, configurable: true,
    },
    attacker:  { get: function () { return __s2_dmg_ref("m_hAttacker"); },  enumerable: true, configurable: true },
    inflictor: { get: function () { return __s2_dmg_ref("m_hInflictor"); }, enumerable: true, configurable: true },
    // The victim (the entity taking damage) — decoded from the detour `this`, not a field of the info.
    victim: {
      get: function () {
        var h = __s2_damage_victim() >>> 0;
        if (h === 0 || h === 0xFFFFFFFF) return null;
        var d = __s2_handle_adopt(h);
        return d ? new EntityRef(d[0], d[1]) : null;
      }, enumerable: true, configurable: true,
    },
  });
  var Damage = { onPre: function (handler) { return __s2_damage_subscribe(handler); } };
  globalThis.__s2pkg_damage = { Damage: Damage, DamageInfo: DamageInfo };
  // --- Usercmd primitive Task 4: @s2script/usercmd (UserCmd.onRun + the SINGLETON block-scoped Cmd).
  //     The per-tick input fields are Source2-shared (usercmd.proto) -> engine-generic, lives in core.
  //     Field enum (0 forwardMove/1 sideMove/2 upMove/3 pitch/4 yaw/5 roll/6 impulse)
  //     matches the Task-3 shim ops exactly; only the shim maps it onto CS2's protobuf nesting — no
  //     CS2/protobuf name appears here. Cmd is ONE shared object (MF-3, the DamageInfo precedent does
  //     NOT apply — dispatch_usercmd fetches this exact singleton, not a per-handler `new Cmd()`); its
  //     accessors read/write the CURRENT usercmd via the natives and are valid only during dispatch. ---
  var Cmd = {
    get forwardMove() { return __s2_usercmd_read(0); },
    set forwardMove(v) { __s2_usercmd_write(0, +v); },
    get sideMove() { return __s2_usercmd_read(1); },
    set sideMove(v) { __s2_usercmd_write(1, +v); },
    get upMove() { return __s2_usercmd_read(2); },
    set upMove(v) { __s2_usercmd_write(2, +v); },
    get impulse() { return __s2_usercmd_read(6); },
    set impulse(v) { __s2_usercmd_write(6, +v); },
    // 64-bit pressed-button mask — a real bigint end-to-end (never a decimal string), per spec.
    get buttons() { return __s2_usercmd_read_buttons(); },
    set buttons(v) { __s2_usercmd_write_buttons(v); },
    // Fields 3/4/5 (pitch/yaw/roll), read/written as one QAngle {x,y,z}.
    get viewAngles() { return new QAngle(__s2_usercmd_read(3), __s2_usercmd_read(4), __s2_usercmd_read(5)); },
    set viewAngles(a) { __s2_usercmd_write(3, +a.x); __s2_usercmd_write(4, +a.y); __s2_usercmd_write(5, +a.z); },
    clearSubtickMoves: function () { __s2_usercmd_clear_subtick(); },
  };
  var UserCmd = { onRun: function (handler) { return __s2_usercmd_subscribe(handler); } };
  // Cmd is exposed on the package object (NOT as a plugin-facing named export — it's a type-only
  // interface in index.d.ts) purely so dispatch_usercmd (core-side) can fetch this exact singleton
  // via globalThis.__s2pkg_usercmd.Cmd each dispatch.
  globalThis.__s2pkg_usercmd = { UserCmd: UserCmd, HookResult: globalThis.HookResult, Cmd: Cmd };
  // --- Ray-trace slice: @s2script/trace (Trace.line/ray/hull -> TraceHit, TraceMask). ENGINE-GENERIC
  //     (Source-2 physics) — over the single __s2_trace native (the trace_shape engine op). ---
  (function () {
    // InteractionLayers bit positions (mirrors shim/src/trace.h's kLayer* constexprs exactly; all
    // bit positions <=21, well within a JS/Rust-safe 32-bit range).
    var L_SOLID       = 1 << 0;
    var L_HITBOXES    = 1 << 1;
    var L_PLAYERCLIP  = 1 << 4;
    var L_NPCCLIP     = 1 << 5;
    var L_WINDOW      = 1 << 12;
    var L_PASSBULLETS = 1 << 13;
    var L_PLAYER      = 1 << 18;
    var L_NPC         = 1 << 19;
    var L_PHYSICSPROP = 1 << 21;
    var SHOT_PHYSICS = L_SOLID | L_PLAYERCLIP | L_WINDOW | L_PASSBULLETS | L_PLAYER | L_NPC | L_PHYSICSPROP;
    var TraceMask = {
      ShotPhysics: SHOT_PHYSICS,                          // world + player-clip + windows + players/NPCs/props (default)
      ShotHitbox:  L_HITBOXES | L_PLAYER | L_NPC,          // hitboxes only (headshot-style detection)
      ShotFull:    SHOT_PHYSICS | L_HITBOXES,              // physics + hitboxes (a full bullet trace)
      WorldOnly:   L_SOLID | L_WINDOW | L_PASSBULLETS,     // world geometry only, no entities
      Grenade:     L_SOLID | L_WINDOW | L_PHYSICSPROP | L_PASSBULLETS,
      BrushOnly:   L_SOLID | L_WINDOW,                     // brushes only, no clip volumes/entities
      PlayerMove:  L_SOLID | L_WINDOW | L_PLAYERCLIP | L_PASSBULLETS,
      NPCMove:     L_SOLID | L_WINDOW | L_NPCCLIP | L_PASSBULLETS,
    };
    function ignoreOf(opts) {
      var e = opts && opts.ignoreEntity;
      return (e && typeof e.index === "number" && typeof e.id === "number")
        ? { idx: e.index, id: e.id } : { idx: -1, id: 0 };
    }
    function maskOf(opts) { return (opts && typeof opts.mask === "number") ? opts.mask : TraceMask.ShotPhysics; }
    function excludeOf(opts) { return (opts && typeof opts.exclude === "number") ? opts.exclude : 0; }
    var Trace = {
      line: function (start, end, opts) {
        var ig = ignoreOf(opts);
        return __s2_trace(
          [start.x, start.y, start.z], [end.x, end.y, end.z], [0, 0, 0], [0, 0, 0],
          maskOf(opts), excludeOf(opts), ig.idx, ig.id
        );
      },
      ray: function (start, angles, distance, opts) {
        var f = __s2pkg_math.forwardVector(angles);
        var end = { x: start.x + f.x * distance, y: start.y + f.y * distance, z: start.z + f.z * distance };
        return Trace.line(start, end, opts);
      },
      hull: function (start, end, mins, maxs, opts) {
        var ig = ignoreOf(opts);
        return __s2_trace(
          [start.x, start.y, start.z], [end.x, end.y, end.z], [mins.x, mins.y, mins.z], [maxs.x, maxs.y, maxs.z],
          maskOf(opts), excludeOf(opts), ig.idx, ig.id
        );
      },
    };
    globalThis.__s2pkg_trace = { Trace: Trace, TraceMask: TraceMask };
  })();
  // --- Slice 6.12: plugin management (list / load / unload / reload — the SM `sm plugins` backend).
  //     Mutations are DEFERRED to the frame drain (the natives only enqueue), so this is safe from a command. ---
  var __s2_plugins = {
    list: function () { try { return JSON.parse(__s2_plugins_list()); } catch (e) { return []; } },
    unload: function (id) { return __s2_plugin_unload(String(id)); },   // false if not loaded
    reload: function (id) { return __s2_plugin_reload(String(id)); },   // false if id unknown
    load: function (id) { return __s2_plugin_load(String(id)); },       // false if not currently unloaded
  };
  globalThis.__s2pkg_plugins = { Plugins: __s2_plugins };
  // --- @s2script/sdk/unsafe — plugin-declared engine calls. A THIN shim by design: core registered
  //     every descriptor at plugin load from the packed gamedata.json and owns all marshalling, so
  //     this layer only asks by NAME. `call()` guards ONCE (at factory time) and hands back a plain
  //     callable or null, keeping call sites clean.
  //     The plugin id is NEVER passed from here: each native reads the CALLING CONTEXT's id itself,
  //     so this layer cannot name another plugin even by mistake. The callable resolves that id at
  //     invoke time rather than factory time, which is the same context either way — a function
  //     cannot cross the plugin boundary (inter-plugin payloads are JSON structured copies). ---
  globalThis.__s2pkg_unsafe = {
    Engine: {
      call: function (name) {
        if (!__s2_engine_call_ready(name)) return null;
        // A receiverless call (`receiver.kind: "none"` — a static engine function) takes NO leading
        // `self`: shifting one off would silently eat the first real argument.
        if (__s2_engine_call_receiverless(name)) {
          return function () {
            return __s2_engine_call_invoke(name, 0, 0, Array.prototype.slice.call(arguments));
          };
        }
        return function () {
          var args = Array.prototype.slice.call(arguments);
          var self = args.shift();
          if (!self) return null;          // no receiver -> no-op (never a call on a null `this`)
          return __s2_engine_call_invoke(name, self.index, self.id, args);
        };
      },
      status: function (name) { return __s2_engine_call_status(name); },
      // Inbound sibling of `call`. The owner is NEVER passed from here: each native reads the
      // CALLING CONTEXT's id itself, so this layer cannot name another plugin even by mistake.
      hook: function (name) {
        if (!__s2_engine_hook_ready(name)) return null;
        return function (handler) { return __s2_engine_hook_on(name, handler); };
      },
      hookStatus: function (name) { return __s2_engine_hook_status(name); },
    },
  };
  // --- Slice 6.1/6.2: commands module (register / registerServer / registerAdmin) ---
  // Console output carries NO control bytes: a chat colour control byte is in the C0 range, so a
  // coloured message printed to a developer console renders as garbage (or, for \x09/\x0A/\x0D, as
  // stray whitespace). Strip the whole C0 range + DEL with NO \t/\n/\r exemption — those three
  // codepoints ARE colours. Engine-generic: "a console line carries no control bytes" holds on any
  // Source 2 game, so core learns nothing game-specific here.
  // Expand colour tags, then drop every control byte. Expansion must come first: an unexpanded
  // "{green}" is not a control byte and would survive the strip as literal text on an rcon reply.
  function __s2cmd_stripCtl(s) { return globalThis.__s2_colors.consoleLine(s); }
  // ReplySource (core/src/commands.rs) → the JS name. Index order is load-bearing: it matches the
  // enum's discriminants (Server = 0, Console = 1, Chat = 2).
  var __s2cmd_SRC = ["server", "console", "chat"];
  // Normalise whatever the dispatch path handed us. Rust sends the numeric discriminant; a JS caller
  // (Commands.dispatch) may pass the string, or nothing at all. Anything unrecognised falls back to
  // the slot: the server console, else that player's own console (SM FakeClientCommand parity).
  function __s2cmd_srcName(src, s) {
    if (typeof src === "string" && __s2cmd_SRC.indexOf(src) >= 0) return src;
    if (typeof src === "number" && __s2cmd_SRC[src | 0]) return __s2cmd_SRC[src | 0];
    return s < 0 ? "server" : "console";
  }
  function __s2cmd_ctx(slot, argString, src) {
    var s = (slot | 0);
    var replySource = __s2cmd_srcName(src, s);
    var raw = String(argString == null ? "" : argString);
    var args = raw.length ? raw.split(/\s+/).filter(function (a) { return a.length; }) : [];
    var ctx = {
      callerSlot: s,
      replySource: replySource,                    // "server" | "console" | "chat" — set by the dispatch path
      args: args,                                  // 0-based, split on whitespace (kept for compat)
      argString: raw,                              // the full raw arg string (SM GetCmdArgString)
      argCount: args.length,                       // SM GetCmdArgs
      // SM-parity argument retrieval so commands don't hand-roll a parser (0-based; the command name is NOT arg 0).
      arg: function (n) { var a = args[n | 0]; return a == null ? "" : a; },                 // "" if absent (SM GetCmdArg)
      argInt: function (n, fb) { var v = parseInt(args[n | 0], 10); return isNaN(v) ? (fb === undefined ? 0 : fb) : v; },
      argFloat: function (n, fb) { var v = parseFloat(args[n | 0]); return isNaN(v) ? (fb === undefined ? 0 : fb) : v; },
      argsFrom: function (n) { return args.slice(n | 0).join(" "); },   // the rest, re-joined (a reason/value that spans spaces)
      // Force the reply into the caller's CHAT (SM PrintToChat). DEFERRED one frame: for a
      // chat-triggered command (!cmd) the command runs in the Host_Say PRE-hook, before the player's
      // command text is broadcast, so a synchronous reply would land BEFORE their "!slap …" line
      // (jarring). nextFrame lands it after. Sent RAW — colour is content the caller owns. The server
      // (s < 0) has no chat channel, so it degrades to the server console.
      replyToChat: function (m) {
        if (s < 0) { console.log(__s2cmd_stripCtl(m)); return; }
        var msg = String(m);
        globalThis.__s2pkg_timers.nextFrame().then(function () { globalThis.__s2pkg_chat.Chat.toSlot(s, msg); });
      },
      // Force the reply into the caller's developer CONSOLE (SM PrintToConsole). Immediate — there is
      // no chat-broadcast ordering to dodge. Control bytes are stripped; the trailing newline matches
      // Client.print (the native adds none). The server (s < 0) prints to the server console.
      replyToConsole: function (m) {
        var msg = __s2cmd_stripCtl(m);
        if (s < 0) { console.log(msg); return; }
        __s2_client_console_print(s, msg + "\n");
      },
      // SM ReplyToCommand: answer in the channel the caller used. "chat" → their chat; "console" →
      // their developer console; "server" → the server console (replyToConsole's s < 0 branch, which
      // is exactly that row). Chat.color's global prefix lives inside Chat.toSlot, so it applies to
      // the chat path ONLY and never decorates a console reply.
      reply: function (m) {
        if (replySource === "chat") { ctx.replyToChat(m); return; }
        ctx.replyToConsole(m);
      },
      // Localized reply: translate `key` for the CALLER's language, then reply (SM's %t on the reply path).
      // Soft-deps @s2script/translations — degrades to the key if translations isn't loaded.
      replyT: function (key) {
        var t = globalThis.__s2pkg_translations;
        if (!t) { ctx.reply(String(key)); return; }
        ctx.reply(t.Translations.translate.apply(t.Translations, [s, key].concat([].slice.call(arguments, 1))));
      },
    };
    return ctx;
  }
  // Slice 6.11: a per-context registry of wrapped dispatch fns (name -> function(slot, argString)), so a
  // command can be invoked BY NAME (chat triggers) reusing the SAME wrapper as the ConCommand path (admin
  // gating included). __s2cmd_add both registers the engine ConCommand and records the wrapper here.
  var __s2cmd_reg = {};
  // `flags` (default 0) records the required admin mask for Commands.list()/sm_help: 0 = anyone,
  // -1 = console/server-only sentinel, else the ADMFLAG bit mask. Passed through to the __s2_concommand native.
  function __s2cmd_add(name, wrapped, flags) { __s2cmd_reg[name] = wrapped; __s2_concommand(name, wrapped, flags | 0); }
  var __s2cmd_triggers = { public: "!", silent: "/" };   // SM PublicChatTrigger / SilentChatTrigger; mutable
  var __s2_commands = {
    register: function (name, handler) {
      __s2cmd_add(name, function (slot, a, src) { return handler(__s2cmd_ctx(slot, a, src)); }, 0);   // 0 = anyone
    },
    registerServer: function (name, handler) {
      __s2cmd_add(name, function (slot, a, src) {
        var ctx = __s2cmd_ctx(slot, a, src);
        if (ctx.callerSlot < 0) { return handler(ctx); }
        ctx.reply("[SM] This command can only be run from the server console.");
      }, -1);   // -1 = console/server-only sentinel
    },
    registerAdmin: function (name, flags, handler) {
      __s2cmd_add(name, function (slot, a, src) {
        var ctx = __s2cmd_ctx(slot, a, src);
        if (ctx.callerSlot < 0) { return handler(ctx); }        // server / rcon = root
        var check = globalThis.__s2_admin_check;
        if (typeof check !== "function") {
          if (!globalThis.__s2cmd_warnedNoAdmin) { globalThis.__s2cmd_warnedNoAdmin = true;
            console.log("[s2script] WARN: registerAdmin('" + name + "') used without @s2script/admin — denying non-server callers"); }
          ctx.reply("[SM] You do not have access to this command."); return;
        }
        if (check(ctx.callerSlot, flags | 0, name)) { return handler(ctx); }
        ctx.reply("[SM] You do not have access to this command.");
      }, flags | 0);   // the ADMFLAG mask this command requires
    },
    // Slice 6.11: invoke a registered command by name (same context, synchronous — the wrapper applies
    // gating). Returns true if the command exists in this plugin. Used by chat triggers.
    // `src` is optional: omitted (or unrecognised) it falls back to the slot in __s2cmd_srcName —
    // the server console at -1, else that player's console, which is SM's FakeClientCommand
    // behaviour. Pass "chat" when re-dispatching from a chat context.
    dispatch: function (name, slot, argString, src) {
      var w = __s2cmd_reg[name];
      if (!w) return false;
      w(slot | 0, String(argString == null ? "" : argString), src);
      return true;
    },
    // Parse a chat message for a trigger. Returns { silent, name, argString } or null (not a trigger).
    parseChatTrigger: function (message) {
      var m = String(message == null ? "" : message);
      var silent;
      if (__s2cmd_triggers.silent && m.charAt(0) === __s2cmd_triggers.silent) silent = true;
      else if (__s2cmd_triggers.public && m.charAt(0) === __s2cmd_triggers.public) silent = false;
      else return null;
      var body = m.slice(1).replace(/^\s+/, "");
      if (!body.length) return null;
      var sp = body.search(/\s/);
      return { silent: silent, name: sp < 0 ? body : body.slice(0, sp),
               argString: sp < 0 ? "" : body.slice(sp + 1).replace(/^\s+/, "") };
    },
    // Handle a chat message end-to-end: if it's a trigger, dispatch the command (trying `name` then
    // `sm_<name>`, the SM convention) with `slot` as the caller. Returns { silent, ran } if it WAS a
    // trigger (the caller should suppress the chat), or null if it was ordinary chat.
    handleChatTrigger: function (slot, message) {
      var t = __s2_commands.parseChatTrigger(message);
      if (!t) return null;
      var ran = __s2_commands.dispatch(t.name, slot, t.argString, "chat");
      if (!ran && t.name.indexOf("sm_") !== 0) ran = __s2_commands.dispatch("sm_" + t.name, slot, t.argString, "chat");
      return { silent: t.silent, ran: ran };
    },
    triggers: __s2cmd_triggers,   // { public: "!", silent: "/" } — reconfigure the trigger chars here
    // List every globally-registered ConCommand + its required admin flags (0 = anyone, -1 = console-only,
    // else the ADMFLAG bit mask) — the sm_help backend. Degrades to [] on any error.
    list: function () { try { return JSON.parse(__s2_commands_list()); } catch (e) { return []; } },
    // SourceMod AddCommandListener: observe a client command by name — including one the ENGINE
    // owns, which `register` cannot claim. Handler gets (slot, argString); return >= HookResult
    // .Handled to stop the engine handling it, anything else to pass through.
    onClientCommand: function (name, handler) { return __s2_client_command_listen(name, handler); },
  };
  globalThis.__s2pkg_commands = { Commands: __s2_commands };   // named export `Commands`
  // --- admin module (engine-generic; ADMFLAG + Admin API + group/immunity/override resolution) ---
  var __s2_ADMFLAG = {
    RESERVATION: 1<<0, GENERIC: 1<<1, KICK: 1<<2, BAN: 1<<3, UNBAN: 1<<4, SLAY: 1<<5, CHANGEMAP: 1<<6,
    CONVARS: 1<<7, CONFIG: 1<<8, CHAT: 1<<9, VOTE: 1<<10, PASSWORD: 1<<11, RCON: 1<<12, CHEATS: 1<<13, ROOT: 1<<14,
    CUSTOM1: 1<<15, CUSTOM2: 1<<16, CUSTOM3: 1<<17, CUSTOM4: 1<<18, CUSTOM5: 1<<19, CUSTOM6: 1<<20,
  };
  function __s2_hasFlags(flags, req) { return ((flags & __s2_ADMFLAG.ROOT) !== 0) || ((flags & req) === req); }

  // ---- flag-token parsing (a name, a single SM letter, or a compact letter-string) ----
  function __s2_flag_letterBit(ch) {
    if (ch === "z" || ch === "Z") return __s2_ADMFLAG.ROOT;
    var i = String(ch).charCodeAt(0) - 97;                 // 'a'
    if (i >= 0 && i <= 13) return 1 << i;                  // a..n -> RESERVATION..CHEATS
    if (i >= 14 && i <= 19) return 1 << (i + 1);           // o..t -> CUSTOM1..CUSTOM6 (ROOT holds bit 14)
    return 0;
  }
  function __s2_flag_token(tok) {                           // name OR single letter -> bit (0 = unknown)
    var up = String(tok).toUpperCase();
    if (__s2_ADMFLAG[up] != null) return __s2_ADMFLAG[up];
    var s = String(tok);
    return (s.length === 1) ? __s2_flag_letterBit(s) : 0;
  }
  function __s2_parseFlags(value) {                         // array of tokens | a name | a letter-string -> mask
    var mask = 0;
    if (Array.isArray(value)) {
      for (var i = 0; i < value.length; i++) {
        var b = __s2_flag_token(value[i]);
        if (b) mask |= b; else if (String(value[i]).length) console.log("[s2script] WARN: unknown admin flag '" + value[i] + "' — skipped");
      }
    } else if (typeof value === "string") {
      var up = value.toUpperCase();
      if (__s2_ADMFLAG[up] != null) return __s2_ADMFLAG[up];   // the whole string is a flag name
      for (var j = 0; j < value.length; j++) {
        var c = value.charAt(j), lb = __s2_flag_letterBit(c);
        if (lb) mask |= lb; else console.log("[s2script] WARN: unknown admin flag letter '" + c + "' — skipped");
      }
    }
    return mask;
  }
  function __s2_parseOverrideToken(v) {                     // "" -> public; unknown token -> null (skip); else a mask
    if (v === "" || v == null) return { public: true, mask: 0 };
    var m = __s2_parseFlags(v);
    if (!m) return null;                                     // no flag resolved -> invalid override, skip
    return { public: false, mask: m };
  }

  // ---- registries (per-context; populated from the files at prelude time) ----
  var __s2_groups = {};        // name -> { flags, immunity, overrides: {cmd:{public,mask}} }
  var __s2_adminGroups = {};   // sid  -> [groupName]

  function __s2_admin_parseGroups(text) {
    __s2_groups = {};
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admin_groups.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var name in obj) {
      if (name === "_help" || !Object.prototype.hasOwnProperty.call(obj, name)) continue;
      var g = obj[name]; if (!g || typeof g !== "object") continue;
      var ov = {};
      if (g.overrides && typeof g.overrides === "object")
        for (var cmd in g.overrides) if (Object.prototype.hasOwnProperty.call(g.overrides, cmd)) {
          var ot = __s2_parseOverrideToken(g.overrides[cmd]);
          if (ot) ov[cmd] = ot;
          else console.log("[s2script] WARN: group '" + name + "' override '" + cmd + "': unknown flag '" + g.overrides[cmd] + "' — skipped");
        }
      __s2_groups[name] = { flags: __s2_parseFlags(g.flags), immunity: (typeof g.immunity === "number") ? (g.immunity | 0) : 0, overrides: ov };
    }
  }

  function __s2_admin_resolveEntry(entry) {                 // -> { mask, immunity, groups:[], overrides:{cmd:{public,mask}} }
    var mask = 0, immunity = 0, groups = [], overrides = {};
    if (Array.isArray(entry)) {
      mask = __s2_parseFlags(entry);
    } else if (entry && typeof entry === "object") {
      if (entry.flags != null) mask |= __s2_parseFlags(entry.flags);
      if (typeof entry.immunity === "number") immunity = Math.max(immunity, entry.immunity | 0);
      if (Array.isArray(entry.groups)) for (var i = 0; i < entry.groups.length; i++) {
        var gn = entry.groups[i], g = __s2_groups[gn];
        if (!g) { console.log("[s2script] WARN: admins.json references unknown group '" + gn + "' — skipped"); continue; }
        mask |= g.flags; immunity = Math.max(immunity, g.immunity); groups.push(gn);
        for (var c in g.overrides) if (Object.prototype.hasOwnProperty.call(g.overrides, c)) overrides[c] = g.overrides[c];
      }
    }
    return { mask: mask, immunity: immunity, groups: groups, overrides: overrides };
  }

  function __s2_admin_parseAdmins(text, pushCore) {
    __s2_adminGroups = {};
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admins.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var sid in obj) {
      if (sid === "_help" || !Object.prototype.hasOwnProperty.call(obj, sid)) continue;
      var r = __s2_admin_resolveEntry(obj[sid]);
      __s2_adminGroups[String(sid)] = r.groups;
      if (pushCore) {
        __s2_admin_set(String(sid), r.mask, r.immunity, false);
        for (var cmd in r.overrides) if (Object.prototype.hasOwnProperty.call(r.overrides, cmd)) {
          var ov = r.overrides[cmd]; __s2_admin_add_override(String(sid), cmd, ov.mask | 0, !!ov.public);
        }
      }
    }
  }

  function __s2_admin_parseOverrides(text) {                // global admin_overrides.json (pushCore path only)
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: admin_overrides.json malformed — ignored"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var cmd in obj) {
      if (cmd === "_help" || !Object.prototype.hasOwnProperty.call(obj, cmd)) continue;
      var ov = __s2_parseOverrideToken(obj[cmd]);
      if (ov) __s2_admin_set_global_override(cmd, ov.mask | 0, !!ov.public);
      else console.log("[s2script] WARN: admin_overrides.json '" + cmd + "': unknown flag '" + obj[cmd] + "' — skipped");
    }
  }

  // Framework templates live in core/config-templates/*.json, injected at globalThis.__s2_TEMPLATES
  // (name -> file-content string) before this prelude runs. ONE source, no inline literals here.
  function __s2_admin_readOrTemplate(name) {
    var t = __s2_config_read_raw(name);
    if (t == null) {
      var template = (globalThis.__s2_TEMPLATES && globalThis.__s2_TEMPLATES[name]) || "{}\n";
      __s2_config_write_raw(name, template);
      return "{}";
    }
    return t;
  }
  function __s2_admin_reloadAll(pushCore) {
    __s2_admin_parseGroups(__s2_admin_readOrTemplate("admin_groups"));
    __s2_admin_parseAdmins(__s2_admin_readOrTemplate("admins"), pushCore);
    if (pushCore) __s2_admin_parseOverrides(__s2_admin_readOrTemplate("admin_overrides"));
  }

  // ---- AdminInfo + the Admin API ----
  function __s2_adminInfo(steamId, flags, immunity) {
    return {
      steamId: String(steamId), flags: flags | 0, immunity: immunity | 0,
      groups: (__s2_adminGroups[String(steamId)] || []).slice(),
      hasFlags: function (req) { return __s2_hasFlags(flags | 0, req | 0); },
    };
  }
  function __s2_canTargetImm(callerSlot, callerImm, targetImm) {   // the pure immunity comparison (test hook)
    if ((callerSlot | 0) < 0) return true;                        // server console / rcon = infinite
    if ((targetImm | 0) <= 0) return true;                        // non-immune target
    return (callerImm | 0) >= (targetImm | 0);                    // SM default: equal can target
  }
  var __s2_admin = {
    add: function (steamId, flags, immunity) { __s2_admin_set(String(steamId), flags | 0, immunity | 0, true); },
    remove: function (steamId) { __s2_admin_remove(String(steamId), true); },
    get: function (steamId) {
      var sid = String(steamId), m = __s2_admin_get(sid), im = __s2_admin_get_immunity(sid);
      if (!m && !im) return null;
      return __s2_adminInfo(sid, m, im);
    },
    forSlot: function (slot) {
      var sid = __s2_client_steamid(slot | 0);
      if (sid === "0" || !sid) return null;                        // bot / mid-auth -> never an admin
      return __s2_admin.get(sid);
    },
    canTarget: function (callerSlot, targetSlot) {
      var t = __s2_admin.forSlot(targetSlot | 0), ti = t ? t.immunity : 0;
      var c = __s2_admin.forSlot(callerSlot | 0), ci = c ? c.immunity : 0;
      return __s2_canTargetImm(callerSlot | 0, ci, ti);
    },
    getGroup: function (name) {
      var g = __s2_groups[String(name)];
      // Shallow-copy overrides (like `groups`' .slice() above) so a caller can't mutate this
      // context's group registry through the returned object.
      return g ? { name: String(name), flags: g.flags, immunity: g.immunity, overrides: Object.assign({}, g.overrides) } : null;
    },
    // NOTE: reload() clears + re-reads the SHARED core cache (admin flags/immunity/overrides), so
    // ENFORCEMENT (hasFlags/canTarget/__s2_admin_check) refreshes everywhere immediately. But the
    // per-context JS group registries (__s2_groups/__s2_adminGroups below) are only re-parsed in THIS
    // context — another already-loaded context's `.groups` / `getGroup` DISPLAY metadata can be stale
    // until that context reloads (e.g. on its own next file-watch reload). Enforcement is unaffected.
    reload: function () { __s2_admin_clear_file(); __s2_admin_reloadAll(true); },
  };

  // test hooks (safe to expose; pure helpers)
  globalThis.__s2_admin_parseFlags = __s2_parseFlags;
  globalThis.__s2_admin_parseGroups = __s2_admin_parseGroups;
  globalThis.__s2_admin_parseAdmins = __s2_admin_parseAdmins;
  globalThis.__s2_admin_resolveEntry = __s2_admin_resolveEntry;
  globalThis.__s2_canTargetImm = __s2_canTargetImm;

  // Parse the registries in EVERY context (cheap, idempotent — makes getGroup / AdminInfo.groups work
  // everywhere); push the resolved admins + overrides into the shared core cache ONCE (first context).
  // See the reload/staleness note on Admin.reload above: a later reload() in one context does not
  // re-run this per-context parse in every OTHER already-loaded context.
  __s2_admin_reloadAll(!__s2_admin_mark_loaded());

  // Override-aware gating hook. A "public" override (flag "") grants ANYONE — even a non-admin; a flag
  // override changes the requirement; else the command's default mask. (registerAdmin already lets
  // callerSlot<0 / console through as root before reaching here.)
  globalThis.__s2_admin_check = function (slot, requiredMask, cmdName) {
    var sid = __s2_client_steamid(slot | 0);
    var ov = cmdName ? __s2_admin_override(sid || "", String(cmdName)) : "";
    if (ov === "public") return true;
    var a = __s2_admin.forSlot(slot | 0);
    if (!a) return false;
    if (ov !== "") return a.hasFlags(parseInt(ov, 10) | 0);
    return a.hasFlags(requiredMask | 0);
  };
  // Immunity targeting hook (consumed by the CS2 Player.target immunity filter, without importing this module).
  globalThis.__s2_admin_can_target = function (cs, ts) { return __s2_admin.canTarget(cs | 0, ts | 0); };
  globalThis.__s2pkg_admin = { ADMFLAG: __s2_ADMFLAG, Admin: __s2_admin };
  // --- Slice 6.18: bans module (engine-generic; SteamID64 ban store + bans.json persistence via the config bridge) ---
  // Parse bans.json ({ "<steamid64>": { until:<unix|0>, reason:"<str>" } }) into BAN_CACHE. `_help`/non-object
  // entries are skipped. Malformed JSON → silent skip (degrade-never-crash; the file may be hand-edited).
  function __s2_ban_parseFile(text) {
    var obj; try { obj = JSON.parse(text); } catch (e) { return; }
    if (!obj || typeof obj !== "object") return;
    for (var sid in obj) {
      if (sid === "_help" || !Object.prototype.hasOwnProperty.call(obj, sid)) continue;
      var e = obj[sid];
      if (!e || typeof e !== "object") continue;
      var until = (typeof e.until === "number") ? e.until : 0;
      var reason = (typeof e.reason === "string") ? e.reason : "";
      __s2_ban_set(String(sid), until, reason);
    }
  }
  function __s2_ban_load() {
    var text = __s2_config_read_raw("bans");
    if (text == null) {
      // A VALID-JSON self-documenting template (the "_help" key is a string, so parseFile skips it; it
      // round-trips through JSON.parse cleanly — a //-commented template would fail the next-restart parse).
      __s2_config_write_raw("bans", '{\n  "_help": "SteamID64 -> { until: <unix seconds, 0 = permanent>, reason }. Managed by sm_ban/sm_unban."\n}\n');
      text = __s2_config_read_raw("bans");
      if (text == null) return;
    }
    __s2_ban_parseFile(text);
  }
  function __s2_ban_rewrite() {
    var list = JSON.parse(__s2_ban_list());
    var obj = {};
    for (var i = 0; i < list.length; i++) obj[list[i].steamid] = { until: list[i].until, reason: list[i].reason };
    __s2_config_write_raw("bans", JSON.stringify(obj, null, 2) + "\n");
  }
  var __s2_bans = {
    add: function (steamId, minutes, reason) {
      var until = (minutes > 0) ? (Math.floor(Date.now() / 1000) + Math.floor(minutes) * 60) : 0;
      __s2_ban_set(String(steamId), until, reason ? String(reason) : "");
      __s2_ban_rewrite();
    },
    remove: function (steamId) { var r = __s2_ban_remove(String(steamId)); __s2_ban_rewrite(); return r; },
    get: function (steamId) { var s = __s2_ban_get(String(steamId)); return s ? JSON.parse(s) : null; },
    list: function () { return JSON.parse(__s2_ban_list()); },
    reload: function () { __s2_ban_clear(); __s2_ban_load(); },
  };
  // Expose parseFile on globalThis so plugins (and tests) can call it directly (mirrors how the admin module exposes its parser hooks).
  globalThis.__s2_ban_parseFile = __s2_ban_parseFile;
  // One-shot file load (first plugin to import @s2script/bans triggers this).
  if (!__s2_ban_mark_loaded()) { __s2_ban_load(); }
  globalThis.__s2pkg_bans = { Bans: __s2_bans };   // named export `Bans`
  // --- Clients sub-project: @s2script/clients (engine-generic slot-backed Client + lifecycle events).
  //     Client wraps only EXISTING client_* natives (no new engine primitive); Clients.on* subscribe via
  //     __s2_client_subscribe and construct a Client from the dispatched slot. Identity = slot (a client's
  //     slot is stable for its connection; a reused slot is a fresh onConnect). ---
  function Client(slot) { this.slot = slot | 0; }
  Client.prototype.isValid = function () { return __s2_client_valid(this.slot); };
  Object.defineProperty(Client.prototype, "steamId",     { get: function () { return __s2_client_steamid(this.slot); } });
  Object.defineProperty(Client.prototype, "name",        { get: function () { var n = __s2_client_name(this.slot); return n == null ? "" : n; } });
  Object.defineProperty(Client.prototype, "userId",      { get: function () { return __s2_client_userid(this.slot); } });
  Object.defineProperty(Client.prototype, "signonState", { get: function () { return __s2_client_signon(this.slot); } });
  Object.defineProperty(Client.prototype, "isBot",       { get: function () { return __s2_client_steamid(this.slot) === "0"; } });
  Client.prototype.kick = function (reason)  { __s2_client_kick(this.slot, reason == null ? "" : String(reason)); };
  Client.prototype.chat = function (message) { __s2_client_print(this.slot, String(message)); };
  Client.prototype.print = function (msg) { __s2_client_console_print(this.slot, String(msg) + "\n"); };
  // Client command execution (SourceMod ClientCommand parity): ask the CLIENT to run it. The
  // server-side FakeClientCommand variant is NOT here — see the spec: it needs a CCommand, whose
  // ctor and Tokenize are not exported by any shipped engine binary.
  Client.prototype.command = function (cmd) { return __s2_client_command(this.slot, String(cmd)); };
  Client.prototype.fakeCommand = function (cmd) { return __s2_client_fake_command(this.slot, String(cmd)); };
  Object.defineProperty(Client.prototype, "ip", { get: function () {
    var a = __s2_client_address(this.slot); if (!a) return ""; var i = a.indexOf(":"); return i < 0 ? a : a.slice(0, i);
  } });
  // Voice-control slice: server-side voice mute (this client's OUTGOING voice silenced for every
  // receiver — the shim's SetClientListening rewrite). Framework state: cleared on disconnect. When
  // the voice descriptor is degraded the setter is an inert no-op (shim logs the named reason) and
  // reads stay false (get_muted -1/0 both map to false).
  Object.defineProperty(Client.prototype, "voiceMuted", {
    get: function () { return __s2_voice_get_muted(this.slot) === 1; },
    set: function (on) { __s2_voice_set_muted(this.slot, !!on); }
  });
  var __s2_MAX_CLIENTS = 64;
  function __s2_client_on(event, h) { return __s2_client_subscribe(event, function (slot) { return h(new Client(slot)); }); }
  var __s2_clients = {
    onConnect:         function (h) { return __s2_client_on("connect", h); },
    onPutInServer:     function (h) { return __s2_client_on("putinserver", h); },
    onActive:          function (h) { return __s2_client_on("active", h); },
    onFullyConnect:    function (h) { return __s2_client_on("fullyconnect", h); },
    onDisconnect:      function (h) { return __s2_client_on("disconnect", h); },
    onSettingsChanged: function (h) { return __s2_client_on("settingschanged", h); },
    // Fires while a client transmits voice (throttled shim-side to <=1 dispatch/slot/second; the FIRST
    // packet of a transmission always fires). Never fires for bots.
    onVoice:           function (h) { return __s2_client_on("voice", h); },
    fromSlot: function (slot) { slot = slot | 0; return __s2_client_valid(slot) ? new Client(slot) : null; },
    all: function () { var out = []; for (var s = 0; s < __s2_MAX_CLIENTS; s++) { if (__s2_client_valid(s)) out.push(new Client(s)); } return out; }
  };
  var __s2_pendingKicks = {};
  var __s2_kickWired = false;
  // Deliver the reason to the client REPEATEDLY (chat + console, once per second) so they see it even if
  // they were mid-load, then kick on the final tick. Re-resolves the client each tick — stops if they left.
  function __s2_deliverAndKick(slot, reason, remaining) {
    var c = __s2_clients.fromSlot(slot);
    if (!c) return;                                          // already gone → nothing to do
    if (remaining <= 0) { c.kick(reason); return; }          // time's up → kick
    c.chat(reason); c.print(reason);                         // show in chat AND console, each second
    globalThis.__s2pkg_timers.delay(1000).then(function () { __s2_deliverAndKick(slot, reason, remaining - 1); });
  }
  function __s2_deliverPending(slot) {
    var p = __s2_pendingKicks[slot]; if (!p) return;
    delete __s2_pendingKicks[slot];
    __s2_deliverAndKick(slot, p.reason, Math.max(1, Math.round(p.delay)));
  }
  function __s2_wireKick() {
    if (__s2_kickWired) return; __s2_kickWired = true;
    __s2_client_on("active", function (c) { __s2_deliverPending(c.slot); });          // reconnect path: deliver once in-game
    __s2_client_on("disconnect", function (c) { delete __s2_pendingKicks[c.slot]; }); // left before active → drop
  }
  // Show a reason in chat + console (repeated once per second) then kick after ~delaySeconds. Works on an
  // ALREADY-in-game client (e.g. sm_ban — delivered immediately) AND from onConnect (deferred until the
  // client is in-game so they can actually see it). signonState >= 4 = past the connection handshake / in
  // the server (a still-connecting client is at CONNECTED=2), so it can receive messages now.
  Client.prototype.kickWithReason = function (reason, delaySeconds) {
    __s2_wireKick();
    var r = String(reason);
    var d = Math.max(1, Math.round(delaySeconds == null ? 5 : delaySeconds));
    if (this.signonState >= 4) { __s2_deliverAndKick(this.slot, r, d); }          // in-game now → deliver immediately
    else { __s2_pendingKicks[this.slot] = { reason: r, delay: d }; }              // still connecting → deliver at onActive
  };
  globalThis.__s2pkg_clients = { Client: Client, Clients: __s2_clients };   // named exports Client + Clients
  // --- @s2script/sound — engine-generic sound (Sound slice). A soundevent NAME, a recipient slot
  //     set, and a precache resource path are Source2-generic; CS2 soundevent names live in the
  //     game layer (games/cs2/js/pawn.js `Sounds`), never here.
  //     emit: no entity -> worldspawn (index 0, serial sentinel -1 = no serial gate) = a global/2D
  //     sound; no recipients -> every valid client slot (bot slots are additionally skipped
  //     shim-side — no netchannel). Returns the engine SndOpEventGuid (nonzero) or 0.
  //     onPrecache: handler(ctx) gets a BLOCK-SCOPED PrecacheContext — ctx.add(path) is valid only
  //     during the dispatch (the shim's manifest stash is live only then; a stashed ctx used after
  //     the handler returns is a no-op false). Fires at map load / mapchange. ---
  var __s2_sound = {
    emit: function (name, opts) {
      opts = opts || {};
      var idx = 0, id = 0;                          // worldspawn / global-2D default (id 0 = no serial gate)
      var e = opts.entity;
      if (e && typeof e.index === "number" && typeof e.id === "number") {
        idx = e.index | 0; id = e.id;              // raw number (no | 0 — a host-id can exceed 2^31)
      }
      var slots = opts.recipients;
      if (!Array.isArray(slots)) {
        slots = [];
        for (var s = 0; s < __s2_MAX_CLIENTS; s++) if (__s2_client_valid(s)) slots.push(s);
      }
      var vol = (opts.volume == null) ? 1.0 : +opts.volume;
      return __s2_sound_emit(String(name), idx, id, slots, vol);
    },
    // stop(name, opts) — the counterpart to emit. UNLIKE emit, an entity is REQUIRED: the engine
    // call behind this is an instance method on the entity, reached through the books-gated entity
    // resolve, so there is no global/2D form to default to the way emit falls back to worldspawn.
    // Returns false with no entity, on a stale ref, or when the op is unavailable.
    stop: function (name, opts) {
      var e = (opts || {}).entity;
      if (!e || typeof e.stopSound !== "function") return false;
      return e.stopSound(name);
    },
    onPrecache: function (h) {
      return __s2_precache_subscribe(function () {
        h({ add: function (p) { return __s2_sound_precache_add(String(p)); } });
      });
    },
  };
  globalThis.__s2pkg_sound = { Sound: __s2_sound };   // named export `Sound`
  // --- Menu primitive Task 2: chat renderer (registers against @s2script/menu's registerRenderer seam)
  //     + disconnect-close lifecycle. Placed here (not immediately after the Task 1 model) because both
  //     blocks below make IMMEDIATE top-level calls into __s2pkg_menu / __s2pkg_chat / __s2pkg_clients
  //     (Menu.registerRenderer(...) and Clients.onDisconnect(...) run at prelude-eval time, not lazily),
  //     so they must run after all three are assigned to globalThis (menu @776, chat @794, clients above). ---
  // Chat renderer: paints numbered lines via __s2pkg_chat; one shared onMessage sub captures picks.
  (function () {
    var HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;
    var chatSessions = {};      // slot -> session (chat menus only)
    var subInstalled = false;
    function ensureSub() {
      if (subInstalled) return; subInstalled = true;
      __s2_chat_on_message(function (slot, text, teamonly) {
        var s = chatSessions[slot];
        if (!s || s._ended) return;                 // no menu for this slot -> pass through
        var t = ("" + text).trim();
        if (!/^[0-9]$/.test(t)) return;             // not a single digit -> pass through (chat shows)
        s.pickNumber(parseInt(t, 10));
        return HANDLED;                              // swallow the menu pick from public chat
      });
    }
    globalThis.__s2pkg_menu.Menu.registerRenderer(globalThis.__s2pkg_menu.MenuStyle.Chat, {
      open: function (session) { ensureSub(); chatSessions[session.slot] = session; this.update(session); },
      update: function (session) {
        var v = session.view(), C = globalThis.__s2pkg_chat.Chat;
        C.toSlot(session.slot, v.title);
        for (var i = 0; i < v.lines.length; i++) {
          var l = v.lines[i];
          C.toSlot(session.slot, (l.key ? l.key + ". " : "   ") + l.text);
        }
      },
      close: function (slot) { delete chatSessions[slot]; },
    });
  })();

  // Disconnect: close any open menu for a leaving slot.
  globalThis.__s2pkg_clients.Clients.onDisconnect(function (client) {
    var s = __s2_menu_activeBySlot[client.slot];
    if (s) s._end(MenuCancelReason.Disconnect);
  });
  // --- @s2script/db — Database.open/query/execute/close over the built-in drivers. Both SQLite
  //     (__s2_sqlite_*, a per-connection actor thread) and mysql/postgres (__s2_db_remote_*, the
  //     shared tokio+sqlx runtime) run query/execute OFF the game thread and resolve later.
  //     Database.open resolves a name via databases.json (operator-owned; absent name -> sqlite). ---
  var __s2_db_drivers = {};
  var __s2_db_config = {};   // name -> {driver,host,port,user,password,database} from databases.json — IIFE-PRIVATE (credentials; never on globalThis)
  function __s2_db_loadConfig() {
    var text = __s2_config_read_raw("databases");
    if (text == null) {
      var template = (globalThis.__s2_TEMPLATES && globalThis.__s2_TEMPLATES.databases) || "{}\n";
      __s2_config_write_raw("databases", template);
      return;
    }
    var obj; try { obj = JSON.parse(text); } catch (e) { console.log("[s2script] WARN: databases.json malformed — all connections default to sqlite"); return; }
    if (!obj || typeof obj !== "object") return;
    for (var name in obj) {
      if (name === "_help" || !Object.prototype.hasOwnProperty.call(obj, name)) continue;
      var c = obj[name];
      if (c && typeof c === "object" && (c.driver === "mysql" || c.driver === "postgres")) __s2_db_config[name] = c;
      else if (c && typeof c === "object" && c.driver !== "sqlite") console.log("[s2script] WARN: databases.json '" + name + "': unknown driver '" + c.driver + "' — using sqlite");
    }
  }
  function __s2_db_resolveConfig(connName) {
    var c = __s2_db_config[connName];
    if (c) { return { driver: c.driver, name: connName, host: c.host, port: c.port, user: c.user, password: c.password, database: c.database }; }
    return { driver: "sqlite", name: connName };
  }
  // test hooks — secret-free: an injector (sets the private map) + a driver-ONLY (redacted) resolve.
  globalThis.__s2_db_testSetConfig = function (cfg) { __s2_db_config = cfg || {}; };
  globalThis.__s2_db_resolveConfigDriver = function (name) { return __s2_db_resolveConfig(name).driver; };

  __s2_db_drivers["sqlite"] = {
    name: "sqlite",
    connect: function (config) {
      return __s2_sqlite_open(config.name).then(function (handle) {
        return { query: function (s, p) { return __s2_sqlite_query(handle, s, p || []); },
                 execute: function (s, p) { return __s2_sqlite_execute(handle, s, p || []); },
                 close: function () { return __s2_sqlite_close(handle); } };
      });
    },
  };
  function __s2_makeRemoteDriver(driverName) {
    return {
      name: driverName,
      connect: function (config) {
        var handle = __s2_db_remote_connect(JSON.stringify(config));
        if (!handle) return Promise.reject(new Error("could not open " + driverName + " connection '" + config.name + "'"));
        return Promise.resolve({
          query:   function (s, p) { return __s2_db_remote_query(handle, s, p || []); },
          execute: function (s, p) { return __s2_db_remote_execute(handle, s, p || []); },
          close:   function () { return __s2_db_remote_close(handle); },
        });
      },
    };
  }
  __s2_db_drivers["mysql"] = __s2_makeRemoteDriver("mysql");
  __s2_db_drivers["postgres"] = __s2_makeRemoteDriver("postgres");

  __s2_db_loadConfig();

  var __s2_Database = {
    registerDriver: function (driver) { __s2_db_drivers[driver.name] = driver; },
    open: function (name) {
      var connName = name || "default";
      var config = __s2_db_resolveConfig(connName);
      var driver = __s2_db_drivers[config.driver];
      if (!driver) return Promise.reject(new Error("unknown db driver: " + config.driver));
      return driver.connect(config).then(function (conn) {
        return { query: function (s, p) { return conn.query(s, p); },
                 execute: function (s, p) { return conn.execute(s, p); },
                 close: function () { return conn.close(); } };
      });
    },
  };
  globalThis.__s2pkg_db = { Database: __s2_Database };
  // --- @s2script/cookies: SM-parity cookies over the __s2_cookie_* host-global cache ---
  var __s2_cookie_defs = {};   // per-context registry: name -> Cookie (idempotent register)
  var __s2_Cookies = {
    register: function (name, opts) {
      if (__s2_cookie_defs[name]) return __s2_cookie_defs[name];
      opts = opts || {};
      var cookie = { name: name, access: (opts.access == null ? 0 : opts.access), default: (opts.default == null ? "" : String(opts.default)) };
      __s2_cookie_defs[name] = cookie;
      return cookie;
    },
    get: function (client, cookie) {
      if (!client || client.steamId === "0") return cookie.default;      // bots have no cookies
      var v = __s2_cookie_get(client.steamId, cookie.name);
      return v === undefined ? cookie.default : v;   // a stored "" is a hit, not a miss
    },
    set: function (client, cookie, value) {
      if (!client || client.steamId === "0") return;                     // no-op for bots
      __s2_cookie_set(client.steamId, cookie.name, String(value), Math.floor(Date.now() / 1000));
    },
    areCached: function (client) {
      return !!client && client.steamId !== "0" && __s2_cookie_is_cached(client.steamId);
    },
    getTime: function (client, cookie) {
      return (!client || client.steamId === "0") ? 0 : __s2_cookie_get_time(client.steamId, cookie.name);
    },
    setAuthId: function (steamId, cookie, value) {
      if (!steamId || steamId === "0") return;   // no-op for bots
      __s2_cookie_set_authid(String(steamId), cookie.name, String(value), Math.floor(Date.now() / 1000));
    },
    onCached: function (h) {
      // Guard: fromSlot is null if the client disconnected in the load->fan-out window, so only fire
      // for a still-connected Client (the .d.ts promises a non-null Client). A departed client's
      // "cookies cached" notification is moot.
      return __s2_cookie_on_cached(function (slot) { var c = globalThis.__s2pkg_clients.Clients.fromSlot(slot); if (c) h(c); });
    },
  };
  globalThis.__s2pkg_cookies = { Cookies: __s2_Cookies, CookieAccess: { Public: 0, Protected: 1, Private: 2 } };
  // --- @s2script/http: fetch over __s2_fetch (adds text()/json() over the buffered body) ---
  globalThis.__s2pkg_http = {
    fetch: function (url, options) {
      return __s2_fetch(String(url), options || {}).then(function (raw) {
        return {
          status: raw.status, ok: raw.ok, statusText: raw.statusText, headers: raw.headers,
          text: function () { return raw.body; },
          json: function () { return JSON.parse(raw.body); },
        };
      });
    },
  };
  // --- @s2script/ws: client WebSocket over __s2_ws_* (connect resolver + per-conn event subs) ---
  globalThis.__s2pkg_ws = {
    WebSocket: {
      connect: function (url, init) {
        return __s2_ws_connect(String(url), init).then(function (id) {
          return {
            onMessage: function (h) { __s2_ws_on(id, "message", function (m) { h(m); }); },
            onClose:   function (h) { __s2_ws_on(id, "close", function (code, reason) { h(code, reason); }); },
            onError:   function (h) { __s2_ws_on(id, "error", function (e) { h(e); }); },
            send:      function (data) { __s2_ws_send(id, String(data)); },
            close:     function () { __s2_ws_close(id); },
          };
        });
      },
    },
  };
  // --- @s2script/net: raw TCP + UDP client sockets over __s2_net_* (mirrors __s2pkg_ws, binary payloads) ---
  globalThis.__s2pkg_net = {
    Net: {
      connectTcp: function (host, port) {
        return __s2_net_tcp_connect(String(host), port | 0).then(function (id) {
          return {
            onData:  function (h) { __s2_net_on(id, "data", function (b) { h(b); }); },
            onClose: function (h) { __s2_net_on(id, "close", function () { h(); }); },
            onError: function (h) { __s2_net_on(id, "error", function (e) { h(e); }); },
            send:    function (data) { __s2_net_send(id, data); },
            close:   function () { __s2_net_close(id); },
          };
        });
      },
      udp: function () {
        return __s2_net_udp_bind().then(function (id) {
          return {
            onMessage: function (h) { __s2_net_on(id, "message", function (from, b) { h(from, b); }); },
            sendTo:    function (host, port, data) { __s2_net_send_to(id, String(host), port | 0, data); },
            close:     function () { __s2_net_close(id); },
          };
        });
      },
    },
  };
  // --- @s2script/votes: chat-ballot voting (revote) + an optional live center tally (a render seam). ---
  var __s2_vote_state = null;             // the single active vote, or null (the per-context lock)
  var __s2_vote_tallyRenderer = null;     // { show(slot, tally), clear(slot) } — CS2 registers it
  var __s2_vote_subInstalled = false;     // lazy-once guard: install onMessage/onDisconnect on first start
  var VOTE_HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;

  function __s2_vote_eligibleSlots() {
    var out = [], all = globalThis.__s2pkg_clients.Clients.all();
    for (var i = 0; i < all.length; i++) if (!all[i].isBot) out.push(all[i].slot);
    return out;
  }
  function __s2_vote_counts(st) {
    var counts = [], total = 0;
    for (var i = 0; i < st.options.length; i++) counts.push(0);
    st.votes.forEach(function (idx) { if (idx >= 0 && idx < counts.length) { counts[idx]++; total++; } });
    return { counts: counts, total: total };
  }
  var __s2_vote_warnedNoRenderer = false;
  function __s2_vote_showTally(st) {
    if (!st.showLiveTally) return;
    if (!__s2_vote_tallyRenderer) {   // showLiveTally set but no renderer (a non-CS2 game) -> degrade to chat-only, warn once
      if (!__s2_vote_warnedNoRenderer) { __s2_vote_warnedNoRenderer = true; globalThis.console && console.log("[votes] WARN: showLiveTally set but no tally renderer registered — chat-only."); }
      return;
    }
    var c = __s2_vote_counts(st);
    var opts = st.options.map(function (label, i) { return { label: label, count: c.counts[i] }; });
    var tally = { question: st.question, options: opts, total: c.total, secondsLeft: st.secondsLeft };
    var slots = __s2_vote_eligibleSlots();
    for (var i = 0; i < slots.length; i++) { try { __s2_vote_tallyRenderer.show(slots[i], tally); } catch (e) {} }
  }
  function __s2_vote_clearTally(st) {
    if (!st.showLiveTally || !__s2_vote_tallyRenderer) return;
    var slots = __s2_vote_eligibleSlots();
    for (var i = 0; i < slots.length; i++) { try { __s2_vote_tallyRenderer.clear(slots[i]); } catch (e) {} }
  }
  function __s2_vote_castFromChat(slot, text) {
    var st = __s2_vote_state; if (!st) return 0;                    // no active vote -> pass through
    var t = ("" + text).trim();
    if (!/^[0-9]$/.test(t)) return 0;
    var d = parseInt(t, 10);
    if (d < 1 || d > st.options.length) return 0;                  // out of range -> pass through
    st.votes.set(slot, d - 1);                                     // revote replaces
    __s2_vote_showTally(st);
    // NOTE: the "every connected non-bot has voted -> end early" check (design doc Flow step 5) lives
    // in __s2_vote_tick, NOT here. Checking synchronously on every cast would end the vote the instant
    // the last eligible voter casts a FIRST vote, pre-empting a later revote from any of them within the
    // same synchronous burst (see votes_cast_revote_tally_and_winner). Checking at the 1s tick boundary
    // instead gives a full window for still-pending revotes to land before turnout is judged complete.
    return VOTE_HANDLED;
  }
  function __s2_vote_ensureSubs() {
    if (__s2_vote_subInstalled) return; __s2_vote_subInstalled = true;
    __s2_chat_on_message(function (slot, text) { return __s2_vote_castFromChat(slot, text); });
    globalThis.__s2pkg_clients.Clients.onDisconnect(function (c) { var st = __s2_vote_state; if (st) st.votes.delete(c.slot); });
  }
  function __s2_vote_tick(st) {
    if (__s2_vote_state !== st) return;                            // ended/cancelled
    if (st.secondsLeft <= 0) { __s2_vote_end(); return; }
    st.secondsLeft--;
    __s2_vote_showTally(st);
    // End early once every connected non-bot has voted (design doc Flow step 5). Guarded on elig > 0 so
    // a vote started with zero connected non-bots doesn't vacuously "complete" inside Vote.start() itself.
    var elig = __s2_vote_eligibleSlots().length;
    if (elig > 0 && st.votes.size >= elig) { __s2_vote_end(); return; }
    globalThis.__s2pkg_timers.delay(1000).then(function () { __s2_vote_tick(st); });
  }
  function __s2_vote_end() {
    var st = __s2_vote_state; if (!st) return;
    __s2_vote_state = null;                                        // release the lock BEFORE onEnd (so onEnd can start a new vote)
    __s2_vote_clearTally(st);
    var c = __s2_vote_counts(st), winner = null, best = -1, tie = false;
    for (var i = 0; i < c.counts.length; i++) {
      if (c.counts[i] > best) { best = c.counts[i]; winner = i; tie = false; }
      else if (c.counts[i] === best) { tie = true; }
    }
    if (c.total === 0 || tie) winner = null;
    var result = { winner: winner, counts: c.counts, total: c.total };
    if (winner !== null) globalThis.__s2pkg_chat.Chat.toAll("[Vote] Passed: " + st.options[winner] + " (" + Math.round(c.counts[winner] / c.total * 100) + "%)");
    else globalThis.__s2pkg_chat.Chat.toAll("[Vote] Failed — no majority.");
    try { st.onEnd(result); } catch (e) { globalThis.console && console.log("[votes] onEnd threw: " + e); }
  }
  var Vote = {
    start: function (config) {
      if (__s2_vote_state) return false;                          // one vote at a time
      if (!config || !config.question || !config.options || config.options.length < 2) return false;
      __s2_vote_ensureSubs();
      var dur = Math.max(1, (config.duration | 0) || 20);   // clamp: a negative config would end on the first tick
      var st = { question: String(config.question), options: config.options.map(String), votes: new Map(),
                 showLiveTally: !!config.showLiveTally, secondsLeft: dur,
                 onEnd: (typeof config.onEnd === "function") ? config.onEnd : function () {} };
      __s2_vote_state = st;
      // ONE LINE PER OPTION. This used to join every option into a single line
      // ("[Vote] q — 1=a, 2=b, 3=c, ..."), which wrapped across most of the chat box the moment a
      // ballot carried five map names and was genuinely hard to read — the opposite of what a thing
      // you must answer by number needs to be. Plugins worked around it by suppressing this send and
      // reprinting their own, which then double-printed whenever the suppression missed.
      //
      // Deliberately UNCOLOURED: colour is a game concern and this is engine-generic. An option's
      // label is caller-owned, so a plugin that wants colour puts control bytes in the label itself.
      globalThis.__s2pkg_chat.Chat.toAll("[Vote] " + st.question);
      for (var vi = 0; vi < st.options.length; vi++) {
        globalThis.__s2pkg_chat.Chat.toAll("  " + (vi + 1) + ". " + st.options[vi]);
      }
      __s2_vote_showTally(st);
      __s2_vote_tick(st);                                         // starts the countdown + end
      return true;
    },
    isActive: function () { return !!__s2_vote_state; },
    cancel: function () { var st = __s2_vote_state; if (!st) return; __s2_vote_state = null; __s2_vote_clearTally(st); },
    registerTallyRenderer: function (r) { __s2_vote_tallyRenderer = r; },
  };
  globalThis.__s2pkg_votes = { Vote: Vote };

  // --- L1 lifecycle v2: the plugin() artifact + load-scoped ctx (design spec §1/§3/§4) ---
  // The SDK's @s2script/sdk/plugin subpath resolves (via the existing @s2script/ strip) to this.
  globalThis.__s2pkg_plugin = { plugin: function (factory) { return { __s2plugin: 1, factory: factory }; } };

  function __s2_make_ctx() {
    var pending = [];      // registration thunks, replayed at arm (Active)
    var armed = false;
    var sealed = false;
    var scopes = [];
    // The load-window reg: buffer while Loading, run immediately once armed, throw once sealed.
    function ctxReg(thunk) {
      if (sealed) throw new Error("s2script: registration outside the load window - use a Scope from ctx.createScope()");
      if (armed) { thunk(); } else { pending.push(thunk); }
    }
    // Build one subjects bundle over `regFn`. `track` is null for ctx (plugin-lifetime) or the
    // scope's tracker (ids + disposers). `viaId` forwards the native's returned sub id to the tracker
    // (tolerant of natives that still return undefined until T3 makes them return ids).
    function makeSubjects(track, regFn) {
      var t = track || { ids: function () {}, disposer: function (d) {} };
      function viaId(call) { return function () { var id = call.apply(null, arguments); if (typeof id === "number") t.ids(id); }; }
      var Ev = __s2pkg_events.Events, Cl = __s2pkg_clients.Clients, En = __s2pkg_entity.Entity;
      var Sv = __s2pkg_server.Server, Fr = __s2pkg_frame.OnGameFrame, Ck = __s2pkg_cookies.Cookies;
      var Uc = __s2pkg_usercmd.UserCmd, Dm = __s2pkg_damage.Damage, Sn = __s2pkg_sound.Sound;
      return {
        events: {
          on:    function (n, h) { regFn(viaId(function () { return Ev.on(n, h); })); },
          onPre: function (n, h) { regFn(viaId(function () { return Ev.onPre(n, h); })); },
        },
        clients: {
          onConnect:         function (h) { regFn(viaId(function () { return Cl.onConnect(h); })); },
          onPutInServer:     function (h) { regFn(viaId(function () { return Cl.onPutInServer(h); })); },
          onActive:          function (h) { regFn(viaId(function () { return Cl.onActive(h); })); },
          onFullyConnect:    function (h) { regFn(viaId(function () { return Cl.onFullyConnect(h); })); },
          onDisconnect:      function (h) { regFn(viaId(function () { return Cl.onDisconnect(h); })); },
          onSettingsChanged: function (h) { regFn(viaId(function () { return Cl.onSettingsChanged(h); })); },
          onVoice:           function (h) { regFn(viaId(function () { return Cl.onVoice(h); })); },
          onCookiesCached:   function (h) { regFn(viaId(function () { return Ck.onCached(h); })); },
          onSay:             function (h) { regFn(viaId(function () { return __s2_chat_on_message(h); })); },
          onRunCmd:          function (h) { regFn(viaId(function () { return Uc.onRun(h); })); },
        },
        translations: {
          // Declare the phrase files this plugin uses. Nothing is loaded for a plugin automatically
          // — the same rule SourceMod's LoadTranslations enforces. ORDER MATTERS: translate takes
          // the first hit within each of its two passes (the client's language, then English), so
          // list your own set before any shared one to be able to override a shared phrase.
          load: function () {
            for (var i = 0; i < arguments.length; i++) {
              __s2pkg_translations.Translations.load(String(arguments[i]));
            }
          },
        },
        entities: {
          onCreate: function (c, h) { regFn(viaId(function () { return En.onCreate(c, h); })); },
          onSpawn:  function (c, h) { regFn(viaId(function () { return En.onSpawn(c, h); })); },
          onDelete: function (c, h) { regFn(viaId(function () { return En.onDelete(c, h); })); },
          onOutput: function (c, o, h) { regFn(viaId(function () { return En.onOutput(c, o, h); })); },
          onDamage: function (h) { regFn(viaId(function () { return Dm.onPre(h); })); },
        },
        server: {
          onGameFrame: function (fn, opts) { regFn(function () { var d = Fr.subscribe(fn, opts || {}); if (d && d.dispose) t.disposer(d.dispose); }); },
          onMapStart:  function (h) { regFn(viaId(function () { return Sv.onMapStart(h); })); },
          onPrecache:  function (h) { regFn(viaId(function () { return Sn.onPrecache(h); })); },
        },
      };
    }
    var ctx = makeSubjects(null, ctxReg);
    ctx.id = __s2_current_plugin();
    ctx.previous = __s2_handoff_take();
    ctx.commands = {
      register:       function (n, h)    { ctxReg(function () { __s2pkg_commands.Commands.register(n, h); }); },
      registerServer: function (n, h)    { ctxReg(function () { __s2pkg_commands.Commands.registerServer(n, h); }); },
      registerAdmin:  function (n, f, h) { ctxReg(function () { __s2pkg_commands.Commands.registerAdmin(n, f, h); }); },
      onClientCommand: function (n, h)   { ctxReg(function () { __s2pkg_commands.Commands.onClientCommand(n, h); }); },
    };
    ctx.config  = { onChange: function (h) { ctxReg(function () { __s2pkg_config.config.onChange(h); }); } };
    ctx.topmenu = {
      addCategory: function (n)    { ctxReg(function () { __s2pkg_topmenu.TopMenu.addCategory(n); }); },
      addItem:     function (c, i) { ctxReg(function () { __s2pkg_topmenu.TopMenu.addItem(c, i); }); },
    };
    ctx.publish = function (name, impl) {
      ctxReg(function () { __s2_iface_publish(name, impl); });
      return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
    };
    function handleFor(name) {
      return new Proxy({}, { get: function (_t, prop) {
        if (prop === "on") return function (ev, h) { ctxReg(function () { __s2_iface_on(name, ev, h); }); };
        if (typeof prop !== "string") return undefined;
        return function () { return __s2_iface_call(name, prop, Array.prototype.slice.call(arguments)); };
      }});
    }
    ctx.use = function (name) {
      if (sealed) throw new Error("s2script: ctx.use outside the load window");
      var kind = __s2_iface_dep_kind(name);
      if (kind !== "hard") throw new Error("s2script: ctx.use('" + name + "') requires a pluginDependencies entry (declared: " + kind + ")");
      return handleFor(name);
    };
    ctx.tryUse = function (name) {
      if (sealed) throw new Error("s2script: ctx.tryUse outside the load window");
      var kind = __s2_iface_dep_kind(name);
      if (kind !== "optional") throw new Error("s2script: ctx.tryUse('" + name + "') requires an optionalPluginDependencies entry (declared: " + kind + ")");
      return __s2_iface_is_published(name) ? handleFor(name) : null;
    };
    ctx.createScope = function () {
      if (sealed) throw new Error("s2script: createScope outside the load window");
      var ids = [], disposers = [], disposed = false;
      var tracker = { ids: function (i) { ids.push(i); }, disposer: function (d) { disposers.push(d); } };
      // The scope reg: buffer while Loading (replayed at arm), register immediately once armed.
      function scopeReg(thunk) { if (!armed) { pending.push(thunk); } else { thunk(); } }
      var scope = makeSubjects(tracker, scopeReg);
      scope.clear = function () {
        __s2_scope_dispose(ids.slice());   // T3: sweep this scope's sub ids out of every store
        ids.length = 0;
        var ds = disposers.slice(); disposers.length = 0;
        for (var i = 0; i < ds.length; i++) { try { ds[i](); } catch (e) {} }
      };
      scope.dispose = function () { if (disposed) return; scope.clear(); disposed = true; };
      Object.defineProperty(scope, "disposed", { get: function () { return disposed; } });
      scopes.push(scope);
      return scope;
    };
    globalThis.__s2_ctx_arm = function () {
      var p = pending; pending = []; armed = true;
      for (var i = 0; i < p.length; i++) { p[i](); }   // a throw here aborts the arm → Failed (host TryCatch)
      sealed = true;
    };
    globalThis.__s2_ctx_seal = function () { sealed = true; pending = []; };

    // Game packages contribute their own ctx namespaces (e.g. ctx.gameRules) by setting
    // __s2pkg_game_ctx before this runs — the package prelude is evaluated ahead of ctx
    // construction (v8host.rs runs the engine prelude, then @s2script/<game>, then the plugin).
    // ENGINE-GENERIC: core names no game concept here; it merges whatever the package declares,
    // and each factory receives the same ledger registrar every built-in namespace uses, so a
    // game-package subscription is torn down at unload exactly like ctx.events.on.
    //
    // Deliberately LAST — after every built-in ctx member above is attached — so `ns in ctx` is a
    // complete collision test. A hand-maintained name list would only protect whichever built-ins
    // happen to be assigned before the check; running last means there is nothing left to add to a
    // list, and no accident of source order standing in for a guard.
    var gameCtx = globalThis.__s2pkg_game_ctx;
    if (gameCtx) {
      // Same shape as makeSubjects' own viaId, specialised to ctx's own (null) track: ctx-level
      // subscriptions aren't per-id tracked for early disposal (that's what createScope() is
      // for), so this just calls through and drops the sub id.
      var viaId = function (call) { return function () { call.apply(null, arguments); }; };
      for (var ns in gameCtx) {
        if (!Object.prototype.hasOwnProperty.call(gameCtx, ns)) continue;
        if (ns === "__proto__") {
          console.log("[s2script] WARN: game package ctx namespace '__proto__' refused");
          continue;
        }
        if (ns in ctx) {
          console.log("[s2script] WARN: game package ctx namespace '" + ns + "' collides with an existing ctx member — refused");
          continue;
        }
        if (typeof gameCtx[ns] !== "function") {
          console.log("[s2script] WARN: game package ctx namespace '" + ns + "' is not a factory function — skipped");
          continue;
        }
        ctx[ns] = gameCtx[ns](ctxReg, viaId);
      }
    }
    return ctx;
  }

  // Load-window free APIs (SourceMod-shaped). Bound to the current factory / OnPluginStart
  // ctx via globalThis.__s2_load_ctx — set for the load run, cleared at settle. command/hook
  // throw after settle; publish/use/tryUse/topmenu/translations share the same window.
  function __s2_load_ctx_or_throw(api) {
    var ctx = globalThis.__s2_load_ctx;
    if (!ctx) throw new Error("s2script: " + api + " outside the load window");
    return ctx;
  }
  function command(name, handler) {
    __s2_load_ctx_or_throw("command()").commands.register(name, handler);
  }
  command.admin = function (name, flags, handler) {
    __s2_load_ctx_or_throw("command.admin()").commands.registerAdmin(name, flags, handler);
  };
  command.server = function (name, handler) {
    __s2_load_ctx_or_throw("command.server()").commands.registerServer(name, handler);
  };
  var hook = {
    damage: function (h) { __s2_load_ctx_or_throw("hook.damage()").entities.onDamage(h); },
  };
  function publish(name, impl) { return __s2_load_ctx_or_throw("publish()").publish(name, impl); }
  function use(name) { return __s2_load_ctx_or_throw("use()").use(name); }
  function tryUse(name) { return __s2_load_ctx_or_throw("tryUse()").tryUse(name); }
  var topmenu = {
    addCategory: function (n) { __s2_load_ctx_or_throw("topmenu.addCategory()").topmenu.addCategory(n); },
    addItem: function (c, i) { __s2_load_ctx_or_throw("topmenu.addItem()").topmenu.addItem(c, i); },
  };
  var translations = {
    load: function () {
      var c = __s2_load_ctx_or_throw("translations.load()");
      return c.translations.load.apply(c.translations, arguments);
    },
  };
  globalThis.__s2pkg_commands.command = command;
  globalThis.__s2pkg_plugin.hook = hook;
  globalThis.__s2pkg_plugin.publish = publish;
  globalThis.__s2pkg_plugin.use = use;
  globalThis.__s2pkg_plugin.tryUse = tryUse;
  globalThis.__s2pkg_plugin.topmenu = topmenu;
  globalThis.__s2pkg_plugin.translations = translations;

  // def = plugin() artifact or undefined; exports = module.exports (named publics).
  // Order: factory if present → OnGameFrame/OnMapStart subscribe → OnPluginStart() →
  // OnPluginEnd attached as hooks.onUnload. Load window stays open until settle.
  globalThis.__s2_run_factory = function (def, exports) {
    var ctx = __s2_make_ctx();
    globalThis.__s2_load_ctx = ctx;
    function done(hooks) {
      globalThis.__s2_load_ctx = null;
      __s2_load_settled(hooks);
    }
    function fail(e) {
      globalThis.__s2_load_ctx = null;
      __s2_load_failed(String((e && e.stack) || e));
    }
    function afterFactory(hooks) {
      var exp = exports || {};
      var startOut;
      try {
        if (typeof exp.OnGameFrame === "function") ctx.server.onGameFrame(exp.OnGameFrame);
        if (typeof exp.OnMapStart === "function") ctx.server.onMapStart(exp.OnMapStart);
        if (typeof exp.OnPluginStart === "function") startOut = exp.OnPluginStart();
        if (typeof exp.OnPluginEnd === "function") {
          var prevUnload = hooks && hooks.onUnload;
          var prevState = hooks && hooks.state;
          hooks = {
            onUnload: function () {
              try { if (typeof prevUnload === "function") prevUnload(); }
              finally { exp.OnPluginEnd(); }
            },
          };
          if (typeof prevState === "function") hooks.state = prevState;
        }
      } catch (e) { fail(e); return; }
      if (startOut && typeof startOut.then === "function") {
        startOut.then(function () { done(hooks); }, fail);
      } else {
        done(hooks);
      }
    }
    var out;
    try {
      if (def && typeof def.factory === "function") out = def.factory(ctx);
    } catch (e) { fail(e); return; }
    if (out && typeof out.then === "function") {
      out.then(afterFactory, fail);
    } else {
      afterFactory(out);
    }
  };
})();
