#include "s2script_mm.h"
#include "s2script_core.h"
#include "gamedata.h"

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

static void s2_logger([[maybe_unused]] int level, const char* msg) {
    META_CONPRINTF("[s2script] %s\n", msg);
}

bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();

    // --- Interface acquisition (data-driven, degrade-never-crash) ---
    std::string gdError;
    // Path is relative to the game root (csgo/), where addons/ lives at runtime.
    auto versions = LoadInterfaceVersions("addons/s2script/gamedata/core.gamedata.jsonc", gdError);
    if (!gdError.empty()) {
        META_CONPRINTF("[s2script] WARN: %s — skipping interface acquisition\n", gdError.c_str());
    } else {
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
        tryGet("Source2Server",        ismm->GetServerFactory(false));
        tryGet("EngineCvar",           ismm->GetEngineFactory(false));
        tryGet("NetworkServerService", ismm->GetEngineFactory(false));
        // SchemaSystem comes from the schemasystem module factory, not engine/server.
        // Wire it by dlopen("schemasystem.so") and GetProcAddress("CreateInterface")
        // following the CounterStrikeSharp pattern — deferred until module loading
        // helpers are available.
        META_CONPRINTF("[s2script] NOTE: SchemaSystem acquisition deferred — schemasystem module factory not yet wired\n");
    }
    // --- end interface acquisition ---

    META_CONPRINTF("[s2script] Load(): initializing V8 core\n");

    if (s2script_core_init(&s2_logger) != 0) {
        META_CONPRINTF("[s2script] ERROR: V8 core init failed (plugin stays loaded for diagnosis)\n");
        return true; // degrade, do not fail the load (spec §7)
    }
    s2script_core_eval("console.log('hello from V8 in CS2')");
    return true;
}

bool S2ScriptPlugin::Unload(char* error, size_t maxlen) {
    META_CONPRINTF("[s2script] Unload(): shutting down V8 core\n");
    s2script_core_shutdown();
    return true;
}
