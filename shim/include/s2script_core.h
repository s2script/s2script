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

/* Engine-identity ops (Slice 5D.2) — read the connected-client list (INetworkServerService ->
 * game server -> CServerSideClient[]) at gamedata offsets. All degrade to safe misses on any null. */
typedef int          (*s2_client_valid_fn)(int slot);          /* 0/1: connected client at slot */
typedef int          (*s2_client_userid_fn)(int slot);         /* engine user-id, or -1 */
typedef int          (*s2_client_signon_fn)(int slot);         /* signon state, or -1 */
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
/* Shim -> core: called by the IGameEventListener2 trampoline when a game event fires.
 * name = ev->GetName().  During this call the shim's s_currentEvent is set so the
 * event accessor ops (event_get_int / float / bool / string / uint64 / player_slot)
 * read live data from the current IGameEvent*.  After dispatch returns, s_currentEvent
 * is restored to its previous value (re-entrancy guard). */
void s2script_core_dispatch_game_event(const char* name);
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
