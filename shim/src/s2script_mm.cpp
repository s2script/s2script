#include "s2script_mm.h"

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();
    META_CONPRINTF("[s2script] Load(): boot handshake (no V8 yet)\n");
    return true;
}

bool S2ScriptPlugin::Unload(char* error, size_t maxlen) {
    META_CONPRINTF("[s2script] Unload(): clean teardown\n");
    return true;
}
