#ifndef S2SCRIPT_CORE_H
#define S2SCRIPT_CORE_H
#include <stdint.h>   /* uint64_t */
#ifdef __cplusplus
extern "C" {
#endif

typedef void (*s2_log_fn)(int level, const char* utf8_msg);
typedef void (*s2_hook_request_fn)(const char* descriptor, int enable); /* core -> shim: install(1)/remove(0) */

/* Engine-ops table: generated from core/engine-ops.jsonc (A7).
 * Field ORDER is the ABI. Do not edit the generated header. */
#include "s2script_engine_ops.generated.h"

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

/* S2EngineOps is a generated function-pointer ABI: sizeof alone cannot detect a same-sized
 * signature change. New shims MUST use the versioned entry and pass both constants. */
#define S2_ENGINE_OPS_ABI_VERSION UINT32_C(2)
#define S2_CORE_INIT_ABI_MISMATCH (-3)

/* Legacy compatibility entry. `ops == NULL` remains supported for tests/no-engine embedders.
 * A non-null unversioned table is always rejected with S2_CORE_INIT_ABI_MISMATCH. */
int  s2script_core_init(s2_log_fn logger, s2_hook_request_fn request_hook, const S2EngineOps* ops);
/* ops may be null -> all engine natives degrade. The core copies a matching table by value; the
 * caller's storage need not outlive the call. ABI version and exact struct size are checked before
 * ops is dereferenced. */
int  s2script_core_init_v2(s2_log_fn logger, s2_hook_request_fn request_hook,
                           const S2EngineOps* ops, uint32_t ops_abi_version,
                           uint32_t ops_struct_size);
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
/* Post-phase spectator mux for a returning inbound hook (CanAcquire). `skipped` is 1 when Pre
 * suppressed the original. Weak for the same reason as dispatch_hook. */
int s2script_core_dispatch_hook_post(int hookId, void* argView, int skipped) __attribute__((weak));
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
