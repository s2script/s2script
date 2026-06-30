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
        META_CONPRINTF("[s2script] NOTE: SchemaSystem acquisition deferred — schemasystem module factory not yet wired\n");
    }
    // --- end interface acquisition ---

    META_CONPRINTF("[s2script] Load(): initializing V8 core\n");

    // Pass both callbacks; the core calls s2_request_hook("OnGameFrame", 1)
    // to lazily install the SourceHook detour once a script subscribes.
    if (s2script_core_init(&s2_logger, &s2_request_hook) != 0) {
        META_CONPRINTF("[s2script] ERROR: V8 core init failed (plugin stays loaded for diagnosis)\n");
        return true; // degrade, do not fail the load (spec §7)
    }
    // Slice 1 live demo: subscribe two OnGameFrame handlers at different priorities.
    // Subscribing the first one drives request_hook("OnGameFrame", 1) -> the SourceHook
    // detour installs lazily; each frame then dispatches through the multiplexer, and
    // HIGH must log before LOW within a frame (priority-ordered composition).
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
