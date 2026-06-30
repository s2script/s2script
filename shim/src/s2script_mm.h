#pragma once
#include <ISmmPlugin.h>

class S2ScriptPlugin : public ISmmPlugin {
public:
    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    bool Unload(char* error, size_t maxlen) override;

    // Plugin info
    const char* GetAuthor() override      { return "s2script"; }
    const char* GetName() override        { return "s2script"; }
    const char* GetDescription() override { return "TypeScript plugin runtime for Source 2"; }
    const char* GetURL() override         { return "https://s2script.com"; }
    const char* GetLicense() override     { return "TBD"; }
    const char* GetVersion() override     { return "0.0.0-slice0"; }
    const char* GetDate() override        { return __DATE__; }
    const char* GetLogTag() override      { return "S2SCRIPT"; }
};

extern S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_GLOBALVARS();
