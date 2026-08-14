#ifndef S2SCRIPT_CORE_H
#define S2SCRIPT_CORE_H
#include <stdint.h>   /* uint64_t */
#ifdef __cplusplus
extern "C" {
#endif

typedef void (*s2_log_fn)(int level, const char* utf8_msg);
typedef void (*s2_hook_request_fn)(const char* descriptor, int enable); /* core -> shim: install(1)/remove(0) */

/* Engine-operation function pointers the shim implements and the core calls.
 * Every Slice-3 engine touchpoint is a C++ call (SchemaSystem virtuals, entity
 * system, ...) that lives shim-side; the core only ever sees these opaque C-ABI
 * pointers, never a raw C++ vtable.  All fields may be null -> the matching native
 * degrades to a safe miss.  Task 3 wires schema_offset; Tasks 4-5 fill the rest. */
typedef int   (*s2_schema_offset_fn)(const char* cls, const char* field);
typedef void* (*s2_ent_by_index_fn)(int idx);
typedef void* (*s2_deref_handle_fn)(unsigned int handle);
typedef void  (*s2_ent_state_changed_fn)(void* ent, int offset);
typedef void  (*s2_concommand_register_fn)(const char* name);

/* Schema enumeration (5B.1). The shim walks the SchemaSystem and streams each class/field to core
 * via these callbacks (core provides them + an opaque ctx). kind ∈ atomic|handle|class|ptr|enum|unknown.
 * A null parent/name/inner is an absent value.
 *
 * `size` is the field type's byte width where the SchemaSystem states it, else 0 (= unknown, omit).
 * It exists for `enum`: the category gives a type NAME but no width, and without a width there is no
 * way to pick a reader, so every enum field was being skipped. Derived from the enum binding on our
 * own binary, so it is a resolved fact rather than a borrowed constant. */
typedef void (*s2_emit_class_fn)(void* ctx, const char* name, const char* parent);
typedef void (*s2_emit_field_fn)(void* ctx, const char* cls, const char* name, int offset,
                                 const char* kind, const char* type_name, const char* inner,
                                 int size);
/* One enumerator of one declared enum. `size` is the enum's byte width (1/2/4/8); `value` is the
 * enumerator's value, widened to 64-bit signed because the schema stores it as int64. Emitted for
 * every enum in the type scope, so an enum no field happens to use is still described — a plugin can
 * legitimately want the constants without the field. */
typedef void (*s2_emit_enum_fn)(void* ctx, const char* enum_name, int size,
                                const char* enumerator, long long value);
typedef int  (*s2_schema_enumerate_fn)(void* ctx, s2_emit_class_fn emit_class, s2_emit_field_fn emit_field,
                                       s2_emit_enum_fn emit_enum);

/* Game-event engine-ops (Slice 5D.1). The shim implements these; the core calls them.
 * event_subscribe/unsubscribe track which events the JS layer has subscribed to and
 * install/remove the IGameEventListener2 per-name.  The six accessors read the current
 * IGameEvent* (set by FireGameEvent before calling s2script_core_dispatch_game_event).
 * All return safe defaults when the manager or current event is null (degrade-never-crash). */
typedef int          (*s2_event_subscribe_fn)(const char* name);
typedef int          (*s2_event_unsubscribe_fn)(const char* name);
typedef int          (*s2_event_get_int_fn)(const char* key);
typedef float        (*s2_event_get_float_fn)(const char* key);
typedef int          (*s2_event_get_bool_fn)(const char* key);        /* 0/1 */
typedef const char*  (*s2_event_get_string_fn)(const char* key);      /* valid during dispatch; core copies now */
typedef uint64_t     (*s2_event_get_uint64_fn)(const char* key);
typedef int          (*s2_event_get_player_slot_fn)(const char* key); /* -1 if absent */

/* Engine-identity ops — the connected-client list via TYPED SDK VIRTUALS (GetIGameServer /
 * GetPlayerUserId / GetClientConVarValue) + a lifecycle-tracked signon array
 * (clientlist-fakeconvar-onmapstart slice; retired the 5D.2 hand offsets). Contracts unchanged.
 * All degrade to safe misses on any null. */
typedef int          (*s2_client_valid_fn)(int slot);          /* 0/1: connected client at slot (incl. bots) */
typedef int          (*s2_client_userid_fn)(int slot);         /* engine user-id, or -1 */
typedef int          (*s2_client_signon_fn)(int slot);         /* tracked signon: 0 none/disconnected, 2 connected, 5 spawned, 6 full in-game; -1 slot OOB */
typedef const char*  (*s2_client_name_fn)(int slot);           /* valid during call; core copies now */
typedef int          (*s2_client_find_by_userid_fn)(int userid); /* slot, or -1 */

/* Event write/fire ops (Slice 5D.3). Write the shim's current write target (the pre-hook's live
 * IGameEvent, OR a just-created to-be-fired event). All no-op if the target/manager is null. */
typedef void (*s2_event_set_int_fn)(const char* key, int value);
typedef void (*s2_event_set_float_fn)(const char* key, float value);
typedef void (*s2_event_set_bool_fn)(const char* key, int value);       /* 0/1 */
typedef void (*s2_event_set_string_fn)(const char* key, const char* value);
typedef void (*s2_event_set_uint64_fn)(const char* key, uint64_t value);
typedef int  (*s2_event_create_fn)(const char* name);                   /* 1 = created (retargets writes); 0 = null mgr / unknown name */
typedef int  (*s2_event_fire_fn)(int dontBroadcast);                    /* returns FireEvent result; 0 if no created event */

/* Config ops (Slice 5E.2). Read/auto-generate the admin override file addons/s2script/configs/<id>.json. */
typedef const char* (*s2_config_read_fn)(const char* id);            /* file content, or null if absent; valid until the next config_read */
typedef int         (*s2_config_write_fn)(const char* id, const char* content); /* 1 ok / 0 fail */

/* Chat messaging (Slice 6.1). Print a message to one client's chat; slot is 0-based (server console
   has no chat, so slot < 0 is a no-op). The shim implements this via the CS2 chat user message. */
typedef void (*s2_client_print_fn)(int slot, const char* msg);

/* Client SteamID64 as a decimal string (Slice 6.2). "0" for bots / unauthenticated / invalid slot.
   Valid until the next client_steamid call. Via IVEngineServer2::GetClientXUID. */
typedef const char* (*s2_client_steamid_fn)(int slot);

/* Kick a connected client (Slice 6.3). No-op for null engine or out-of-range slot. */
typedef void (*s2_client_kick_fn)(int slot, const char* reason);

/* Server console command + map-validity query (Slice 6.4). Null/no-engine safe. */
typedef void (*s2_server_command_fn)(const char* cmd);
typedef int  (*s2_server_map_valid_fn)(const char* map);
// Slice 6.6 Stage 2: read a field from the CURRENT CTakeDamageInfo (valid only during a damage dispatch)
// at a schema-resolved byte offset. Block-scoped, mirrors the GameEvent accessor pattern.
typedef float (*s2_damage_read_float_fn)(int offset);
typedef int   (*s2_damage_read_int_fn)(int offset);
// Stage 3 (modify): write m_flDamage etc. during a pre-hook.
typedef void  (*s2_damage_write_float_fn)(int offset, float value);
// The victim's raw CEntityHandle (from the detour `this`); -1/0xFFFFFFFF = none. JS decodes -> EntityRef.
typedef int   (*s2_damage_victim_fn)(void);
// Slice 6.7: read a cvar's current value as a string ("" if absent). Valid until the next cvar_get call.
typedef const char* (*s2_cvar_get_fn)(const char* name);
/* Write a cvar through ICvar now (not ServerCommand). 1 = applied, 0 = absent / unparseable. */
typedef int (*s2_cvar_set_fn)(const char* name, const char* value);
// Slice sub-project-2: print one line to a client's developer console (IVEngineServer2::ClientPrintf).
typedef void (*s2_client_console_print_fn)(int slot, const char* msg);
// Client IP address ("IP:port"; "" for a bot/no netchannel). Valid until the next call.
typedef const char* (*s2_client_address_fn)(int slot);
// Server-info ops (reservedslots+basetriggers) — typed calls on the held INetworkGameServer*.
typedef int         (*s2_server_max_clients_fn)(void); /* GetMaxClients(); 0 if unavailable */
typedef const char* (*s2_server_map_name_fn)(void);    /* GetMapName(); "" if unavailable. Valid until next call. */
typedef float       (*s2_server_game_time_fn)(void);   /* GetGlobals()->curtime; 0 if unavailable */
// Slice DB: absolute path to the s2script data directory (<addon>/data), created if absent.
typedef const char* (*s2_db_data_dir_fn)(void);
// Slice menu: fire the pending created event to ONE client's per-client legacy listener (SourceMod
// FireToClient parity). Returns 1 on success, 0 on a miss (no manager / no pending event / no client / bot).
typedef int (*s2_event_fire_to_client_fn)(int slot);
// Slice nominations: raw configs-dir file read/write (name includes its extension; no .json append).
// Reads/writes addons/s2script/configs/<sanitized name>; a ".." or empty name resolves to a null read /
// no-op write (no traversal). APPENDED after event_fire_to_client; order is the ABI.
typedef const char* (*s2_config_read_file_fn)(const char* name);
typedef int         (*s2_config_write_file_fn)(const char* name, const char* content);

/* Ray-trace slice: CNavPhysicsInterface::TraceShape, resolved by an RTTI vtable-by-name scan
 * (shim/src/vtable.{h,cpp}) — CS2 does not export this vtable via dlsym. ENGINE-GENERIC (Source-2
 * physics; no CS2 names here). Returns 1 and fills *out on a completed trace (didHit/fraction from
 * CGameTrace::DidHit()/m_flFraction, endpos/normal copied out, allSolid from m_bStartInSolid,
 * hitEntHandle = the hit CEntityInstance's GetRefEHandle().ToInt() or -1). Returns 0 (op
 * unavailable / vtable unresolved) leaving *out untouched — degrade-never-crash. The ignore entity
 * is resolved shim-side from (ignoreEntIdx, ignoreEntSerial) via the EXISTING serial-gated entity
 * lookup (s2_deref_handle); a negative idx/serial means "no ignore entity". The hit entity crosses
 * back ONLY as hitEntHandle (an (index,serial)-decodable int) — never a raw pointer; the Rust core
 * decodes it into a serial-gated EntityRef (the DamageInfo.victim pattern). */
typedef struct {
    int   didHit;
    float fraction;
    float endpos[3];
    float normal[3];
    int   allSolid;
    int   hitEntHandle;
} S2TraceResult;
typedef int (*s2_trace_shape_fn)(const float* start, const float* end, const float* mins, const float* maxs,
                                 unsigned long long interactsWith, unsigned long long interactsExclude,
                                 int ignoreEntIdx, int ignoreEntSerial, S2TraceResult* out);

/* Entity-creation lifecycle slice — APPENDED after trace_shape; order is the ABI.
 * create: className -> packed CEntityHandle (ToInt), 0 = failure. The raw CBaseEntity* is
 * converted shim-side and never crosses to JS. spawn/teleport/remove take the (index, serial)
 * pair already used by every other serial-gated entity op. teleport's origin/angles/velocity
 * are nullable [x,y,z]/[pitch,yaw,roll] float triples. */
typedef int (*s2_entity_create_fn)(const char* className);
typedef int (*s2_entity_spawn_fn)(int index, int serial);
typedef int (*s2_entity_teleport_fn)(int index, int serial, const float* origin, const float* angles, const float* velocity);
typedef int (*s2_entity_remove_fn)(int index, int serial);

/* Item slice — APPENDED after entity_remove; order is the ABI. (A5b retired this group's
 * CS2-specific give_named_item/remove_player_item members to gamedata/cs2 `calls` descriptors; the
 * two survivors keep their original relative order.)
 * entity_subobj_vcall: (index,serial) of an entity + a subObjOffset + a vtableIndex (.text-validated
 * shim-side) + an optional (argIdx,argSerial) entity arg (-1,-1 = no arg) -> 1/0 success.
 * entity_read_handle_vector: (index,serial) + a pointer-deref chain (ptrOffs[ptrCount]) + a
 * vectorOff (CUtlVector base) + a maxCount cap -> fills outHandles[] with packed CEntityHandles,
 * returns the element count written (<= maxCount), 0 on any unresolved step. */
typedef int (*s2_entity_subobj_vcall_fn)(int index, int serial, int subObjOffset, int vtableIndex, int argIndex, int argSerial);
typedef int (*s2_entity_read_handle_vector_fn)(int index, int serial, const int* ptrOffs, int ptrCount, int vectorOff, int maxCount, int* outHandles);

/* Entity-I/O slice — APPENDED after entity_read_handle_vector; order is the ABI.
 * entity_fire_input: fire an entity input via AddEntityIOEvent (the game's own input-firing path,
 * used e.g. by map I/O and FireOutputInternal). (index,serial) serial-gates the target; value is the
 * input's string argument ("" = none, Source parses it per the input's field type); (actIdx,actSerial)/
 * (callerIdx,callerSerial) are the activator/caller entities (<0 = none/null); delay is queued same-tick
 * via the engine's I/O event queue (0 = fires this same tick). Returns 1/0 success. */
typedef int (*s2_entity_fire_input_fn)(int index, int serial, const char* input, const char* value,
                                       int actIdx, int actSerial, int callerIdx, int callerSerial, float delay);

/* EKV slice — APPENDED after entity_fire_input; order is the ABI.
 * entity_spawn_kv: DispatchSpawn a serial-gated entity with a CEntityKeyValues built shim-side from
 * parallel arrays. types[i]: 0=string 1=int 2=float 3=bool; values are stringified ("1"/"0" for bool).
 * The CEntityKeyValues lives entirely inside the call (build -> AddRef -> DispatchSpawn -> guarded
 * Release) — no handle, no raw pointer to JS. Returns 1 ok / 0 fail. */
typedef int (*s2_entity_spawn_kv_fn)(int index, int serial, int count,
    const char* const* keys, const int* types, const char* const* values);

/* Game-rules + UserMessage slice — APPENDED after entity_spawn_kv; order is the ABI.
 * entity_find_by_class: fill outIndices/outSerials with the (index,serial) of every entity whose
 * CEntityIdentity::m_designerName == className, up to maxCount; returns the TOTAL match count. */
typedef int (*s2_entity_find_by_class_fn)(const char* className, int* outIndices, int* outSerials, int maxCount);

/* UserMessage send family — APPENDED after entity_find_by_class; order is the ABI. Generalize the
 * SayText2 protobuf-reflection path: create a named message into a single shim-side target, set its
 * scalar fields by reflection cpp_type, then send to the given slots (slotCount<0 = broadcast). */
typedef int (*s2_user_message_create_fn)(const char* name);
typedef int (*s2_user_message_set_int_fn)(const char* field, int64_t value);
typedef int (*s2_user_message_set_float_fn)(const char* field, double value);
typedef int (*s2_user_message_set_string_fn)(const char* field, const char* value);
typedef int (*s2_user_message_set_bool_fn)(const char* field, int value);
typedef int (*s2_user_message_send_fn)(const int* slots, int slotCount);

/* convar_register: register a plugin-owned ConVar via ICvar::RegisterConVar (FakeConVar slice).
 * type: 0=bool 1=int32 2=float32 3=string. defaultValue always a string (shim parses).
 * minValue/maxValue: nullable, numeric types only. FCVAR_RELEASE is OR'd shim-side.
 * Returns 1 registered (or already registered — idempotent), 0 fail. */
typedef int (*s2_convar_register_fn)(const char* name, const char* help, uint64_t flags, int type,
                                     const char* defaultValue, const char* minValue, const char* maxValue);

/* Translations slice — APPENDED after convar_register; order is the ABI.
 * translations_read(lang,name): content of translations/[<lang>/]<name>.phrases.json, or null.
 * lang=="" -> the root file. Both segments sanitized; ".." refused. Valid until the next call. */
typedef const char* (*s2_translations_read_fn)(const char* lang, const char* name);
/* client_language(slot): the client's cl_language ("english"/"german"/...), or null. */
typedef const char* (*s2_client_language_fn)(int slot);

/* collision_activate: register a serial-gated entity's collision bounds in the spatial partition
 * (zones real-trigger slice). A raw schema-write bbox on a runtime-created trigger_multiple never
 * fires touch until the entity joins the collision partition; this runs the CCollisionProperty
 * registration path (CollisionMarkPartitionDirty ± the init sequence Task 1 proves). Returns 1 if
 * the call was made, 0 if unresolved/stale (degrade-never-crash). ENGINE-GENERIC. */
typedef int (*s2_collision_activate_fn)(int index, int serial);

/* entity_set_model: CBaseEntity::SetModel(const char*) on a serial-gated entity (zones real-trigger
 * slice). Gives a runtime entity a model + its collision — a trigger_multiple needs a model to build
 * the physics volume that fires touch (map triggers get it via InitTrigger->SetModel(GetModelName());
 * a runtime entity's model name is empty). Returns 1 on success, 0 if unresolved/stale. ENGINE-GENERIC. */
typedef int (*s2_entity_set_model_fn)(int index, int serial, const char* modelName);

/* Entity-property slice. Five engine-generic setters on a serial-gated entity; each returns 1 on
 * success, 0 if the signature was unresolved or the ref is stale. ENGINE-GENERIC (CBaseEntity /
 * CBaseModelEntity — no CS2 identifier appears in any of them).
 *
 * entity_set_gravity_scale: CBaseEntity::SetGravityScale(float). NOT equivalent to writing
 *   m_flGravityScale — the setter early-returns on an unchanged value and maintains
 *   m_flActualGravityScale, so a raw field write appears to do nothing.
 * entity_apply_abs_velocity_impulse: CBaseEntity::ApplyAbsVelocityImpulse(const Vector*). The
 *   impulse is three floats BY ADDRESS; `impulse` is a 3-float array. Additive and physics-aware,
 *   unlike writing m_vecAbsVelocity (which skips the partition/physics update).
 * entity_stop_sound: CBaseEntity::StopSound(const char*). Pairs with sound_emit.
 * entity_set_body_group_by_name: CBaseModelEntity::SetBodyGroupByName(const char*, int). `group` is
 *   32-bit callee-side; the width is load-bearing.
 * entity_set_model_scale: CBaseModelEntity::SetModelScale(float). Arg shape confirmed by
 *   disassembly; the NAME is a catalogue attribution the body does not itself prove — see the
 *   gamedata comment. Safe to call regardless. */
typedef int (*s2_entity_set_gravity_scale_fn)(int index, int serial, float scale);
typedef int (*s2_entity_apply_abs_velocity_impulse_fn)(int index, int serial, const float* impulse);
typedef int (*s2_entity_stop_sound_fn)(int index, int serial, const char* soundName);
typedef int (*s2_entity_set_body_group_by_name_fn)(int index, int serial, const char* name, int group);
typedef int (*s2_entity_set_model_scale_fn)(int index, int serial, float scale);

/* Entity lifecycle listeners slice — APPENDED after entity_set_model; order is the ABI.
 * entity_listener_install: lazily register the IEntityListener on CGameEntitySystem on the
 * first-ever JS entity-lifecycle subscribe. Idempotent (AddListenerEntity guards Find) + re-asserted
 * each map by the StartupServer POST hook. Returns 1 if installed/queued, 0 if the AddListenerEntity
 * signature is unresolved (degrade — subscribe delivers nothing). */
typedef int (*s2_entity_listener_install_fn)(void);

/* entity_name: read an entity's targetname (CEntityIdentity::m_name, a CUtlSymbolLarge; String() is
 * inline). Serial-gated (index,serial). Returns the name ("" if the entity has no targetname), valid
 * during the call — the core copies immediately — or NULL if stale/invalid. ENGINE-GENERIC.
 * Zones/surftimer slice. */
typedef const char* (*s2_entity_name_fn)(int index, int serial);

/* Sound slice — APPENDED after entity_name (the ABI tail); order is the ABI.
 * sound_emit: play a named CS2 SoundEvent from a serial-gated source entity to a slot set.
 * Sig-resolved CBaseEntity::EmitSound (preferred member overload (name, volume*, IRecipientFilter*);
 * the CSSharp EmitSound_t static path as fallback — see the 2026-07-13 sound spec). soundName = the
 * soundevent name (the engine resolves name->hash). entSerial < 0 = emit from entIndex with NO serial
 * check (worldspawn / global 2D). slots[0..slotCount) = recipient slots (bot slots are skipped — no
 * netchannel). volume in [0,1]. Returns the SndOpEventGuid (nonzero uint32 as int) or 0 (unresolved
 * sig / stale entity / caller requested no recipients (slotCount <= 0)). An all-bot-skipped filter
 * still CALLS the engine (plays to nobody), not a degrade. ENGINE-GENERIC. */
typedef int (*s2_sound_emit_fn)(const char* soundName, int entIndex, int entSerial,
                                const int* slots, int slotCount, float volume);
/* sound_precache_add: add a resource path (e.g. "soundevents/mypack.vsndevts") to the session
 * resource manifest currently being built. Valid ONLY during a precache-hook dispatch (the manifest
 * pointer is live only then; block-scoped like a game event). Returns 1 on add, 0 if no active
 * manifest / unresolved. ENGINE-GENERIC. */
typedef int (*s2_sound_precache_add_fn)(const char* path);
/* checktransmit slice: upsert the merged visibility mask for a serial-gated entity.
 * Returns 1 on success, 0 on a stale ref / full table / uninstalled hook / disabled descriptor. */
typedef int (*s2_transmit_set_fn)(int index, int serial, unsigned long long mask);
/* checktransmit slice: drop the entity's rule entry (1 removed, 0 absent). */
typedef int (*s2_transmit_clear_fn)(int index);
/* checktransmit slice: copy the hot-path counters into out[5] = {snapshots, entries, bitsCleared, nsLast, nsMax}. */
typedef void (*s2_transmit_stats_fn)(unsigned long long* out);

/* usercmd slice — APPENDED after sound_precache_add; order is the ABI. All operate on the shim's
 * s_currentUserCmd (the in-flight cmd's CSGOUserCmdPB); valid only during a usercmd dispatch. */
typedef int   (*s2_usercmd_hook_install_fn)(void);              /* lazily install the ProcessUsercmds detour; 1 ok / 0 unresolved */
typedef double(*s2_usercmd_read_fn)(int field);                 /* field: 0 fwd,1 side(raw leftmove NEGATED->+right),2 up,3 pitch,4 yaw,5 roll,6 impulse */
typedef void  (*s2_usercmd_write_fn)(int field, double value);
typedef uint64_t (*s2_usercmd_read_buttons_fn)(void);           /* base.buttons_pb.buttonstate1 */
typedef void  (*s2_usercmd_write_buttons_fn)(uint64_t mask);
typedef void  (*s2_usercmd_clear_subtick_fn)(void);             /* clear base.subtick_moves */

/* Voice-control slice — APPENDED after transmit_stats; order is the ABI.
 * voice_set_muted: set/clear the per-slot server-side voice mute (sender -> ALL receivers). The flag
 * lives SHIM-side: the SetClientListening pre-hook consults it allocation-free (O(n^2) per game voice
 * refresh), so JS only flips state through this op. Returns 1 = recorded + enforceable; 0 = slot out
 * of range OR the voice descriptor is degraded (hook missing / vtable validation failed) — the flag
 * is then inert and the shim has logged the named reason.
 * voice_get_muted: 1 = muted, 0 = not muted, -1 = slot out of range / degraded. */
typedef int (*s2_voice_set_muted_fn)(int slot, int muted);
typedef int (*s2_voice_get_muted_fn)(int slot);
/* Crash-reporter slice — APPENDED after voice_get_muted; order is the ABI.
 * server_build_number: the engine build via IVEngineServer2::GetBuildVersion(); 0 if the
 * interface is unavailable. Engine-generic (a Source 2 engine virtual, not a game name). */
typedef int (*s2_server_build_number_fn)(void);
/* Crash-harness (dev-only, gated core-side by crashreporter.json dev_test): raise a real
 * native fault on command. kind: 0 = null volatile write (SIGSEGV), 1 = abort() (SIGABRT). */
typedef void (*s2_crash_test_native_fn)(int kind);
/* E1 entity-liveness slice — slot-side identity-chunk validation (engine-generic). */
typedef void*     (*s2_ent_resolve_fn)(int index, int serial);
typedef long long (*s2_ent_identity_flags_fn)(int index, int serial);
typedef int       (*s2_ent_snapshot_fn)(int* out_indices, int* out_serials, int cap);

/* Plugin-declared engine calls (plugin-gamedata slice) — APPENDED after ent_snapshot; order is the
 * ABI. Implemented in shim/src/engine_calls.{h,cpp}; these MUST stay signature-identical to the
 * declarations there. Engine-generic: every string is an OPAQUE name out of the PLUGIN's own
 * gamedata, so no game identifier reaches the core.
 * engine_call_resolve: resolve one descriptor against the live binary -> call id >= 0, or -1 with a
 *   named reason written to reasonOut (the core stores it verbatim as the degrade reason).
 * engine_call_invoke: call a resolved descriptor on a serial-gated entity receiver. 1 = called
 *   (retOut written), 0 = degraded (stale receiver / absent `via` sub-object / bad arg budget). */
typedef int (*s2_engine_call_resolve_fn)(const char* kind, const char* module, const char* pattern,
                                         const char* resolve, const char* className, int vtableIndex,
                                         const char* prologue, char* reasonOut, int reasonCap);
typedef int (*s2_engine_call_invoke_fn)(int callId, int entIndex, int entSerial, int subObjOff,
                                        const uint64_t* gp, const unsigned char* gpKind, int gpCount,
                                        const double* fp, int fpCount,
                                        const char* const* strs, const float* vecs,
                                        int retKind, uint64_t* retOut);
/* --- client-command slice: make a client run a console command --- */
/* Clear identity flag bits on a live entity -> flags AFTER the write, or -1 if not live. CLEAR-ONLY
 * (a mask of bits to drop); EF_IS_INVALID_EHANDLE is never clearable. Exists for EF_IN_STAGING_LIST,
 * which SetModel asserts on. */
typedef long long (*s2_ent_identity_flags_clear_fn)(int index, int serial, unsigned int mask);
typedef int (*s2_client_command_fn)(int slot, const char* cmd);
typedef int (*s2_client_fake_command_fn)(int slot, const char* cmd);
/* --- deferred-dispatch selftest (APPENDED after ent_identity_flags_clear; order is the ABI) ---
 * DEV-ONLY, gated on the S2_DEFER_SELFTEST env var on BOTH sides: core installs the
 * `__s2_defer_selftest` native only when it is set, and the shim refuses the call when it is not.
 * Fires ONE synthetic notify dispatch from inside the calling JS native's borrow, so the queue's
 * GAME-EVENT path executes end to end — a path with no natural trigger before A5b (see the block
 * comment on s2_defer_selftest in s2script_mm.cpp). MUST NOT be armed in production: it dispatches
 * a real event name with FAKE fields to every subscribed plugin.
 *   1 = the dispatch reported DEFERRED and a duplicate was queued (the path ran)
 *   0 = refused or degraded (gate off, no event manager, CreateEvent failed, duplication unavailable)
 *  -1 = the dispatch was NOT deferred — the isolate was free, so the run proves nothing */
typedef int (*s2_defer_selftest_fn)(void);

/* --- voice hearability slice (APPENDED after engine_call_invoke; order is the ABI) ---
 * Per-SENDER "who may hear me" bitmask, enforced by the SetClientListening PRE hook. The core owns
 * the policy (owner -> sender -> mask, AND-merged) and pushes only the merged result, so the hot
 * path stays a shift and a test — no FFI, no JS, no allocation.
 * voice_audible_set:   1 = applied; 0 = voice degraded OR sender out of range.
 * voice_audible_clear: 1 = a rule was present and removed; 0 = nothing to remove OR degraded (both
 *                      mean "no rule is in force afterwards", the only thing a caller can act on).
 * voice_audible_stats: out[3] = {calls, entries, rewrites}; 1 = written, 0 = null out. */
typedef int (*s2_voice_audible_set_fn)(int sender, uint64_t mask);
typedef int (*s2_voice_audible_clear_fn)(int sender);
typedef int (*s2_voice_audible_stats_fn)(uint64_t* out);

/* UserMessage-interception slice — APPENDED after voice_get_muted; order is the ABI.
 * usermsg_hook_sub: resolve an unscoped message name (FindNetworkMessagePartial, the live-proven
 * SayText2 path), VALIDATE the m_MessageId extraction fail-closed (non-null NetMessageInfo, id in
 * [0,2048), requested name a substring of GetUnscopedName), lazily SH_ADD_HOOK PostEventAbstract on
 * the first-ever sub, set the id's bitmap bit, write the canonical unscoped name into canonicalOut.
 * Returns the id, or -1 with a named USERMSG reason logged. All read ops target the BLOCK-SCOPED
 * current intercepted message (null-guarded; valid only during a dispatch). */
typedef int (*s2_usermsg_hook_sub_fn)(const char* name, char* canonicalOut, int canonicalLen);
typedef int (*s2_usermsg_hook_unsub_fn)(int id);
typedef int (*s2_usermsg_hook_read_int_fn)(const char* path, long long* out);
typedef int (*s2_usermsg_hook_read_float_fn)(const char* path, double* out);
typedef int (*s2_usermsg_hook_read_string_fn)(const char* path, char* buf, int buflen);
typedef int (*s2_usermsg_hook_has_field_fn)(const char* path);
typedef int (*s2_usermsg_hook_recipients_fn)(unsigned long long* outMask);
typedef int (*s2_usermsg_hook_debug_fn)(char* buf, int buflen);

/* Declarative inbound hooks — APPENDED after defer_selftest; order is the ABI. Implemented in
 * shim/src/engine_hooks.{h,cpp}; these MUST stay signature-identical to the declarations there.
 * Engine-generic: the address arrives already RESOLVED (through engine_call_resolve, off the
 * plugin's own gamedata) and the shape arrives as an id from the closed vocabulary in
 * shim/src/hook_dispatch.h, so no game identifier reaches the core.
 * hook_install: lazily detour `addr` with hook `hookId`'s own compiled thunk for `shape`. 0 = the
 *   hook is (now, or already) installed; -1 with a NAMED reason in reasonOut, which the core stores
 *   verbatim as that hook's degrade reason. Idempotent per id — core calls it on every subscribe.
 * hook_arm_bypass: arm the hook's one-shot bypass latch immediately BEFORE core invokes the
 *   `bypassWith` call descriptor, so our own outbound call does not fire our own hook (SourceMod's
 *   g_pIgnoreTerminateDetour semantic, and what keeps a hook from firing while core holds the
 *   isolate borrow). Out-of-range ids are a silent no-op.
 * hook_disarm_bypass: clear it again, immediately AFTER that invoke. Not redundant with the thunk's
 *   take: an invoke that never reaches the hooked function (degraded descriptor, stale receiver)
 *   would otherwise leave the latch armed and swallow the next GENUINE engine-driven call. Core
 *   cannot clear it any other way — the latch lives in the shim.
 * the hook_read/hook_write pairs and hook_receiver_handle: the BLOCK-SCOPED arg view (engine_hooks.h).
 *   (Spelled out rather than globbed: a `hook_read_<star>` in a C comment would close it here.) `idx`
 *   is the descriptor's positional param index; every accessor is liveness-, bounds- and
 *   class-checked shim-side and returns -1 rather than reinterpreting bits, which core surfaces as a
 *   NAMED degrade of that hook — a handler reading a param must never get a plausible-looking 0 when
 *   the read actually failed. `argView` is opaque to core: it is a THUNK'S STACK FRAME and core only
 *   ever hands it back here.
 * engine_call_address: the resolved absolute address behind an engine_call_resolve id (0 = unknown
 *   id). A hook resolves through the SAME descriptor path as a call, but then has to patch bytes,
 *   and S2_HookInstall takes an address rather than an id. Core treats it as an opaque token and
 *   never dereferences it; hook_install re-proves the whole patch window is executable. */
typedef int  (*s2_hook_install_fn)(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap);
typedef void (*s2_hook_arm_bypass_fn)(int hookId);
typedef void (*s2_hook_disarm_bypass_fn)(int hookId);
typedef int  (*s2_hook_read_f32_fn)(void* argView, int idx, float* out);
typedef int  (*s2_hook_read_i32_fn)(void* argView, int idx, int32_t* out);
typedef int  (*s2_hook_write_f32_fn)(void* argView, int idx, float value);
typedef int  (*s2_hook_write_i32_fn)(void* argView, int idx, int32_t value);
typedef int  (*s2_hook_receiver_handle_fn)(void* argView, uint32_t* outHandle);
typedef int64_t (*s2_engine_call_address_fn)(int callId);

/* The C-ABI engine-ops table. Field ORDER is the ABI: this struct and `S2EngineOps` in
 * core/src/v8host.rs must stay index-for-index identical and must change in the SAME commit.
 *
 * CONVENTION, amended in A5b: append-only ACROSS a release boundary, not WITHIN one. The
 * per-field `APPENDED after <field>` markers below record where each slice landed and still forbid
 * reordering — but they never made a field immortal. Core and the shim ship in the same zip and
 * nothing outside this repo links this table, so a slice that RETIRES an op deletes its field
 * outright and renumbers the markers that named it. A5b did exactly that for eight CS2-specific
 * calls (CommitSuicide, ChangeTeam, SwitchTeam, TerminateRound, Respawn, SetPawn, GiveNamedItem,
 * RemovePlayerItem), now `calls` descriptors in gamedata/cs2/ served by the generic
 * engine_call_resolve/engine_call_invoke pair further down. */
typedef struct {
    s2_schema_offset_fn       schema_offset;
    s2_ent_by_index_fn        ent_by_index;
    s2_deref_handle_fn        deref_handle;
    s2_ent_state_changed_fn   ent_state_changed;
    s2_concommand_register_fn concommand_register;
    s2_schema_enumerate_fn    schema_enumerate;
    /* Game-event ops (Slice 5D.1) — MUST remain in this order; mirrors S2EngineOps in core/src/v8host.rs */
    s2_event_subscribe_fn     event_subscribe;
    s2_event_unsubscribe_fn   event_unsubscribe;
    s2_event_get_int_fn       event_get_int;
    s2_event_get_float_fn     event_get_float;
    s2_event_get_bool_fn      event_get_bool;
    s2_event_get_string_fn    event_get_string;
    s2_event_get_uint64_fn    event_get_uint64;
    s2_event_get_player_slot_fn event_get_player_slot;
    /* Engine-identity ops (Slice 5D.2) — APPENDED after the event ops; order is the ABI. */
    s2_client_valid_fn          client_valid;
    s2_client_userid_fn         client_userid;
    s2_client_signon_fn         client_signon;
    s2_client_name_fn           client_name;
    s2_client_find_by_userid_fn client_find_by_userid;
    /* Event write/fire ops (Slice 5D.3) — APPENDED after the client ops; order is the ABI. */
    s2_event_set_int_fn    event_set_int;
    s2_event_set_float_fn  event_set_float;
    s2_event_set_bool_fn   event_set_bool;
    s2_event_set_string_fn event_set_string;
    s2_event_set_uint64_fn event_set_uint64;
    s2_event_create_fn     event_create;
    s2_event_fire_fn       event_fire;
    /* Config ops (Slice 5E.2) — APPENDED after the event ops; order is the ABI. */
    s2_config_read_fn  config_read;
    s2_config_write_fn config_write;
    /* Chat messaging (Slice 6.1) — APPENDED after config ops; order is the ABI. */
    s2_client_print_fn client_print;   /* Slice 6.1 — APPENDED after config ops; order is the ABI. */
    /* Client SteamID (Slice 6.2) — APPENDED after client_print; order is the ABI. */
    s2_client_steamid_fn client_steamid;
    /* Client kick (Slice 6.3) — APPENDED after client_steamid; order is the ABI. */
    s2_client_kick_fn client_kick;
    /* Server command + map-validity (Slice 6.4) — APPENDED after client_kick; order is the ABI. */
    s2_server_command_fn   server_command;
    s2_server_map_valid_fn server_map_valid;
    s2_damage_read_float_fn  damage_read_float;
    s2_damage_read_int_fn    damage_read_int;
    s2_damage_write_float_fn damage_write_float;
    s2_damage_victim_fn      damage_victim;
    s2_cvar_get_fn           cvar_get;
    /* Console print + client address (ban-reason sub-project 2) — APPENDED after cvar_get; order is the ABI. */
    s2_client_console_print_fn client_console_print;
    s2_client_address_fn       client_address;
    /* Server-info ops (reservedslots+basetriggers) — APPENDED after client_address; order is the ABI. */
    s2_server_max_clients_fn server_max_clients;
    s2_server_map_name_fn    server_map_name;
    s2_server_game_time_fn   server_game_time;
    /* Slice DB — APPENDED after server_game_time; order is the ABI. */
    s2_db_data_dir_fn db_data_dir;
    /* Slice menu: per-client event fire — APPENDED after db_data_dir; order is the ABI. */
    s2_event_fire_to_client_fn event_fire_to_client;
    /* Slice nominations: raw configs-dir file read/write — APPENDED after event_fire_to_client; order is the ABI. */
    s2_config_read_file_fn  config_read_file;
    s2_config_write_file_fn config_write_file;
    /* Ray-trace slice — APPENDED after config_write_file; order is the ABI. */
    s2_trace_shape_fn trace_shape;
    /* Entity-creation lifecycle slice — APPENDED after trace_shape; order is the ABI. */
    s2_entity_create_fn   entity_create;
    s2_entity_spawn_fn    entity_spawn;
    s2_entity_teleport_fn entity_teleport;
    s2_entity_remove_fn   entity_remove;
    /* Item slice — APPENDED after entity_remove; order is the ABI. */
    s2_entity_subobj_vcall_fn       entity_subobj_vcall;
    s2_entity_read_handle_vector_fn entity_read_handle_vector;
    /* Entity-I/O slice — APPENDED after entity_read_handle_vector; order is the ABI. */
    s2_entity_fire_input_fn entity_fire_input;
    /* EKV slice — APPENDED after entity_fire_input; order is the ABI. */
    s2_entity_spawn_kv_fn entity_spawn_kv;
    /* Game-rules + UserMessage slice — APPENDED after entity_spawn_kv; order is the ABI. */
    s2_entity_find_by_class_fn entity_find_by_class;
    /* UserMessage send family — APPENDED after entity_find_by_class; order is the ABI. */
    s2_user_message_create_fn     user_message_create;
    s2_user_message_set_int_fn    user_message_set_int;
    s2_user_message_set_float_fn  user_message_set_float;
    s2_user_message_set_string_fn user_message_set_string;
    s2_user_message_set_bool_fn   user_message_set_bool;
    s2_user_message_send_fn       user_message_send;
    /* FakeConVar (clientlist-fakeconvar-onmapstart slice) — APPENDED after user_message_send; order is the ABI. */
    s2_convar_register_fn convar_register;
    /* Translations slice — APPENDED after convar_register; order is the ABI. */
    s2_translations_read_fn  translations_read;
    s2_client_language_fn    client_language;
    /* Zones real-trigger slice — APPENDED after client_language (the struct tail); order is the ABI. */
    s2_collision_activate_fn collision_activate;
    /* Zones real-trigger slice — APPENDED after collision_activate; order is the ABI. */
    s2_entity_set_model_fn entity_set_model;
    /* Entity lifecycle listeners slice — APPENDED after entity_set_model; order is the ABI. */
    s2_entity_listener_install_fn entity_listener_install;
    /* entity_name slice — APPENDED after entity_listener_install; order is the ABI; do not reorder above. */
    s2_entity_name_fn entity_name;
    /* Sound slice — APPENDED after entity_name (the struct tail); order is the ABI. */
    s2_sound_emit_fn         sound_emit;
    s2_sound_precache_add_fn sound_precache_add;
    /* usercmd slice — APPENDED after sound_precache_add; order is the ABI; do not reorder above. */
    s2_usercmd_hook_install_fn  usercmd_hook_install;
    s2_usercmd_read_fn          usercmd_read;
    s2_usercmd_write_fn         usercmd_write;
    s2_usercmd_read_buttons_fn  usercmd_read_buttons;
    s2_usercmd_write_buttons_fn usercmd_write_buttons;
    s2_usercmd_clear_subtick_fn usercmd_clear_subtick;
    /* checktransmit slice — APPENDED after usercmd_clear_subtick; order is the ABI; do not reorder above. */
    s2_transmit_set_fn   transmit_set;
    s2_transmit_clear_fn transmit_clear;
    s2_transmit_stats_fn transmit_stats;
    /* Voice-control slice — APPENDED after transmit_stats; order is the ABI. */
    s2_voice_set_muted_fn  voice_set_muted;
    s2_voice_get_muted_fn  voice_get_muted;
    /* UserMessage-interception slice — APPENDED after voice_get_muted; order is the ABI. */
    s2_usermsg_hook_sub_fn         usermsg_hook_sub;
    s2_usermsg_hook_unsub_fn       usermsg_hook_unsub;
    s2_usermsg_hook_read_int_fn    usermsg_hook_read_int;
    s2_usermsg_hook_read_float_fn  usermsg_hook_read_float;
    s2_usermsg_hook_read_string_fn usermsg_hook_read_string;
    s2_usermsg_hook_has_field_fn   usermsg_hook_has_field;
    s2_usermsg_hook_recipients_fn  usermsg_hook_recipients;
    s2_usermsg_hook_debug_fn       usermsg_hook_debug;
    /* Crash-reporter slice — APPENDED after usermsg_hook_debug; order is the ABI. */
    s2_server_build_number_fn server_build_number;
    /* Crash-harness — APPENDED after server_build_number; order is the ABI. */
    s2_crash_test_native_fn crash_test_native;
    /* E1 entity-liveness slice — MUST remain in this order; mirrors S2EngineOps in core/src/v8host.rs */
    s2_ent_resolve_fn        ent_resolve;        /* (index, engine_serial) -> CEntityInstance* | NULL — identity-CHUNK validated */
    s2_ent_identity_flags_fn ent_identity_flags; /* (index, engine_serial) -> m_flags (>=0) | -1 stale/absent */
    s2_ent_snapshot_fn       ent_snapshot;       /* fill live (index, serial) pairs; returns TOTAL found (may exceed cap) */
    /* Plugin-declared engine calls — APPENDED after ent_snapshot; order is the ABI; do not reorder above. */
    s2_engine_call_resolve_fn engine_call_resolve;
    s2_engine_call_invoke_fn  engine_call_invoke;
    /* voice hearability slice — APPENDED after engine_call_invoke; order is the ABI; do not reorder above. */
    s2_voice_audible_set_fn   voice_audible_set;
    s2_voice_audible_clear_fn voice_audible_clear;
    s2_voice_audible_stats_fn voice_audible_stats;
    /* --- client-command slice (APPENDED after voice_audible_stats; order is the ABI) --- */
    s2_client_command_fn      client_command;
    s2_client_fake_command_fn client_fake_command;
    /* identity-flag clear — APPENDED after client_fake_command; order is the ABI; do not reorder. */
    s2_ent_identity_flags_clear_fn ent_identity_flags_clear;
    /* deferred-dispatch selftest (DEV-ONLY) — APPENDED after ent_identity_flags_clear; order is
     * the ABI; do not reorder above. */
    s2_defer_selftest_fn defer_selftest;
    /* declarative inbound hooks — APPENDED after defer_selftest; order is the ABI; do not reorder
     * above. Implemented in shim/src/engine_hooks.cpp. */
    s2_hook_install_fn    hook_install;
    s2_hook_arm_bypass_fn hook_arm_bypass;
    /* declarative inbound hooks, core half — APPENDED after hook_arm_bypass; order is the ABI; do
     * not reorder above. hook_disarm_bypass closes the latch-leak window (spec §10); the five
     * accessors are how the block-scoped arg view reaches JS at all; engine_call_address is what
     * turns a resolved descriptor into an installable address. */
    s2_hook_disarm_bypass_fn   hook_disarm_bypass;
    s2_hook_read_f32_fn        hook_read_f32;
    s2_hook_read_i32_fn        hook_read_i32;
    s2_hook_write_f32_fn       hook_write_f32;
    s2_hook_write_i32_fn       hook_write_i32;
    s2_hook_receiver_handle_fn hook_receiver_handle;
    s2_engine_call_address_fn  engine_call_address;
    /* Entity-property slice — APPENDED after engine_call_address; order is the ABI; do not reorder
     * above. Five engine-generic CBaseEntity/CBaseModelEntity setters with no usable schema-write
     * route (the gravity setter early-returns on an unchanged value and maintains a second field;
     * writing m_vecAbsVelocity directly skips the physics/partition update; the body-group choices
     * are a CUtlOrderedMap, not a scalar). All serial-gated, all return 1 on success / 0 if the
     * signature was unresolved or the ref is stale. */
    s2_entity_set_gravity_scale_fn          entity_set_gravity_scale;
    s2_entity_apply_abs_velocity_impulse_fn entity_apply_abs_velocity_impulse;
    s2_entity_stop_sound_fn                 entity_stop_sound;
    s2_entity_set_body_group_by_name_fn     entity_set_body_group_by_name;
    s2_entity_set_model_scale_fn            entity_set_model_scale;
    /* ICvar set — APPENDED after entity_set_model_scale; order is the ABI. Write through
     * ConVarData (same layout as cvar_get), not ServerCommand. 1 = applied, 0 = absent / bad type. */
    s2_cvar_set_fn cvar_set;
} S2EngineOps;

/* Returned by a NOTIFY dispatch entry when the JS isolate was already borrowed (a re-entrant
 * dispatch — a handler made the engine synchronously dispatch back into core): core delivered
 * NOTHING and the caller must queue a replay for the next GameFrame. Any other value means the
 * entry was handled (delivered, or there were no subscribers) — do not queue.
 *
 * Test it EXACTLY (`== S2_DISPATCH_DEFERRED`). Never `< 0`, never `if (r)` truthiness: -1000 is
 * C-truthy, which is why no entry whose result is consumed as a boolean may ever be made
 * deferrable. Negative on purpose — a future deferrable entry read as `if (r >= 2) SUPERCEDE`
 * degrades to today's "engine proceeds unhooked" if a call site forgets the check, instead of
 * superseding a message it never meant to.
 *
 * MUST stay byte-identical to `S2_DISPATCH_DEFERRED` in core/src/ffi.rs. */
#define S2_DISPATCH_DEFERRED (-1000)

/* ops may be null -> all engine natives degrade.  The core copies the struct by
 * value at init; the caller's storage need not outlive the call. */
int  s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook, const S2EngineOps* ops);
int  s2script_core_eval(const char* utf8_js);
int  s2script_core_dispatch_game_frame(int phase, int simulating, int first, int last); /* phase 0=Pre,1=Post; returns collapsed HookResult */
void s2script_core_shutdown(void);
/* Shim -> core: called by the ConCommand trampoline when a registered command fires.
 * name = Arg(0) (command name), slot = CPlayerSlot::Get() (-1 for server console),
 * args = ArgS() (everything after the command name). */
void s2script_core_dispatch_concommand(const char* name, int slot, const char* args);
/* Shim -> core: called by the Host_Say detour for every player chat line (Slice 6.11b).
 * slot = the speaker's player slot (controller entity index - 1), text = CCommand::Arg(1)
 * (the raw message), teamonly = 1 for team-only chat else 0. Parses a `!cmd` / `/cmd` trigger and
 * dispatches the matching command; a non-command line is delivered to the raw Chat.onMessage
 * subscribers (Slice 6.13b) with (slot, text, teamonly). Returns 1 if the caller should SUPPRESS the
 * chat broadcast (a matched silent `/` trigger, OR a raw subscriber that returned >= Handled), else 0
 * (the public `!` trigger and ordinary chat with no blocking subscriber show). */
int s2script_core_dispatch_chat(int slot, const char* text, int teamonly);
/* Shim -> core: called by the ClientCommand hook when a player types a command at the console
 * (Slice 6.11c). slot = the player's slot, name = CCommand::Arg(0), args = ArgS(). Dispatches the
 * matching registered command. Returns 1 if handled (the caller SUPERCEDEs the engine's handling). */
int s2script_core_dispatch_client_command(int slot, const char* name, const char* args);
/* Shim -> core: called by the ICvar::DispatchConCommand hook for EVERY ConCommand dispatch — the
 * AddCommandListener seam. slot = CCommandContext::GetPlayerSlot(), name = CCommand::Arg(0),
 * args = ArgS(). Runs the `Commands.onClientCommand(name, ...)` listeners ONLY; s2script's own
 * registered commands are driven by their ConCommand trampoline and must not be dispatched twice.
 * Returns 1 iff a listener returned >= Handled (the caller SUPERCEDEs), else 0 — observe-by-default,
 * because superseding an engine command like `player_ping` would stop it doing its own work. */
int s2script_core_dispatch_command_listeners(int slot, const char* name, const char* args);
/* Shim -> core: take (and clear) the recipient allow-mask a plugin set with Events.setRecipients during
 * the current game-event pre-dispatch. Returns 1 and writes *out_mask when a mask was set; 0 means the
 * plugin expressed no opinion and the caller must NOT filter. */
int s2script_core_take_event_recipients(uint64_t* out_mask);

/* ---------------------------------------------------------------------------------------------
 * The five DEFERRABLE notify entries, and their replays (deferred-dispatch-queue slice).
 *
 * Each of the five is SPLIT in two, and the split is part of the ABI contract:
 *
 *     s2script_core_dispatch_X(args) = host bookkeeping (ALWAYS runs, NEVER deferred)
 *                                    + s2script_core_replay_X(args)   // the JS fan-out, and nothing else
 *
 * The shim queues the REPLAY entry, never the dispatch entry, because the bookkeeping halves are
 * not idempotent: replaying map_start's entity-books clear would wipe the books a frame INTO the
 * new map, a replayed entity "create" would resurrect a since-deleted entity, and a replayed
 * client_event would double-count the crash breadcrumb's player total.
 *
 * All ten return S2_DISPATCH_DEFERRED when the isolate was already borrowed, and 0 otherwise
 * (delivered, no subscribers, or any degrade path — a null pointer, invalid UTF-8 or a caught
 * panic is NOT deferrable). A replay that itself reports DEFERRED is a bug (the drain runs with the
 * isolate provably free); the shim drops it with a named reason and never re-queues it.
 * --------------------------------------------------------------------------------------------- */

/**
 * A cvar's value changed (ICvar global change callback). NOTIFY-only — the engine has already
 * applied the value by the time this fires, so there is no return value to veto with.
 */
int s2script_core_dispatch_cvar_change(const char* name, const char* newValue, const char* oldValue);
/* Replay of a deferred cvar-change: the JS fan-out only (this path carries no bookkeeping; the
 * entry exists so the shim's drain has one uniform replay vocabulary). */
int s2script_core_replay_cvar_change(const char* name, const char* newValue, const char* oldValue);
/* Shim -> core: called by the seven client lifecycle hooks (@s2script/clients sub-project). name is
 * one of connect/putinserver/active/fullyconnect/disconnect/settingschanged/voice; slot is the
 * player's slot (CPlayerSlot::Get()). Notify-only: runs the JS Clients.on(name) subscribers. */
int s2script_core_dispatch_client_event(const char* name, int slot);
/* Replay of a deferred client-lifecycle event: JS fan-out only. The breadcrumb player count and the
 * voice slot-reuse clear already ran, unconditionally, at dispatch time. NOTE a deferred
 * "disconnect" arrives AFTER the shim cleared the slot, so the handler sees the client invalid. */
int s2script_core_replay_client_event(const char* name, int slot);
/* Shim -> core: the INetworkServerService::StartupServer POST hook reports a map start with the
 * live map name (clientlist-fakeconvar-onmapstart slice). Notify-only: runs the JS Server.onMapStart
 * subscribers. catch_unwind-wrapped; a null pointer degrades to "" (never panic across the boundary). */
int s2script_core_dispatch_map_start(const char* map);
/* Replay of a deferred map start: JS fan-out only — it must NEVER touch the entity books. */
int s2script_core_replay_map_start(const char* map);
/* Shim -> core: an IEntityListener callback (create/spawn/delete) reports an entity by its packed
 * CEntityHandle (ToInt()) + class name. Notify-only; core builds a serial-gated EntityRef. */
int s2script_core_dispatch_entity_event(const char* kind, const char* className, int handle);
/* Replay of a deferred entity-lifecycle event: JS fan-out only (the books feed ran at dispatch
 * time). A replayed "delete" hands the handler a null EntityRef — the books already say dead —
 * which is the same books-gated degrade any stale ref gets, and the className still names what died. */
int s2script_core_replay_entity_event(const char* kind, const char* className, int handle);
/* Shim -> core: the CGameRulesGameSystem::OnPrecacheResource manual hook reports the session
 * resource-manifest build (Sound slice). The shim stashes the live IResourceManifest* around this
 * call so the sound_precache_add op can AddResource into it; the stash is cleared when this
 * returns (block-scoped — a handler must use its PrecacheContext synchronously). Notify-only:
 * runs the JS Sound.onPrecache subscribers. */
void s2script_core_dispatch_precache(void);
/* Shim -> core: is `xuid` currently banned? (Slice 6.18). Called by the ClientConnect hook with the
 * connecting player's SteamID64 and the current unix time. Returns 1 iff banned (perm or unexpired); on a
 * hit, the ban reason is bounded-copied (NUL-terminated) into out_reason for the shim's log line. Panic ->
 * 0 (fail-open: a core bug must never wedge all connections). */
int s2script_core_ban_check(uint64_t xuid, int64_t now, char* out_reason, int cap);
/* Shim -> core: called by the IGameEventListener2 trampoline when a game event fires.
 * name = ev->GetName().  During this call the shim's s_currentEvent is set so the
 * event accessor ops (event_get_int / float / bool / string / uint64 / player_slot)
 * read live data from the current IGameEvent*.  After dispatch returns, s_currentEvent
 * is restored to its previous value (re-entrancy guard). */
int s2script_core_dispatch_game_event(const char* name);
/* Replay of a deferred game event: JS fan-out only. The shim has pointed s_currentEvent at its own
 * DuplicateEvent copy for the duration of this call, so handlers read the REAL field values (the
 * engine's original IGameEvent died when the original dispatch returned); the copy is FreeEvent'd
 * after, under RAII, so a throwing handler cannot leak it. */
int s2script_core_replay_game_event(const char* name);
// Slice 6.6 Stage 2: run the Damage.onPre subscribers over the current CTakeDamageInfo (set by the shim
// detour). Handlers read/modify the live info in place (setting damage to 0 = block).
void s2script_core_dispatch_damage(void);
/* Shim -> core: called by the FireOutputInternal detour (entity-I/O slice) with the firing entity's
 * classname, the output name, packed activator/caller CEntityHandle ints (-1 = none), the output's
 * value as a string, and the delay. Runs the matching Entity.onOutput subscribers SYNCHRONOUSLY
 * (key match on (class,output)/(class,"*")/("*",output)/("*","*")) and collapses their returned
 * HookResults via run_chain; the caller supersedes the original FireOutputInternal (suppresses the
 * output) when the returned value is >= Handled (2). Returns the collapsed HookResult (0 Continue ..
 * 3 Stop). catch_unwind -> 0 (fail-open: a core bug must never suppress an output it didn't mean to). */
int s2script_core_dispatch_output(const char* classname, const char* output, int actHandle, int callerHandle,
                                  const char* value, float delay);
/* Shim -> core: called by the FireEvent Pre hook (Slice 5D.3). Runs the PRE subscribers for `name`
 * (s_currentEvent is set + mutable during the call). Returns 1 to suppress the client broadcast
 * (a pre-hook returned Handled/Stop), else 0. */
int s2script_core_dispatch_game_event_pre(const char* name);
/* Shim -> core: called by the ProcessUsercmds detour once per in-flight CUserCmd (usercmd primitive).
 * slot = the firing player's controller slot (derived shim-side from the detour's `this`). The shim
 * sets s_currentUserCmd (the live CSGOUserCmdPB message) BEFORE calling this and clears it after —
 * the JS UserCmd.onRun subscribers read/modify the live message in place via the usercmd_read/write/
 * read_buttons/write_buttons/clear_subtick ops during this call. Returns the collapsed HookResult (0
 * Continue .. 3 Stop); the caller neutralizes (zeroes) the cmd's movement/buttons when >= 2 (Handled)
 * and always still calls the original trampoline (server-authoritative — a suppressed cmd is a ZEROED
 * cmd, not a skipped call). catch_unwind -> 0 (fail-open: a core bug must never corrupt player input). */
int s2script_core_dispatch_usercmd(int slot);
/* Shim -> core: called by the PostEventAbstract PRE hook on a bitmap-hit outbound user message
 * (usermsg-hook slice). name = the message's canonical GetUnscopedName() (the dispatch key), id =
 * its m_MessageId. The shim sets the block-scoped current-message statics BEFORE this call and nulls
 * them after — the JS UserMessages.onPre subscribers read the live message via the usermsg_hook_read_*
 * / recipients / debug ops during this call. Returns the collapsed HookResult (0 Continue .. 3 Stop);
 * the caller MRES_SUPERCEDEs the send when >= Handled (2). catch_unwind -> 0 (fail-open: a core bug
 * must never suppress a message it didn't mean to). */
int s2script_core_dispatch_usermsg(const char* name, int id);
/* Shim -> core: an installed DECLARATIVE INBOUND HOOK fired. Called from the hook's compiled thunk
 * (shim/src/engine_hooks.cpp) with the hook id the thunk carries as a compile-time constant and an
 * OPAQUE pointer to the thunk's own stack-frame arg view — core reads and writes it only through the
 * S2_HookRead/S2_HookWrite accessors in engine_hooks.h, and it dies with the frame, so nothing it
 * points at can outlive the dispatch. Returns the collapsed HookResult (0 Continue .. 3 Stop); the
 * thunk suppresses the original engine call entirely when >= Handled (2).
 *
 * DECLARED WEAK ON PURPOSE. The shim and the core are two separate .so files. A non-weak reference to
 * a core entry the resident libs2script_core.so does not define is an undefined symbol at dlopen,
 * which takes down the WHOLE addon — every plugin, on a live server — and that is the one failure mode
 * this project refuses (degrade per-descriptor, never crash globally). Weak makes a mismatched pair
 * resolve to null instead; S2Hook_SetOps then receives a null dispatch, S2Hook_Dispatch returns
 * Continue, and Load logs the miss BY NAME. */
int s2script_core_dispatch_hook(int hookId, void* argView) __attribute__((weak));
/* Retained for shim link-compatibility; now a no-op (game JS is provided via
 * s2script_core_register_package instead).  Safe to call; does nothing. */
void s2script_core_load_cs2(const char* path);
/* Register a game-package JS source under `name` so core can inject it into each
 * plugin context at runtime without baking game JS into the core binary.
 * name and js must be null-terminated UTF-8.  Null pointers degrade to a no-op. */
void s2script_core_register_package(const char* name, const char* js);
/* Hand core the same game package's own GAMEDATA (A5b): the merged `signatures` + `calls` the
 * shim's one loader already produced for that owner (GameConfig::mergedJson), as JSON text.
 * `name` is the SAME string passed to s2script_core_register_package. Core registers the `calls`
 * descriptors under a reserved owner id derived from it — an identity no .s2sp can claim — and
 * exempts them from the engine:calls operator allow-list (first-party runtime, not a plugin).
 * Both pointers must be null-terminated UTF-8; null degrades to a no-op, and an empty json is the
 * normal "this owner declares no calls" state, not an error. */
void s2script_core_register_package_gamedata(const char* name, const char* gamedata_json);
/* Set the plugins directory for the .s2sp watcher.  Called once by the shim at
 * load time with the resolved addons/s2script/plugins/ path (dladdr-derived).
 * path must be null-terminated UTF-8.  A null pointer degrades to a no-op. */
void s2script_core_set_plugins_dir(const char* path);

/* Crash reporter: the breadcrumb POD base pointer + byte size. The shim's crash callback
 * dumps exactly this many raw bytes with a single write() (signal-safe; no field access). */
const uint8_t* s2script_core_crash_breadcrumb(void);
uint32_t       s2script_core_crash_breadcrumb_size(void);
/* Crash reporter: push the treadmill identity block + the crash-spool dir (called once in
 * Load, after s2script_core_init). gd_fail_count > 0 marks gamedata stale. */
void s2script_core_crash_set_identity(const char* gamedata_fingerprint,
                                      const char* gamedata_generated_at,
                                      const char* hl2sdk_build,
                                      const char* schema_build,
                                      int gamedata_fail_count,
                                      const char* spool_dir);

#ifdef __cplusplus
}
#endif
#endif /* S2SCRIPT_CORE_H */
