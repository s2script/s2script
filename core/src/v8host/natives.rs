use super::*;

/// Install the full native API on a context's global object: `console` plus every `__s2_*`
/// primitive and the `__s2require` shim.  Called for BOTH the shared `HOST` context (so the
/// C-ABI `eval` surface keeps `console`/`__s2_concommand` etc.) and every per-plugin context.
/// The native internal names are unchanged from Slice 0–3; the RENAMED, engine-generic API
/// (`OnGameFrame.subscribe`/`delay`/…) is layered on top by the injected prelude (per-context).
pub(super) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    // console = { log: fn }.
    let console_obj = v8::Object::new(scope);
    let log_key = v8::String::new(scope, "log").unwrap();
    let log_fn = v8::Function::new(scope, console_log).unwrap();
    console_obj.set(scope, log_key.into(), log_fn.into());
    let console_key = v8::String::new(scope, "console").unwrap();
    global_obj.set(scope, console_key.into(), console_obj.into());

    // Multiplexer primitives.
    set_native(scope, global_obj, "__s2_subscribe", s2_subscribe);
    set_native(scope, global_obj, "__s2_unsubscribe", s2_unsubscribe);
    // Async timer primitives (Delay / NextTick / NextFrame / threadSleep).
    set_native(scope, global_obj, "__s2_async_stats", s2_async_stats);
    set_native(scope, global_obj, "__s2_delay", s2_delay);
    set_native(scope, global_obj, "__s2_timer_create", s2_timer_create);
    set_native(scope, global_obj, "__s2_timer_kill", s2_timer_kill);
    set_native(scope, global_obj, "__s2_timer_alive", s2_timer_alive);
    set_native(scope, global_obj, "__s2_next_tick", s2_next_tick);
    set_native(scope, global_obj, "__s2_next_frame", s2_next_frame);
    set_native(scope, global_obj, "__s2_thread_sleep", s2_thread_sleep);
    // Schema + entity system.
    set_native(scope, global_obj, "__s2_schema_offset", s2_schema_offset);
    // Perf instrumentation: monotonic ns clock + isolate heap size + force-GC (dev/benchmark).
    set_native(scope, global_obj, "__s2_v8_heap_used", s2_v8_heap_used);
    set_native(scope, global_obj, "__s2_v8_gc", s2_v8_gc);
    set_native(scope, global_obj, "__s2_hrtime_ns", s2_hrtime_ns);
    // Slice 5A: (index, serial) entity natives — serial-gated read/write/valid/decode.
    // The five Slice-3 raw-pointer natives (entity-by-index, deref-handle, ent-read/write-i32,
    // ent-state-changed) were retired in Task 4; callers now use the __s2_ent_ref_* path.
    // __s2_ent_ref_read_i32/__s2_ent_ref_write_i32 (introduced in Slice 5A) were retired in 5B.2 → generic __s2_ent_ref_read/write.
    crate::entity::install_natives(scope, global_obj);
    set_native(scope, global_obj, "__s2_handle_decode", s2_handle_decode);
    set_native(scope, global_obj, "__s2_handle_adopt", s2_handle_adopt);
    crate::commands::install_natives(scope, global_obj);
    // Schema dump (5B.1): drives the shim's schema_enumerate op into a Catalog and writes JSON.
    set_native(scope, global_obj, "__s2_schema_dump", s2_schema_dump);
    // Per-context identity probe + the CJS require shim.
    set_native(scope, global_obj, "__s2_current_plugin", s2_current_plugin);
    // L1 lifecycle v2: awaited-factory settle + reload-handoff consume (design spec §5).
    set_native(scope, global_obj, "__s2_load_settled", s2_load_settled);
    set_native(scope, global_obj, "__s2_load_failed", s2_load_failed);
    set_native(scope, global_obj, "__s2_handoff_take", s2_handoff_take);
    set_native(scope, global_obj, "__s2_scope_dispose", s2_scope_dispose);
    set_native(scope, global_obj, "__s2require", s2require);
    set_native(scope, global_obj, "__s2_crash_set_game", s2_crash_set_game);
    set_native(scope, global_obj, "__s2_server_build", s2_server_build);
    set_native(scope, global_obj, "__s2_crash_test", s2_crash_test);
    // Deferred-dispatch selftest — INSTALLED ONLY when S2_DEFER_SELFTEST is set in the process
    // environment. Gating the INSTALL (not the call) is the point: in a production process the
    // property does not exist on any global, so `typeof __s2_defer_selftest === "undefined"` and
    // there is nothing for a plugin to reach. See `s2_defer_selftest` for why the path it exercises
    // has no natural trigger before A5b.
    if defer_selftest_armed() {
        set_native(scope, global_obj, "__s2_defer_selftest", s2_defer_selftest);
    }
    // Inter-plugin interface primitives (Slice 4.5).
    set_native(scope, global_obj, "__s2_iface_publish", s2_iface_publish);
    set_native(scope, global_obj, "__s2_iface_dep_kind", s2_iface_dep_kind);
    set_native(
        scope,
        global_obj,
        "__s2_iface_is_published",
        s2_iface_is_published,
    );
    set_native(scope, global_obj, "__s2_iface_call", s2_iface_call);
    // Event subscription / emission (Slice 4.5 events half).
    set_native(scope, global_obj, "__s2_iface_on", s2_iface_on);
    set_native(scope, global_obj, "__s2_iface_off", s2_iface_off);
    set_native(scope, global_obj, "__s2_iface_emit", s2_iface_emit);
    // Game-event system (Slice 5D.1): subscribe/unsubscribe + accessor natives.
    crate::events::install_natives(scope, global_obj);
    // Engine-identity client-list natives (Slice 5D.2).
    crate::client::install_natives(scope, global_obj);
    // Translations slice: root/language phrase-file read + the client's cl_language cvar.
    set_native(
        scope,
        global_obj,
        "__s2_translations_read",
        s2_translations_read,
    );
    // Event write/fire (Slice 5D.3): pre-subscribe/unsubscribe + setters + create/fire.
    // Config live-reload (Slice 5E.2): register an onChange handler for this plugin's config file.
    set_native(
        scope,
        global_obj,
        "__s2_config_on_change",
        s2_config_on_change,
    );
    // Client-lifecycle subscriber (Clients sub-project): register a Clients.on* handler.
    // Map-start subscriber (clientlist-fakeconvar-onmapstart slice): register a Server.onMapStart handler.
    set_native(
        scope,
        global_obj,
        "__s2_map_start_subscribe",
        s2_map_start_subscribe,
    );
    // Precache subscriber (Sound slice): register a Sound.onPrecache handler.
    set_native(
        scope,
        global_obj,
        "__s2_precache_subscribe",
        s2_precache_subscribe,
    );

    crate::admin::install_natives(scope, global_obj);
    // Slice 6.18: ban cache natives (engine-generic — a SteamID/ban map, like the admin cache).
    crate::bans::install_natives(scope, global_obj);
    // clientprefs: cookie cache natives (engine-generic — a SteamID/string-KV map, like admin/ban).
    crate::cookies::install_natives(scope, global_obj);
    // Voice-control slice: per-slot voice mute set/get (shim-side flag consulted by the
    // SetClientListening rewrite hook; JS never sits in that hot path).
    set_native(
        scope,
        global_obj,
        "__s2_voice_set_muted",
        s2_voice_set_muted,
    );
    set_native(
        scope,
        global_obj,
        "__s2_voice_get_muted",
        s2_voice_get_muted,
    );
    // ban-reason sub-project 2: developer-console print + client IP address.
    crate::sdkhooks::install_natives(scope, global_obj);
    set_native(
        scope,
        global_obj,
        "__s2_damage_read_float",
        s2_damage_read_float,
    );
    set_native(
        scope,
        global_obj,
        "__s2_damage_read_int",
        s2_damage_read_int,
    );
    set_native(
        scope,
        global_obj,
        "__s2_damage_write_float",
        s2_damage_write_float,
    );
    set_native(scope, global_obj, "__s2_damage_victim", s2_damage_victim);
    set_native(scope, global_obj, "__s2_cvar_get", s2_cvar_get);
    set_native(scope, global_obj, "__s2_cvar_set", s2_cvar_set);
    set_native(
        scope,
        global_obj,
        "__s2_convar_register",
        s2_convar_register,
    );
    // Usercmd primitive Task 2: raw subscribe native (block/read/write natives are Task 3/4).
    set_native(
        scope,
        global_obj,
        "__s2_usercmd_subscribe",
        s2_usercmd_subscribe,
    );
    // Usercmd primitive Task 3: field read/write + buttons + subtick-clear natives (Task 4 wraps these
    // in the prelude's singleton Cmd accessor object).
    set_native(scope, global_obj, "__s2_usercmd_read", s2_usercmd_read);
    set_native(scope, global_obj, "__s2_usercmd_write", s2_usercmd_write);
    set_native(
        scope,
        global_obj,
        "__s2_usercmd_read_buttons",
        s2_usercmd_read_buttons,
    );
    set_native(
        scope,
        global_obj,
        "__s2_usercmd_write_buttons",
        s2_usercmd_write_buttons,
    );
    set_native(
        scope,
        global_obj,
        "__s2_usercmd_clear_subtick",
        s2_usercmd_clear_subtick,
    );
    set_native(scope, global_obj, "__s2_plugins_list", s2_plugins_list);
    set_native(scope, global_obj, "__s2_plugin_unload", s2_plugin_unload);
    set_native(scope, global_obj, "__s2_plugin_reload", s2_plugin_reload);
    set_native(scope, global_obj, "__s2_plugin_load", s2_plugin_load);
    set_native(scope, global_obj, "__s2_server_command", s2_server_command);
    set_native(
        scope,
        global_obj,
        "__s2_server_map_valid",
        s2_server_map_valid,
    );
    // reservedslots+basetriggers: server-info natives (max clients / map name / game time).
    set_native(
        scope,
        global_obj,
        "__s2_server_max_clients",
        s2_server_max_clients,
    );
    set_native(
        scope,
        global_obj,
        "__s2_server_map_name",
        s2_server_map_name,
    );
    set_native(
        scope,
        global_obj,
        "__s2_server_game_time",
        s2_server_game_time,
    );
    // Slice 6.2 Task 2: config-bridge natives for the admin module (file load/write).
    set_native(
        scope,
        global_obj,
        "__s2_config_read_raw",
        s2_config_read_raw,
    );
    set_native(
        scope,
        global_obj,
        "__s2_config_write_raw",
        s2_config_write_raw,
    );
    // Slice nominations Task 1: raw configs-dir file read/write for @s2script/config.
    set_native(
        scope,
        global_obj,
        "__s2_config_read_file",
        s2_config_read_file,
    );
    set_native(
        scope,
        global_obj,
        "__s2_config_write_file",
        s2_config_write_file,
    );
    // Slice DB Task 3: the `__s2_sqlite_*` natives (query/execute now actor-backed, off-thread) for `@s2script/db`.
    set_native(scope, global_obj, "__s2_sqlite_open", s2_sqlite_open);
    set_native(scope, global_obj, "__s2_sqlite_query", s2_sqlite_query);
    set_native(scope, global_obj, "__s2_sqlite_execute", s2_sqlite_execute);
    set_native(scope, global_obj, "__s2_sqlite_close", s2_sqlite_close);
    // Remote SQL driver Task 2: the `__s2_db_remote_*` natives (MySQL/Postgres over sqldb.rs).
    set_native(
        scope,
        global_obj,
        "__s2_db_remote_connect",
        s2_db_remote_connect,
    );
    set_native(
        scope,
        global_obj,
        "__s2_db_remote_query",
        s2_db_remote_query,
    );
    set_native(
        scope,
        global_obj,
        "__s2_db_remote_execute",
        s2_db_remote_execute,
    );
    set_native(
        scope,
        global_obj,
        "__s2_db_remote_close",
        s2_db_remote_close,
    );
    // Slice HTTP Task 2: async fetch over the process-global tokio+reqwest engine (core/src/http.rs).
    crate::http::install_natives(scope, global_obj);
    // WebSocket Task 2: client ws over the process-global tokio+tungstenite engine (core/src/ws.rs).
    set_native(scope, global_obj, "__s2_ws_connect", s2_ws_connect);
    crate::ws::install_natives(scope, global_obj);
    // Net Task 2: raw TCP/UDP client sockets over the process-global tokio engine (core/src/net.rs).
    set_native(
        scope,
        global_obj,
        "__s2_net_tcp_connect",
        s2_net_tcp_connect,
    );
    set_native(scope, global_obj, "__s2_net_udp_bind", s2_net_udp_bind);
    crate::net::install_natives(scope, global_obj);
    // TopMenu registry (adminmenu framework): owner-tracked categories/items + post-drain select dispatch.
    set_native(
        scope,
        global_obj,
        "__s2_topmenu_add_category",
        s2_topmenu_add_category,
    );
    set_native(
        scope,
        global_obj,
        "__s2_topmenu_add_tab",
        s2_topmenu_add_tab,
    );
    set_native(
        scope,
        global_obj,
        "__s2_topmenu_add_item",
        s2_topmenu_add_item,
    );
    set_native(
        scope,
        global_obj,
        "__s2_topmenu_snapshot",
        s2_topmenu_snapshot,
    );
    set_native(scope, global_obj, "__s2_topmenu_select", s2_topmenu_select);
    // Host-side HUD pool claims: the pooled panel trees are one shared entity, so who-owns-which-
    // slot is cross-plugin state — it cannot live in the per-context prelude (see crate::ui_pool).
    crate::ui_pool::install_natives(scope, global_obj);
    // Ray-trace slice: the sole native over the trace_shape engine op (engine-generic, no CS2 names).
    set_native(scope, global_obj, "__s2_trace", s2_trace);
    // Entity-creation lifecycle slice: createEntity + EntityRef.spawn/teleport/remove natives.
    set_native(
        scope,
        global_obj,
        "__s2_user_message_create",
        s2_user_message_create,
    );
    set_native(
        scope,
        global_obj,
        "__s2_user_message_set_int",
        s2_user_message_set_int,
    );
    set_native(
        scope,
        global_obj,
        "__s2_user_message_set_float",
        s2_user_message_set_float,
    );
    set_native(
        scope,
        global_obj,
        "__s2_user_message_set_string",
        s2_user_message_set_string,
    );
    set_native(
        scope,
        global_obj,
        "__s2_user_message_set_bool",
        s2_user_message_set_bool,
    );
    set_native(
        scope,
        global_obj,
        "__s2_user_message_send",
        s2_user_message_send,
    );
    set_native(
        scope,
        global_obj,
        "__s2_collision_activate",
        s2_collision_activate,
    );
    set_native(scope, global_obj, "__s2_sound_emit", s2_sound_emit);
    set_native(
        scope,
        global_obj,
        "__s2_sound_precache_add",
        s2_sound_precache_add,
    );
    // Item slice: the sub-object vcall native + the readHandleVector native (wrapped as an
    // EntityRef prototype method in the prelude, below). A5b retired give/remove-item to
    // gamedata/cs2 `calls` descriptors.
    // Entity-I/O slice: fire inputs (AddEntityIOEvent) + Entity.onOutput subscribe/unsubscribe
    // (FireOutputInternal detour dispatch — installed at shim Load, see dispatch_output).
    set_native(
        scope,
        global_obj,
        "__s2_output_subscribe",
        s2_output_subscribe,
    );
    set_native(scope, global_obj, "__s2_cvar_on_change", s2_cvar_on_change);
    set_native(
        scope,
        global_obj,
        "__s2_cvar_off_change",
        s2_cvar_off_change,
    );
    set_native(
        scope,
        global_obj,
        "__s2_output_unsubscribe",
        s2_output_unsubscribe,
    );
    // Entity lifecycle listeners slice: Entity.onCreate/onSpawn/onDelete subscribe/unsubscribe (the
    // IEntityListener is lazily installed shim-side on the first subscribe via entity_listener_install).
    // entity_name slice: EntityRef.name reads CEntityIdentity::m_name (sibling of entity_find_by_class's
    // m_designerName read on the same identity).
    // entity_target slice: EntityRef.target reads CBaseEntity::m_target via the schema-offset cache
    // (the field lives on the instance itself, not the identity — see s2_entity_target's doc comment).
    // E1 entity-liveness slice: identity-slot flags (books-gated) for pawn.isValid's staging check.
    // checktransmit slice: declarative per-client entity visibility rules (@s2script/transmit).
    set_native(scope, global_obj, "__s2_transmit_set", s2_transmit_set);
    set_native(scope, global_obj, "__s2_transmit_reset", s2_transmit_reset);
    set_native(
        scope,
        global_obj,
        "__s2_transmit_reset_all",
        s2_transmit_reset_all,
    );
    set_native(scope, global_obj, "__s2_transmit_stats", s2_transmit_stats);
    // Voice-hearability slice: declarative per-(receiver, sender) rules (@s2script/sdk/voice). The
    // shim evaluates them on the SetClientListening hot path; no JS runs per pair.
    set_native(
        scope,
        global_obj,
        "__s2_voice_audible_set",
        s2_voice_audible_set,
    );
    set_native(
        scope,
        global_obj,
        "__s2_voice_audible_clear",
        s2_voice_audible_clear,
    );
    set_native(
        scope,
        global_obj,
        "__s2_voice_reset_all",
        s2_voice_reset_all,
    );
    set_native(
        scope,
        global_obj,
        "__s2_voice_audible_stats",
        s2_voice_audible_stats,
    );
    // UserMessage-interception slice: UserMessages.onPre subscribe/unsubscribe + the block-scoped view
    // read natives (route through the usermsg_hook_* ops; the shim's PostEventAbstract hook installs
    // lazily on the first subscribe).
    crate::usermsg::install_natives(scope, global_obj);
    // Plugin-declared engine calls (`@s2script/sdk/unsafe`): ask-by-name only — there is deliberately
    // NO registration native (core registers descriptors itself from the packed gamedata.json).
    set_native(
        scope,
        global_obj,
        "__s2_engine_call_ready",
        s2_engine_call_ready,
    );
    set_native(
        scope,
        global_obj,
        "__s2_engine_call_receiverless",
        s2_engine_call_receiverless,
    );
    set_native(
        scope,
        global_obj,
        "__s2_engine_call_status",
        s2_engine_call_status,
    );
    set_native(
        scope,
        global_obj,
        "__s2_engine_call_invoke",
        s2_engine_call_invoke,
    );
    crate::shared_entity_switch::install(scope, global_obj);
    // The GAME-PACKAGE-scoped four (A5b): same natives, keyed on core's reserved owner id for the
    // registered game package instead of the calling context's plugin id. The game package's
    // prelude runs in the raw context scope and has no plugin identity of its own, so it cannot use
    // the four above; and an owner id is never taken from JS, so these cannot be aimed elsewhere.
    set_native(
        scope,
        global_obj,
        "__s2_game_call_ready",
        s2_game_call_ready,
    );
    set_native(
        scope,
        global_obj,
        "__s2_game_call_receiverless",
        s2_game_call_receiverless,
    );
    set_native(
        scope,
        global_obj,
        "__s2_game_call_status",
        s2_game_call_status,
    );
    set_native(
        scope,
        global_obj,
        "__s2_game_call_invoke",
        s2_game_call_invoke,
    );
    // Declarative inbound hooks: `__s2_hook_on` is the game-package subscribe (owner is the first
    // argument, remapped to the reserved owner id). `__s2_engine_hook_*` is the plugin path —
    // owner is the calling context, never an argument — so `Engine.hook` cannot name another plugin.
    // There is no registration native: core registers hook descriptors itself from the packed
    // gamedata, so JS can never declare a detour, only subscribe to a declared one.
    set_native(scope, global_obj, "__s2_hook_on", s2_hook_on);
    set_native(
        scope,
        global_obj,
        "__s2_engine_hook_ready",
        s2_engine_hook_ready,
    );
    set_native(
        scope,
        global_obj,
        "__s2_engine_hook_status",
        s2_engine_hook_status,
    );
    set_native(scope, global_obj, "__s2_engine_hook_on", s2_engine_hook_on);
    set_native(scope, global_obj, "__s2_hook_on_post", s2_hook_on_post);
    set_native(scope, global_obj, "__s2_hook_q_u16", s2_hook_q_u16);
    set_native(
        scope,
        global_obj,
        "__s2_hook_self_matches",
        s2_hook_self_matches,
    );
}

