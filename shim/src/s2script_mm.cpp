#include "s2script_mm.h"
#include "s2script_core.h"

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

static void s2_logger(int level, const char* msg) {
    META_CONPRINTF("[s2script] %s\n", msg);
}

bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();
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
