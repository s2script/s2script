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
// Forward-declared for the ClientDisconnect lifecycle hook (@s2script/clients). This header is parsed
// before eiface.h pulls the full definition, so an opaque enum decl with the SDK's fixed underlying type
// (`: int`, per network_connection.pb.h) is required; it is compatible with the later full definition.
enum ENetworkDisconnectionReason : int;
// Forward-declared for the StartupServer map-start hook (clientlist-fakeconvar-onmapstart slice); full
// definitions (iserver.h) live in s2script_mm.cpp. INetworkServerService / ISource2WorldSession are
// forward-declared classes in iserver.h too (iserver.h:41,43), so `class` here is compatible.
// GameSessionConfiguration_t is only ever forward-declared across the whole SDK (its real body is
// commented out), so it stays INCOMPLETE here — but the SH_DECL_HOOK3_void macro in the .cpp sizeof's
// the by-ref param type, and `sizeof` needs a COMPLETE type. s2script_mm.cpp therefore promotes it to a
// (definitionally empty, ABI-safe) complete type with `class GameSessionConfiguration_t {};` just before
// the SH_DECL; the forward decl here remains compatible with that later definition (forward-decl then
// define within the one TU is legal).
class INetworkServerService;
class GameSessionConfiguration_t;
class ISource2WorldSession;

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

    // Client lifecycle notify-hooks (@s2script/clients sub-project) — six post-hooks on the same
    // m_gameClients interface. Each forwards to s2script_core_dispatch_client_event and never alters flow.
    // (Ban enforcement no longer rejects at ClientConnect — sub-project 3 moved it to the JS onConnect
    // event [basebans], which shows the reason then kicks; the old ClientConnect reject hook was removed.)
    // `uint64` params are declared `unsigned long long` (== uint64 on Linux) because META_NO_HL2SDK keeps
    // HL2SDK basetypes out of this header.
    void Hook_OnClientConnected(CPlayerSlot slot, const char* name, unsigned long long xuid,
                                const char* netid, const char* addr, bool fake);
    void Hook_ClientPutInServer(CPlayerSlot slot, const char* name, int type, unsigned long long xuid);
    void Hook_ClientActive(CPlayerSlot slot, bool bLoadGame, const char* name, unsigned long long xuid);
    void Hook_ClientFullyConnect(CPlayerSlot slot);
    void Hook_ClientDisconnect(CPlayerSlot slot, ENetworkDisconnectionReason reason, const char* name,
                               unsigned long long xuid, const char* netid);
    void Hook_ClientSettingsChanged(CPlayerSlot slot);

    // Map-start hook (clientlist-fakeconvar-onmapstart slice) — POST hook on
    // INetworkServerService::StartupServer (the CSSharp OnMapStart mechanism). Reads the live map
    // name off the (typed) game server and forwards to s2script_core_dispatch_map_start.
    void Hook_StartupServer(const GameSessionConfiguration_t& config, ISource2WorldSession* session,
                            const char* unk);

    // (Sound slice precache: NO member hook — OnPrecacheResource is intercepted by a class-vtable slot
    // swap (s2vtable::GetVTableByName + s2detour-free WriteVtableSlot) whose handler + installer are
    // file-static free functions in s2script_mm.cpp. No live instance, no SourceHook needed; see the
    // precache block comment there for why the factory-walk / inline-detour / manual-hook options were
    // ruled out on the pinned binary.)

    // Server interface pointer acquired in Load(); used by s2_request_hook.
    ISource2Server* m_server = nullptr;
    ISource2GameClients* m_gameClients = nullptr;
    bool m_frameHookInstalled  = false;
    bool m_eventHookInstalled  = false;
    bool m_clientCmdHookInstalled = false;
    bool m_clientLifecycleHooksInstalled = false;  // @s2script/clients: the six notify lifecycle hooks
    bool m_startupServerHookInstalled = false;     // OnMapStart: the StartupServer POST hook
    // (Sound slice precache install state — s_precacheHookInstalled — is a file-static in the .cpp,
    // since the hook is a class-vtable swap driven by free functions, not a member SourceHook.)

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
