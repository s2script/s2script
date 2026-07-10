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
 * A null parent/name/inner is an absent value. */
typedef void (*s2_emit_class_fn)(void* ctx, const char* name, const char* parent);
typedef void (*s2_emit_field_fn)(void* ctx, const char* cls, const char* name, int offset,
                                 const char* kind, const char* type_name, const char* inner);
typedef int  (*s2_schema_enumerate_fn)(void* ctx, s2_emit_class_fn emit_class, s2_emit_field_fn emit_field);

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
// Slice 6.14: kill a player pawn via CBasePlayerPawn::CommitSuicide (a sig-resolved DIRECT call — NOT a
// vtable index). The shim reconstructs + serial-gates the pawn from (idx, serial); no-op on a stale ref or
// an unresolved signature. Calls CommitSuicide(pawn, /*bExplode=*/false, /*bForce=*/true).
typedef void (*s2_pawn_commit_suicide_fn)(int idx, int serial);
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

/* Item slice — APPENDED after entity_remove; order is the ABI.
 * give_named_item: (index,serial) of a pawn + a subObjOffset (m_pItemServices, live-schema-resolved
 * JS-side) + a className -> a packed CEntityHandle (ToInt) of the created weapon, 0 on failure.
 * entity_subobj_vcall: (index,serial) of an entity + a subObjOffset + a vtableIndex (.text-validated
 * shim-side) + an optional (argIdx,argSerial) entity arg (-1,-1 = no arg) -> 1/0 success.
 * remove_player_item: (pawnIndex,pawnSerial,weaponIndex,weaponSerial) -> 1/0 success.
 * entity_read_handle_vector: (index,serial) + a pointer-deref chain (ptrOffs[ptrCount]) + a
 * vectorOff (CUtlVector base) + a maxCount cap -> fills outHandles[] with packed CEntityHandles,
 * returns the element count written (<= maxCount), 0 on any unresolved step. */
typedef int (*s2_give_named_item_fn)(int index, int serial, int subObjOffset, const char* className);
typedef int (*s2_entity_subobj_vcall_fn)(int index, int serial, int subObjOffset, int vtableIndex, int argIndex, int argSerial);
typedef int (*s2_remove_player_item_fn)(int pawnIndex, int pawnSerial, int weaponIndex, int weaponSerial);
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
    /* Pawn suicide (Slice 6.14) — APPENDED after cvar_get; order is the ABI. */
    s2_pawn_commit_suicide_fn pawn_commit_suicide;
    /* Console print + client address (ban-reason sub-project 2) — APPENDED after pawn_commit_suicide; order is the ABI. */
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
    s2_give_named_item_fn           give_named_item;
    s2_entity_subobj_vcall_fn       entity_subobj_vcall;
    s2_remove_player_item_fn        remove_player_item;
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
} S2EngineOps;

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
/* Shim -> core: called by the six ISource2GameClients lifecycle hooks (@s2script/clients sub-project).
 * name is one of connect/putinserver/active/fullyconnect/disconnect/settingschanged; slot is the
 * player's slot (CPlayerSlot::Get()). Notify-only: runs the JS Clients.on(name) subscribers. */
void s2script_core_dispatch_client_event(const char* name, int slot);
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
void s2script_core_dispatch_game_event(const char* name);
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
/* Retained for shim link-compatibility; now a no-op (game JS is provided via
 * s2script_core_register_package instead).  Safe to call; does nothing. */
void s2script_core_load_cs2(const char* path);
/* Register a game-package JS source under `name` so core can inject it into each
 * plugin context at runtime without baking game JS into the core binary.
 * name and js must be null-terminated UTF-8.  Null pointers degrade to a no-op. */
void s2script_core_register_package(const char* name, const char* js);
/* Set the plugins directory for the .s2sp watcher.  Called once by the shim at
 * load time with the resolved addons/s2script/plugins/ path (dladdr-derived).
 * path must be null-terminated UTF-8.  A null pointer degrades to a no-op. */
void s2script_core_set_plugins_dir(const char* path);

#ifdef __cplusplus
}
#endif
#endif /* S2SCRIPT_CORE_H */
