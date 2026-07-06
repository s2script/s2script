#pragma once
#include <ISmmPlugin.h>

// ISource2Server is forward-declared here; full definition (eiface.h) is
// included only in s2script_mm.cpp where the SourceHook machinery lives.
class ISource2Server;
// IGameEvent is forward-declared here; full definition (igameevents.h) is
// included only in s2script_mm.cpp where the SourceHook machinery lives.
class IGameEvent;
// Forward-declared for the ClientCommand hook (Slice 6.11c); full definitions
// (eiface.h / convar.h / playerslot.h) live in s2script_mm.cpp.
class ISource2GameClients;
class CCommand;
class CPlayerSlot;
// Forward-declared for the ClientConnect hook (Slice 6.18); the reject-reason buffer is left
// untouched (writing it is the tier1 dlopen-cascade risk), so a pointer forward-decl suffices.
class CBufferString;

class S2ScriptPlugin : public ISmmPlugin {
public:
    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    bool Unload(char* error, size_t maxlen) override;

    // SourceHook handlers — installed lazily by s2_request_hook("OnGameFrame",1).
    // Pre-phase (false) dispatches phase 0; post-phase (true) dispatches phase 1.
    void Hook_GameFramePre(bool simulating, bool first, bool last);
    void Hook_GameFramePost(bool simulating, bool first, bool last);

    // FireEvent Pre hook (Slice 5D.3) — installed lazily by s2_request_hook("GameEvent",1).
    bool Hook_FireEventPre(IGameEvent* ev, [[maybe_unused]] bool bDontBroadcast);

    // ClientCommand hook (Slice 6.11c) — the engine callback when a client types a command at the console.
    // This is how CS2 frameworks (CSSharp/ModSharp) implement player CONSOLE commands: a clean
    // (slot, CCommand) — no low-level detour. Installed in Load() once ISource2GameClients is acquired.
    void Hook_ClientCommand(CPlayerSlot slot, const CCommand& args);

    // ClientConnect hook (Slice 6.18) — the engine callback when a client attempts to connect (NOT called
    // for bots). Returning false rejects the connection (eiface.h:569-571). Sibling of Hook_ClientCommand
    // on the same m_gameClients interface; rejects a banned SteamID64 (via s2script_core_ban_check).
    // `unsigned long long` (not the Valve `uint64` typedef) because META_NO_HL2SDK keeps HL2SDK basetypes
    // out of this header; on Linux `uint64` IS `unsigned long long` (platform.h), so the .cpp definition's
    // `uint64` matches this declaration exactly and SH_MEMBER binds the hook.
    bool Hook_ClientConnect(CPlayerSlot slot, const char* name, unsigned long long xuid, const char* netid,
                            bool unk1, CBufferString* rejectReason);

    // Server interface pointer acquired in Load(); used by s2_request_hook.
    ISource2Server* m_server = nullptr;
    ISource2GameClients* m_gameClients = nullptr;
    bool m_frameHookInstalled  = false;
    bool m_eventHookInstalled  = false;
    bool m_clientCmdHookInstalled = false;
    bool m_clientConnectHookInstalled = false;

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
