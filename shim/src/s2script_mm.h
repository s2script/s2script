#pragma once
#include <ISmmPlugin.h>
#include "khook_map.h"

// ISource2Server is forward-declared here; full definition (eiface.h) is
// included only in s2script_mm.cpp where the KHook machinery lives.
class ISource2Server;
// IGameEvent is forward-declared here; full definition (igameevents.h) is
// included only in s2script_mm.cpp where the KHook machinery lives.
class IGameEvent;
class IGameEventManager2;
// Forward-declared for the ClientCommand hook (Slice 6.11c); full definitions
// (eiface.h / convar.h / playerslot.h) live in s2script_mm.cpp.
class ISource2GameClients;
class CCommand;
class CPlayerSlot;
// Forward-declared for the DispatchConCommand listener hook (the AddCommandListener seam); full
// definitions (convar.h) live in s2script_mm.cpp. `ConCommandRef` is passed BY VALUE, so the
// declaration below is enough here only because the definition is in scope at the point of use.
class ICvar;
class ConCommandRef;
class CCommandContext;
class IVEngineServer2;
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
// Forward-declared for the CheckTransmit hook (checktransmit slice); full definitions
// (eiface.h / iservernetworkable.h / bitvec.h) live in s2script_mm.cpp. CBitVec's forward decl
// matches bitvec.h:414 (`template <int NUM_BITS> class CBitVec`); by-ref params in pure
// DECLARATIONS don't need the complete type.
class ISource2GameEntities;
class CCheckTransmitInfo;
struct Entity2Networkable_t;
template <int NUM_BITS> class CBitVec;
// Forward-declared for the UserMessage-interception PostEventAbstract hook (usermsg-hook slice); full
// definitions (engine/igameeventsystem.h -> networksystem/inetworkserializer.h, netmessage.h,
// tier1/convar.h, inetchannel.h) live in s2script_mm.cpp. Pure by-value/by-pointer DECLARATION params
// don't need complete types here. `unsigned long long` == the SDK's uint64 on Linux (the META_NO_HL2SDK
// convention used by the client-lifecycle hooks); NetChannelBufType_t's underlying type is int8
// (== signed char, platform.h:273), stated so this forward decl matches the later full definition.
struct CSplitScreenSlot;
class IGameEventSystem;
class INetworkMessageInternal;
class CNetMessage;
enum NetChannelBufType_t : signed char;

class S2ScriptPlugin : public ISmmPlugin {
public:
    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    // False with a named retry message while dispatch is active or checked-hook
    // retirement is pending. A later `meta unload` finishes cleanup once.
    bool Unload(char* error, size_t maxlen) override;

    // KHook handlers — installed lazily by s2_request_hook("OnGameFrame",1).
    // Pre-phase dispatches phase 0; post-phase dispatches phase 1. Every Virtual
    // callback takes the interface pointer first.
    KHook::Return<void> Hook_GameFramePre(ISource2Server* server, bool simulating, bool first, bool last);
    KHook::Return<void> Hook_GameFramePost(ISource2Server*, bool simulating, bool first, bool last);
    KHook::Return<bool> Hook_FireEventPre(IGameEventManager2*, IGameEvent* ev, bool bDontBroadcast);
    KHook::Return<void> Hook_ClientCommand(ISource2GameClients*, CPlayerSlot slot, const CCommand& args);
    KHook::Return<void> Hook_DispatchConCommand(ICvar*, ConCommandRef cmd, const CCommandContext& ctx,
                                                const CCommand& args);
    KHook::Return<void> Hook_OnClientConnected(ISource2GameClients*, CPlayerSlot slot, const char* name,
                                               unsigned long long xuid, const char* netid, const char* addr, bool fake);
    KHook::Return<void> Hook_ClientPutInServer(ISource2GameClients*, CPlayerSlot slot, const char* name,
                                               int type, unsigned long long xuid);
    KHook::Return<void> Hook_ClientActive(ISource2GameClients*, CPlayerSlot slot, bool bLoadGame,
                                          const char* name, unsigned long long xuid);
    KHook::Return<void> Hook_ClientFullyConnect(ISource2GameClients*, CPlayerSlot slot);
    KHook::Return<void> Hook_ClientDisconnect(ISource2GameClients*, CPlayerSlot slot,
                                              ENetworkDisconnectionReason reason, const char* name,
                                              unsigned long long xuid, const char* netid);
    KHook::Return<void> Hook_ClientSettingsChanged(ISource2GameClients*, CPlayerSlot slot);
    KHook::Return<void> Hook_ClientVoice(ISource2GameClients*, CPlayerSlot slot);
    KHook::Return<bool> Hook_SetClientListening(IVEngineServer2*, CPlayerSlot receiver,
                                                CPlayerSlot sender, bool bListen);
    KHook::Return<void> Hook_StartupServer(INetworkServerService*, const GameSessionConfiguration_t& config,
                                           ISource2WorldSession* session, const char* unk);
    KHook::Return<void> Hook_CheckTransmit(ISource2GameEntities*, CCheckTransmitInfo** ppInfoList, int nInfoCount,
                                           CBitVec<16384>& unionTransmitEdicts, CBitVec<16384>& unionTransmitEdicts2,
                                           const Entity2Networkable_t** pNetworkables,
                                           const unsigned short* pEntityIndices, int nEntityIndices);
    KHook::Return<void> Hook_PostEvent(IGameEventSystem*, CSplitScreenSlot nSlot, bool bLocalOnly, int nClientCount,
                                       const unsigned long long* clients, INetworkMessageInternal* pEvent,
                                       const CNetMessage* pData, unsigned long nSize, NetChannelBufType_t bufType);

    // (Sound slice precache: NO member hook — OnPrecacheResource is intercepted by a class-vtable slot
    // swap (s2vtable::GetVTableByName + s2detour-free WriteVtableSlot) whose handler + installer are
    // file-static free functions in s2script_mm.cpp. No live instance, no SourceHook needed; see the
    // precache block comment there for why the factory-walk / inline-detour / manual-hook options were
    // ruled out on the pinned binary.)

    // Server interface pointer acquired in Load(); used by s2_request_hook.
    ISource2Server* m_server = nullptr;
    ISource2GameClients* m_gameClients = nullptr;
    ISource2GameEntities* m_gameEntities = nullptr;
    bool m_checkTransmitHookInstalled = false;     // checktransmit: the CheckTransmit POST hook
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
    const char* GetVersion() override     { return S2SCRIPT_VERSION; }
    const char* GetDate() override        { return __DATE__; }
    const char* GetLogTag() override      { return "S2SCRIPT"; }
};

extern S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_GLOBALVARS();
