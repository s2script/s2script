#pragma once
#include <ISmmPlugin.h>

// ISource2Server is forward-declared here; full definition (eiface.h) is
// included only in s2script_mm.cpp where the SourceHook machinery lives.
class ISource2Server;

class S2ScriptPlugin : public ISmmPlugin {
public:
    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    bool Unload(char* error, size_t maxlen) override;

    // SourceHook handlers — installed lazily by s2_request_hook("OnGameFrame",1).
    // Pre-phase (false) dispatches phase 0; post-phase (true) dispatches phase 1.
    void Hook_GameFramePre(bool simulating, bool first, bool last);
    void Hook_GameFramePost(bool simulating, bool first, bool last);

    // Server interface pointer acquired in Load(); used by s2_request_hook.
    ISource2Server* m_server = nullptr;
    bool m_frameHookInstalled = false;

    // Plugin info
    const char* GetAuthor() override      { return "s2script"; }
    const char* GetName() override        { return "s2script"; }
    const char* GetDescription() override { return "TypeScript plugin runtime for Source 2"; }
    const char* GetURL() override         { return "https://s2script.com"; }
    const char* GetLicense() override     { return "TBD"; }
    const char* GetVersion() override     { return "0.0.0-slice1"; }
    const char* GetDate() override        { return __DATE__; }
    const char* GetLogTag() override      { return "S2SCRIPT"; }
};

extern S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_GLOBALVARS();
