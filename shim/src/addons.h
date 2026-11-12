// Native addon mounting (@s2script/sdk/addons): workshop addons mounted on the server and pushed to
// every client, without a separate Metamod plugin.
//
// Engine-generic: every Source 2 server mounts addons into the "GAME" search path, lists them in its
// host-state request, and tells each client which ones to load in S2C_CONNECTION and
// CNETMsg_SignonState. Nothing here names a game; which workshop item to mount is the caller's data.
//
// Three things make a workshop addon reach players:
//   1. Server mount. The addon's VPK goes into the server's "GAME" search path (IFileSystem).
//   2. Host-state list. Every map load (SetPendingHostStateRequest) carries the extra addons after
//      the map's own workshop addon, so the server loads with them and lists them to clients.
//   3. Client handshake. A client downloads ONE addon per connection. S2C_CONNECTION
//      (ReplyConnection) names the first addon the client still lacks. When the client comes back
//      inside the timeout, that addon counts as downloaded, and the next CNETMsg_SignonState
//      (CServerSideClient::SendNetMessage) is turned into a CHANGELEVEL that names the next one.
//      That repeats until the client has all of them.
//
// Every engine touchpoint is optional. A missing signature, offset or interface turns off the part
// that needs it, with one named log line, and leaves the rest working.
#ifndef S2SCRIPT_ADDONS_H
#define S2SCRIPT_ADDONS_H

#include <cstdint>
#include <string>

#include "engine_resolver.h"
#include "khook_binding.h"

class IFileSystem;
class INetworkMessageInternal;

// The engine facts this module needs, gathered by s2script_mm.cpp from gamedata at boot. A null
// pointer or a negative offset turns off the part that needs it.
struct S2AddonsEngine {
    IFileSystem* fs = nullptr;                       // VFileSystem017 (slot-pinned by the caller)
    void* hostStateRequestFn = nullptr;              // CHostStateMgr::SetPendingHostStateRequest
    void* replyConnectionFn = nullptr;               // CNetworkGameServerBase::ReplyConnection
    s2resolve::VirtualSlotResolution sendNetMessage; // CServerSideClient::SendNetMessage
    INetworkMessageInternal* signonInfo = nullptr;   // the CNETMsg_SignonState record
    int reqLevelNameOffset = -1;                     // CHostStateRequest::m_LevelName (CUtlString)
    int reqAddonsOffset = -1;                        // CHostStateRequest::m_Addons (CUtlString)
    int reqKeyValuesOffset = -1;                     // CHostStateRequest::m_pKV (KeyValues*)
    int serverAddonNameOffset = -1;                  // CNetworkGameServerBase::m_szAddonName
    int clientSteamIdOffset = -1;                    // CServerSideClientBase::m_SteamID (uint64)
    int clientSlotOffset = -1;                       // CServerSideClientBase::m_nClientSlot (int32)
    // Disconnects a client. Called from the game frame, never from inside an engine hook.
    void (*kick)(int slot, const char* reason) = nullptr;
};

// Install the hooks the engine facts allow. Logs one line per part that is on or off.
void S2Addons_Init(const S2AddonsEngine& engine);

// Lifecycle notifications from s2script_mm.cpp's existing hooks.
void S2Addons_OnStartupServer();                         // a map is starting (pre)
void S2Addons_OnClientConnected(uint64_t xuid);          // ISource2GameClients::OnClientConnected
void S2Addons_OnClientActive(uint64_t xuid);             // ISource2GameClients::ClientActive
void S2Addons_OnClientDisconnect(uint64_t xuid);         // ISource2GameClients::ClientDisconnect
void S2Addons_Frame();                                   // top of the game frame (kicks)

// Checked-hook teardown, in the shape of the other hook inventories.
bool S2AddonsHooksCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2AddonsHooksUnloadSync(const S2HookTerminalPermit& permit);
bool S2AddonsHooksRemovalComplete();

// Engine ops (core/engine-ops.jsonc, group "Addons"). Addon ids are decimal workshop ids; lists are
// comma-separated, in load order.
//
// Bit 0: mounting available (filesystem). Bit 1: map loads carry extra addons (host-state hook).
// Bit 2: client handshake available (ReplyConnection + SendNetMessage hooks).
int s2_addons_status();
// Mount `<folder>/<id>.vpk` (or the multi-chunk `<id>_dir.vpk`) into "GAME". 1 mounted, 0 already
// mounted, -1 no filesystem, -2 bad id, -3 no VPK in that folder.
int s2_addons_mount(const char* id, const char* folder, int add_to_tail);
// Remove a mount this module made. 1 removed, 0 not mounted by us.
int s2_addons_unmount(const char* id);
// The addons map loads should carry after the map's own one.
void s2_addons_set_extra(const char* csv);
// The addons every client must load, after the server's.
void s2_addons_set_client_global(const char* csv);
// The extra addons one client must load (SteamID64 as a decimal string), after the global ones.
void s2_addons_set_client(const char* xuid, const char* csv);
// Timeouts in seconds; `cache_seconds` 0 keeps a cached list forever.
void s2_addons_configure(double extra_timeout, double connect_timeout, int cache_clients, double cache_seconds);
// Forget what one client (or, for "" / "0", every client) has downloaded.
void s2_addons_clear_client_cache(const char* xuid);
// Unmount everything and clear every list (the managing plugin unloaded).
void s2_addons_reset();
// The current map's own workshop addon(s) ("" for a stock map), written NUL-terminated into `out`.
int s2_addons_current_map(char* out, int cap);
// The addons this module has mounted, comma-separated, in mount order.
int s2_addons_mounted(char* out, int cap);
// The addons the client with this SteamID64 is told to load (server + global + its own).
int s2_addons_client_list(const char* xuid, char* out, int cap);

#endif
