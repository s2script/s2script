// sdk_stubs/network_connection.pb.h (our stub, found first via CMake include path) must
// be resolved before eiface.h tries to include it. CMakeLists.txt lists sdk_stubs
// ahead of ${HL2SDK}/public, so the search succeeds without running protoc.
#include "s2script_mm.h"
#include "s2script_core.h"
#include "gamedata.h"

// Pull in ISource2Server (and the typedef IServerGameDLL = ISource2Server)
// from the HL2SDK.  The stub sdk_stubs/network_connection.pb.h satisfies the
// one missing generated include that eiface.h unconditionally pulls in.
#include <eiface.h>

// SchemaSystem: ISchemaSystem + the type-scope / class-info / field-data layout used by the
// schema-offset engine-op (recon Q1/Q2; include paths mirror shim/CMakeLists.txt).
#include <schemasystem/schemasystem.h>

// Entity system: CGameEntitySystem, CEntitySystem, CConcreteEntityList, CEntityIdentity,
// CEntityHandle, CEntityIndex, MAX_ENTITIES_IN_LIST, MAX_ENTITY_LISTS (recon Q3/Q4).
// Requires sdk_stubs/entitydatainstantiator.h (see shim/src/sdk_stubs/ stub — recon gap).
#include <entity2/entitysystem.h>
// CEntityInstance, NetworkStateChangedData (recon Q6).
#include <entity2/entityinstance.h>

// ConCommand, FnCommandCallback_t, ConCommandCallbackInfo_t, CCommandContext, CCommand (recon Q7).
#include <convar.h>

#include <dlfcn.h>    // dladdr
#include <libgen.h>   // dirname
#include <cstring>
#include <cstdio>
#include <string>

// SourceHook hook declaration: 3 void-return parameters (bool, bool, bool).
// ISource2Server is confirmed at eiface.h:384; GameFrame at eiface.h:407.
// IServerGameDLL (used in the s2_sample_mm reference) is a typedef to the same class.
SH_DECL_HOOK3_void(ISource2Server, GameFrame, SH_NOATTRIB, 0, bool, bool, bool);

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

// ---------------------------------------------------------------------------
// Logging callback (Rust core -> Metamod console)
// ---------------------------------------------------------------------------
static void s2_logger([[maybe_unused]] int level, const char* msg) {
    META_CONPRINTF("[s2script] %s\n", msg);
}

// ---------------------------------------------------------------------------
// SchemaSystem — acquired in Load() (recon Q2), queried by the schema-offset
// engine-op below.  File-scope so the C-ABI callback can reach it; null when
// acquisition failed (schema natives then degrade to a miss, never crash).
// ---------------------------------------------------------------------------
static ISchemaSystem* s_pSchemaSystem = nullptr;

// ---------------------------------------------------------------------------
// Entity system — IGameResourceService* and the gamedata byte-offset are cached
// at Load().  The CGameEntitySystem* itself is NOT cached at Load (the map and
// entity-system don't exist yet at that point); instead GetEntitySystem() reads
// it fresh on every entity-native call so it becomes valid once the map is live.
// ---------------------------------------------------------------------------
static void* s_pGameResourceService   = nullptr;
static int   s_gameEntitySystemOffset = -1;

/// Read CGameEntitySystem* fresh from the IGameResourceService* on each call.
/// Returns nullptr when the service pointer or offset is not yet available,
/// or when the field hasn't been written yet (e.g. before the first map load).
static CGameEntitySystem* GetEntitySystem() {
    if (!s_pGameResourceService || s_gameEntitySystemOffset < 0) return nullptr;
    return *reinterpret_cast<CGameEntitySystem**>(
        reinterpret_cast<uintptr_t>(s_pGameResourceService)
        + static_cast<size_t>(s_gameEntitySystemOffset));
}

// ---------------------------------------------------------------------------
// Engine-op: resolve a schema field's flattened byte offset within a class via
// the live SchemaSystem (recon Q1).  C-ABI, called by the Rust core through the
// S2EngineOps table; `cls`/`field` are opaque strings the JS @s2script/cs2 layer
// supplies.  Returns -1 (degrade-never-crash) when the SchemaSystem is missing or
// the scope / class / field can't be resolved.
// ---------------------------------------------------------------------------
static int s2_schema_offset(const char* cls, const char* field) {
    if (!s_pSchemaSystem || !cls || !field) return -1;

    // "libserver.so" is the CS2 Linux server module string (recon [LC]) — a module *filename*,
    // not a CS2 schema identifier; hardcoded here.
    // TODO: gamedata key if it ever varies across games/platforms (recon Q1 [LC]).
    CSchemaSystemTypeScope* scope = s_pSchemaSystem->FindTypeScopeForModule("libserver.so");
    if (!scope) scope = s_pSchemaSystem->GlobalTypeScope();  // fallback scope (recon Q1)
    if (!scope) return -1;

    CSchemaClassInfo* info = scope->FindRawClassBinding(cls);  // direct pointer, no handle unwrap
    if (!info) return -1;

    for (int i = 0; i < info->m_nFieldCount; ++i) {
        const SchemaClassFieldData_t& f = info->m_pFields[i];
        if (f.m_pszName && strcmp(f.m_pszName, field) == 0) {
            return f.m_nSingleInheritanceOffset;  // THE offset getter (recon Q1)
        }
    }
    return -1;  // field not found on the class
}

// ---------------------------------------------------------------------------
// Engine-op: resolve entity by index → CEntityInstance* (opaque void*, or null).
// Uses the signature-free manual chunk walk (recon Q3) — no gamedata signature needed
// beyond the CGameEntitySystem* anchor already loaded from the interface offset.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static void* s2_ent_by_index(int idx) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;
    if (idx < 0 || idx >= MAX_TOTAL_ENTITIES) return nullptr;

    int chunk = idx / MAX_ENTITIES_IN_LIST;
    int slot  = idx % MAX_ENTITIES_IN_LIST;

    // Guard: the chunk pointer may be null for unallocated (sparse) chunks.
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
    if (!chunk_base) return nullptr;

    CEntityIdentity* id = &chunk_base[slot];
    // EF_IS_INVALID_EHANDLE: identity slot is free/unallocated (recon Q3 [HC]).
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;

    return id->m_pInstance;  // may still be null (entity removed in progress); caller checks
}

// ---------------------------------------------------------------------------
// Engine-op: resolve a packed entity handle (u32) → CEntityInstance* or null.
// Signature-free chunk walk (recon Q4): mirrors s2_ent_by_index but adds serial
// validation via CEntityIdentity::GetRefEHandle() (inline, entityidentity.h:74).
// Does NOT call CEntitySystem::GetEntityIdentity(CEntityHandle const&) — that
// non-inline method is not exported by any CS2 module (confirmed via nm; dlopen
// blocker).  All helpers used here are inline or field accesses.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static void* s2_deref_handle(unsigned int handle) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;

    CEntityHandle h(static_cast<uint32>(handle));
    // GetEntryIndex() is inline (entityhandle.h:106); returns -1 for INVALID_EHANDLE_INDEX.
    int idx = h.GetEntryIndex();
    if (idx < 0 || idx >= MAX_TOTAL_ENTITIES) return nullptr;

    int chunk = idx / MAX_ENTITIES_IN_LIST;
    int slot  = idx % MAX_ENTITIES_IN_LIST;

    // Guard: the chunk pointer may be null for unallocated (sparse) chunks.
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
    if (!chunk_base) return nullptr;

    CEntityIdentity* id = &chunk_base[slot];
    // EF_IS_INVALID_EHANDLE: identity slot is free/unallocated (recon Q3 [HC]).
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;
    // Serial validation: GetRefEHandle() is inline (entityidentity.h:74); it returns
    // the identity's stored handle, adjusted for EF_IS_INVALID_EHANDLE so a free slot
    // never matches a live handle.  If index+serial differ from h → stale → null.
    if (!(id->GetRefEHandle() == h)) return nullptr;

    return id->m_pInstance;  // may still be null (entity removed in progress); caller checks
}

// ---------------------------------------------------------------------------
// Engine-op: mark an entity field dirty for network transmission (recon Q6).
// Calls CEntityInstance::NetworkStateChanged(NetworkStateChangedData(offset))
// via the vtable — no export needed; null-guards the entity pointer.
// C-ABI, called by the Rust core through the S2EngineOps table.
// ---------------------------------------------------------------------------
static void s2_ent_state_changed(void* ent, int offset) {
    if (!ent) return;
    // SAFETY: the Rust caller holds a block-scoped entity pointer obtained from
    // s2_ent_by_index or s2_deref_handle and must not cross an await boundary.
    static_cast<CEntityInstance*>(ent)->NetworkStateChanged(
        NetworkStateChangedData(static_cast<uint32>(offset)));
}

// ---------------------------------------------------------------------------
// ConCommand support (recon Q7).
//
// s_concommands: persistent storage for heap-allocated ConCommand objects.  The
// ConCommand constructor self-registers into the cvar system; the destructor calls
// Destroy() to unregister.  Objects live for the plugin lifetime.
// TODO(teardown): iterate s_concommands in Unload() and delete each one (ledger item).
// ---------------------------------------------------------------------------
[[maybe_unused]] static std::vector<ConCommand*> s_concommands;

// ONE shared trampoline for every registered ConCommand.  Source 2 puts the command
// name at Arg(0); ArgS() is everything after it.  Reads the name, slot, and args, then
// calls back into the Rust core via C-ABI so the registered JS function is invoked.
static void s2_concommand_trampoline(const CCommandContext& ctx, const CCommand& cmd) {
    const char* name = cmd.Arg(0);   // command name is always arg 0 in Source 2
    int slot         = ctx.GetPlayerSlot().Get();  // -1 for server-console invocations
    const char* args = cmd.ArgS();   // everything after the command name
    s2script_core_dispatch_concommand(name, slot, args ? args : "");
}

// Engine-op: register a ConCommand with the shared trampoline.
// Called by the Rust core's __s2_concommand native (through the S2EngineOps table).
// C-ABI; degrade-never-crash if name is null.
//
// NEUTRALIZED: ConCommand::Create is NOT exported by any CS2 module (confirmed via nm;
// it was a dlopen blocker).  This function logs a WARN and returns without registering.
// The trampoline, the s_concommands vector, and this wiring in S2EngineOps are kept
// intact so the full ConCommand machinery assembles cleanly for the Slice-5 command
// framework, which will route through an exported/vtable path.
// TODO(slice-5): replace this body with the Slice-5 command-registration path once the
//               correct exported or vtable-routed ConCommand::Create equivalent is identified.
static void s2_concommand_register(const char* name) {
    if (!name) return;
    META_CONPRINTF("[s2script] WARN: ConCommand registration unavailable on this build "
                   "(ConCommand::Create not exported by CS2); command '%s' not registered "
                   "— console-command support arrives with the Slice-5 command framework\n", name);
}

// ---------------------------------------------------------------------------
// GamedataPath: resolve the gamedata file relative to the plugin .so via
// dladdr so the path works regardless of the server's working directory.
// Expected layout: addons/s2script/bin/linuxsteamrt64/s2script.so
//   dirname ×1 → .../bin/linuxsteamrt64  → bin
//   dirname ×2 → .../bin                 → s2script addon dir
//   dirname ×3 → .../s2script            → addons/s2script
//   + /gamedata/core.gamedata.jsonc
// ---------------------------------------------------------------------------
static std::string GamedataPath() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&GamedataPath), &info) && info.dli_fname) {
        char buf[4096];
        // dirname mutates the buffer in-place; copy each time.
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/gamedata/core.gamedata.jsonc";
    }
    // Fallback: relative to the server's cwd (mirrors the Slice-0 behaviour).
    return "addons/s2script/gamedata/core.gamedata.jsonc";
}

// ---------------------------------------------------------------------------
// Cs2JsPath: resolve pawn.js relative to the plugin .so via dladdr (mirrors
// GamedataPath).  Expected layout (three dirname steps from the .so):
//   addons/s2script/bin/linuxsteamrt64/s2script.so
//     dirname ×1 → bin/linuxsteamrt64
//     dirname ×2 → bin
//     dirname ×3 → s2script addon root
//   + /js/pawn.js
// ---------------------------------------------------------------------------
static std::string Cs2JsPath() {
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&Cs2JsPath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf);             // linuxsteamrt64
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // bin
        snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);                         // s2script addon root
        return dir + "/js/pawn.js";
    }
    // Fallback: relative to the server's cwd (mirrors the GamedataPath fallback).
    return "addons/s2script/js/pawn.js";
}

// ---------------------------------------------------------------------------
// Hook-request callback: invoked by the Rust core to install/remove the
// SourceHook detour.  Called while the core holds an internal borrow —
// MUST NOT call back into the core (no eval/dispatch/shutdown).
// ---------------------------------------------------------------------------
static void s2_request_hook(const char* descriptor, int enable) {
    if (strcmp(descriptor, "OnGameFrame") != 0) return;

    if (enable && !g_S2ScriptPlugin.m_frameHookInstalled && g_S2ScriptPlugin.m_server) {
        SH_ADD_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                    SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePre),  false);
        SH_ADD_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                    SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePost), true);
        g_S2ScriptPlugin.m_frameHookInstalled = true;
    } else if (!enable && g_S2ScriptPlugin.m_frameHookInstalled) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                       SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePre),  false);
        SH_REMOVE_HOOK(ISource2Server, GameFrame, g_S2ScriptPlugin.m_server,
                       SH_MEMBER(&g_S2ScriptPlugin, &S2ScriptPlugin::Hook_GameFramePost), true);
        g_S2ScriptPlugin.m_frameHookInstalled = false;
    }
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------
bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();  // sets g_SHPtr = ismm->GetSHPtr() — required by SH_ADD_HOOK

    // --- Interface acquisition (data-driven, degrade-never-crash) ---
    std::string gdError;
    auto versions = LoadInterfaceVersions(GamedataPath(), gdError);
    if (!gdError.empty()) {
        META_CONPRINTF("[s2script] WARN: %s — skipping interface acquisition\n", gdError.c_str());
    } else {
        CreateInterfaceFn serverFactory = ismm->GetServerFactory(false);
        CreateInterfaceFn engineFactory = ismm->GetEngineFactory(false);

        // Acquire and store ISource2Server* — needed for the SourceHook detour.
        {
            auto it = versions.find("Source2Server");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : INTERFACEVERSION_SERVERGAMEDLL;
            int ret = 0;
            m_server = serverFactory
                ? reinterpret_cast<ISource2Server*>(serverFactory(verStr, &ret))
                : nullptr;
            if (m_server && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: Source2Server (%s)\n", verStr);
            } else {
                META_CONPRINTF("[s2script] WARN: interface MISSING: Source2Server (%s)\n", verStr);
            }
        }

        // Log other interfaces (not stored — acquired as needed in later slices).
        auto tryGet = [&](const char* key, CreateInterfaceFn factory) {
            auto it = versions.find(key);
            if (it == versions.end()) {
                META_CONPRINTF("[s2script] WARN: no version string for %s in gamedata\n", key);
                return;
            }
            int ret = 0;
            void* iface = factory ? factory(it->second.c_str(), &ret) : nullptr;
            if (iface && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: %s (%s)\n", key, it->second.c_str());
            } else {
                META_CONPRINTF("[s2script] WARN: interface MISSING: %s (%s)\n", key, it->second.c_str());
            }
        };
        tryGet("EngineCvar",           engineFactory);
        tryGet("NetworkServerService", engineFactory);

        // Acquire and store ISchemaSystem* — backs the schema-offset engine-op (recon Q2).
        // Reuse the engine-factory path (as for EngineCvar/NetworkServerService); the community
        // CS2 pattern resolves SchemaSystem_001 through the engine factory even though it lives in
        // libschemasystem.so.  Degrade-never-crash: leave the pointer null on any failure.
        {
            auto it = versions.find("SchemaSystem");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : SCHEMASYSTEM_INTERFACE_VERSION;
            int ret = 0;
            s_pSchemaSystem = engineFactory
                ? reinterpret_cast<ISchemaSystem*>(engineFactory(verStr, &ret))
                : nullptr;
            if (s_pSchemaSystem && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: SchemaSystem (%s)\n", verStr);
            } else {
                s_pSchemaSystem = nullptr;  // do not keep a partially-resolved pointer
                META_CONPRINTF("[s2script] WARN: interface MISSING: SchemaSystem (%s) — schema natives degrade\n", verStr);
                // TODO(T7): if the engine factory can't resolve SchemaSystem_001 live, fall back to
                // dlopen/dlsym of libschemasystem.so's own CreateInterface (recon Q2 fallback).
            }
        }

        // Acquire IGameResourceService* and derive CGameEntitySystem* at a gamedata-provided
        // offset (recon Q3).  The offset is DATA, never hardcoded here; a wrong value degrades
        // (null entity system → entity natives return null), never crashes.
        {
            auto it = versions.find("GameResourceService");
            const char* verStr = (it != versions.end()) ? it->second.c_str()
                                                        : "GameResourceServiceServerV001";
            int ret = 0;
            void* pGameResSvc = engineFactory
                ? engineFactory(verStr, &ret)
                : nullptr;

            if (pGameResSvc && ret == 0) {
                // Read entity-system offset from gamedata (layout-is-data, never hardcoded).
                std::string offsetError;
                auto offsets = LoadOffsets(GamedataPath(), "linuxsteamrt64", offsetError);
                if (!offsetError.empty()) {
                    META_CONPRINTF("[s2script] WARN: %s — entity-system offset unavailable\n",
                                   offsetError.c_str());
                }
                auto oit = offsets.find("GameEntitySystem");
                if (oit != offsets.end() && oit->second >= 0) {
                    int entSysOffset = oit->second;
                    // Cache the service pointer and offset; do NOT read CGameEntitySystem* here.
                    // The entity-system field is null at Load (the map doesn't exist yet); we read
                    // it fresh on each entity-native call via GetEntitySystem() so it becomes valid
                    // once a map loads.  A null at Load is expected and not a WARN.
                    s_pGameResourceService   = pGameResSvc;
                    s_gameEntitySystemOffset = entSysOffset;
                    META_CONPRINTF("[s2script] interface OK: GameResourceService (%s, entity-system offset=%d cached; resolved per-call)\n",
                                   verStr, entSysOffset);
                } else {
                    META_CONPRINTF("[s2script] WARN: GameEntitySystem offset not in gamedata — entity natives degrade\n");
                }
            } else {
                META_CONPRINTF("[s2script] WARN: GameResourceService interface MISSING (%s) — entity natives degrade\n",
                               verStr);
            }
        }
    }
    // --- end interface acquisition ---

    META_CONPRINTF("[s2script] Load(): initializing V8 core\n");

    // Assemble the engine-ops table for the core.  Task 3 wired schema_offset; Task 4 adds the
    // three entity ops below.  Task 5 fills concommand_register.  A null field (or a null
    // backing pointer inside the helper) degrades the matching native to a miss.
    // The core copies this struct by value at init, so the stack-local is safe to let die when
    // Load() returns.
    S2EngineOps ops = {};
    ops.schema_offset      = &s2_schema_offset;
    ops.ent_by_index       = &s2_ent_by_index;
    ops.deref_handle       = &s2_deref_handle;
    ops.ent_state_changed  = &s2_ent_state_changed;
    ops.concommand_register = &s2_concommand_register;

    // Pass both callbacks + the engine-ops table; the core calls s2_request_hook("OnGameFrame", 1)
    // to lazily install the SourceHook detour once a script subscribes.
    if (s2script_core_init(&s2_logger, &s2_request_hook, &ops) != 0) {
        META_CONPRINTF("[s2script] ERROR: V8 core init failed (plugin stays loaded for diagnosis)\n");
        return true; // degrade, do not fail the load (spec §7)
    }

    // Load the @s2script/cs2 JS package (pawn.js) — CS2 names live in the file, never in core.
    // Degrade-never-crash: a missing or unreadable pawn.js logs a WARN and continues.
    s2script_core_load_cs2(Cs2JsPath().c_str());
    // Slice 1 live demo: subscribe two OnGameFrame handlers at different priorities.
    // Subscribing the first one drives request_hook("OnGameFrame", 1) -> the SourceHook
    // detour installs lazily; each frame then dispatches through the multiplexer, and
    // HIGH must log before LOW within a frame (priority-ordered composition).
    // Slice 2 appends an async demo: `await Delay(1000)` must not block the tick (the frame
    // counter keeps advancing), and an off-thread `threadSleep` must resume on the main thread.
    // (Baked into Load like Slice 0's hello; removed when real plugin loading lands in Slice 4.)
    s2script_core_eval(R"JS(
        console.log('hello from V8 in CS2');
        var __n = 0;
        onGameFrame(function (f) {
            if (__n % 256 === 0) console.log('[demo] HIGH tick=' + __n + ' firstTick=' + f.firstTick);
        }, { priority: 'high' });
        onGameFrame(function (f) {
            if (__n % 256 === 0) console.log('[demo] low');
            __n++;
        }, { priority: 'low' });
        console.log('[demo] subscribed 2 OnGameFrame handlers; HIGH should log before low each frame');

        // Slice 2 async demo: a monitor-priority handler fires once per engine frame (Pre phase, the
        // default) and counts frames. It ARMS the demo only after the server is genuinely live-ticking
        // — reaching 128 frames is impossible during the boot window (which produces ~0 frames/sec), so
        // this cleanly excludes boot. Once armed, `await Delay(1000)` must NOT block the tick: the frame
        // counter advances by ~tickrate during the await. Then an off-thread threadSleep resumes on main.
        var __frames = 0;
        var __armed = false;
        onGameFrame(function (f) {
            __frames++;
            if (!__armed && __frames >= 128) {
                __armed = true;
                var f0 = __frames;
                (async function () {
                    console.log('[async] before Delay(1000) at frame ' + f0);
                    await Delay(1000);
                    console.log('[async] after Delay(1000); frames elapsed ~' + (__frames - f0) + ' (tick was NOT blocked)');
                    await threadSleep(50);
                    console.log('[async] after threadSleep(50) - resumed on the main thread');
                })();
            }
        }, { priority: 'monitor' });

        // Slice 3 demo (auto readback): once live-ticking, scan slots for the first player pawn
        // (a bot added post-boot via bot_add) and prove `pawn.health` get/set + the folded-in
        // network state-change by reading the value straight back. Retries each frame until a pawn
        // exists, then stops. cs2.* comes from @s2script/cs2 (games/cs2/js/pawn.js), loaded above.
        var __s3done = false;
        onGameFrame(function () {
            if (__s3done || __frames < 150) return;   // wait until the server is live-ticking
            for (var slot = 0; slot < 64; slot++) {
                var p = cs2.pawnForSlot(slot);
                if (!p) continue;
                var got = p.health;
                console.log('[cs2] slot=' + slot + ' HEALTH_OFFSET=' + cs2.HEALTH_OFFSET + ' health get=' + got);
                p.health = 1234;                       // write + NetworkStateChanged (in the setter)
                console.log('[cs2] slot=' + slot + ' health set=1234 readback=' + p.health);
                __s3done = true;
                break;
            }
        }, { priority: 'monitor' });

        // Slice 3 manual HUD: `s2_sethp <value>` sets the CALLING client's pawn health (connect a
        // client, run it in console, watch the HUD change — proves the state-change networks).
        __s2_concommand('s2_sethp', function (slot, args) {
            var p = cs2.pawnForSlot(slot);
            if (!p) { console.log('[cs2] s2_sethp: no pawn for slot ' + slot); return; }
            var v = parseInt(args) || 100;
            p.health = v;
            console.log('[cs2] s2_sethp: slot ' + slot + ' -> health ' + v);
        });
    )JS");
    return true;
}

// ---------------------------------------------------------------------------
// Unload
// ---------------------------------------------------------------------------
bool S2ScriptPlugin::Unload(char* error, size_t maxlen) {
    META_CONPRINTF("[s2script] Unload(): shutting down V8 core\n");

    // Remove hooks before shutdown so no in-flight dispatch can reach a
    // freed core.  SH_REMOVE_HOOK is a no-op if the hook was never added.
    if (m_frameHookInstalled && m_server) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFramePre),  false);
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFramePost), true);
        m_frameHookInstalled = false;
    }

    s2script_core_shutdown();
    return true;
}

// ---------------------------------------------------------------------------
// SourceHook hook handlers
// ---------------------------------------------------------------------------
void S2ScriptPlugin::Hook_GameFramePre(bool simulating, bool first, bool last) {
    s2script_core_dispatch_game_frame(0, static_cast<int>(simulating),
                                      static_cast<int>(first), static_cast<int>(last));
    RETURN_META(MRES_IGNORED);
}

void S2ScriptPlugin::Hook_GameFramePost(bool simulating, bool first, bool last) {
    s2script_core_dispatch_game_frame(1, static_cast<int>(simulating),
                                      static_cast<int>(first), static_cast<int>(last));
    RETURN_META(MRES_IGNORED);
}
