// Steam game-server services — see steam.h for the design.
#include "steam.h"
#include "s2script_core.h"

#include <ISmmPlugin.h>

#include <steam/steam_api_common.h>
#include <steam/isteamgameserver.h>
#include <steam/isteamugc.h>
#include <steam/isteamuser.h>

#include <dlfcn.h>
#include <link.h>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

PLUGIN_GLOBALVARS();

namespace {

// The libsteam_api.so entry points we borrow. Looked up by name in the copy the engine loaded.
using GetHSteamUserFn = HSteamUser (*)();
using FindOrCreateGameServerInterfaceFn = void* (*)(HSteamUser, const char*);
using RegisterCallbackFn = void (*)(CCallbackBase*, int);
using UnregisterCallbackFn = void (*)(CCallbackBase*);

GetHSteamUserFn s_getUser = nullptr;
FindOrCreateGameServerInterfaceFn s_findOrCreate = nullptr;
RegisterCallbackFn s_register = nullptr;
UnregisterCallbackFn s_unregister = nullptr;
bool (*s_onOwnerThread)() = nullptr;
bool s_initTried = false;

// The interfaces are owned by Steam and live for one game-server session (one HSteamUser). They
// are re-requested whenever the engine's session handle changes, and dropped when it is 0.
HSteamUser s_user = 0;
ISteamGameServer* s_server = nullptr;
ISteamUGC* s_ugc = nullptr;
bool s_versionWarned = false;

void Log(const char* line) {
    if (g_SMAPI) META_CONPRINTF("%s", line);
}

// Find libsteam_api.so among the loaded objects: first by soname, then by path, because the engine
// loads it from bin/<platform>/ with a full path. RTLD_NOLOAD: never load a second copy.
void* OpenLoadedSteamApi() {
    if (void* h = dlopen("libsteam_api.so", RTLD_NOW | RTLD_NOLOAD)) return h;
    std::string path;
    dl_iterate_phdr([](dl_phdr_info* info, size_t, void* out) -> int {
        const char* name = info->dlpi_name;
        if (!name) return 0;
        const char* slash = std::strrchr(name, '/');
        if (std::strcmp(slash ? slash + 1 : name, "libsteam_api.so") != 0) return 0;
        *static_cast<std::string*>(out) = name;
        return 1;
    }, &path);
    return path.empty() ? nullptr : dlopen(path.c_str(), RTLD_NOW | RTLD_NOLOAD);
}

// Resolve (or drop) the interfaces for the engine's current game-server session.
void Refresh() {
    if (!s_getUser || !s_findOrCreate) return;
    const HSteamUser user = s_getUser();
    if (user == s_user && (s_ugc || user == 0)) return;
    s_user = user;
    s_server = nullptr;
    s_ugc = nullptr;
    if (user == 0) return;   // the engine has not activated (or has shut down) its Steam session
    s_server = static_cast<ISteamGameServer*>(s_findOrCreate(user, STEAMGAMESERVER_INTERFACE_VERSION));
    s_ugc = static_cast<ISteamUGC*>(s_findOrCreate(user, STEAMUGC_INTERFACE_VERSION));
    if ((!s_server || !s_ugc) && !s_versionWarned) {
        s_versionWarned = true;
        char line[256];
        std::snprintf(line, sizeof line,
                      "[s2script] WARN: Steam does not serve %s%s%s; @s2script/sdk/steam parts OFF\n",
                      s_server ? "" : STEAMGAMESERVER_INTERFACE_VERSION,
                      (!s_server && !s_ugc) ? " / " : "",
                      s_ugc ? "" : STEAMUGC_INTERFACE_VERSION);
        Log(line);
    }
}

ISteamUGC* Ugc() { Refresh(); return s_ugc; }
ISteamGameServer* Server() { Refresh(); return s_server; }

// --- callbacks -----------------------------------------------------------------------------------

struct Pending { int kind; uint64_t id; int result; int flag; };
std::mutex s_pendingMu;
std::vector<Pending> s_pending;
constexpr size_t kMaxPending = 4096;

void Deliver(int kind, uint64_t id, int result, int flag) {
    if (s_onOwnerThread && s_onOwnerThread()) {
        s2script_core_dispatch_steam_event(kind, id, result, flag);
        return;
    }
    std::lock_guard<std::mutex> lock(s_pendingMu);
    if (s_pending.size() < kMaxPending) s_pending.push_back({kind, id, result, flag});
}

// One registered game-server callback. CCallbackBase is the Steamworks ABI for a callback object:
// Steam calls Run(void*) with the callback struct while the engine runs SteamGameServer_RunCallbacks.
// Its flags and id are protected members, so a subclass is the only way to fill them in.
template <typename T>
class GameServerCallback final : public CCallbackBase {
public:
    using Handler = void (*)(const T&);
    explicit GameServerCallback(Handler h) : handler_(h) {
        m_nCallbackFlags = k_ECallbackFlagsGameServer;
        m_iCallback = T::k_iCallback;
    }
    void Register() {
        if (registered_ || !s_register) return;
        s_register(this, T::k_iCallback);   // sets k_ECallbackFlagsRegistered itself
        registered_ = true;
    }
    void Unregister() {
        if (!registered_ || !s_unregister) return;
        s_unregister(this);
        registered_ = false;
    }
    void Run(void* param) override { if (param) handler_(*static_cast<const T*>(param)); }
    void Run(void* param, bool, SteamAPICall_t) override { Run(param); }
    int GetCallbackSizeBytes() override { return static_cast<int>(sizeof(T)); }

private:
    Handler handler_;
    bool registered_ = false;
};

GameServerCallback<SteamServersConnected_t> s_cbConnected([](const SteamServersConnected_t&) {
    Refresh();
    Deliver(S2_STEAM_EVENT_CONNECTED, 0, 0, 0);
});
GameServerCallback<SteamServersDisconnected_t> s_cbDisconnected([](const SteamServersDisconnected_t& p) {
    Deliver(S2_STEAM_EVENT_DISCONNECTED, 0, static_cast<int>(p.m_eResult), 0);
});
GameServerCallback<SteamServerConnectFailure_t> s_cbConnectFailure([](const SteamServerConnectFailure_t& p) {
    Deliver(S2_STEAM_EVENT_CONNECT_FAILURE, 0, static_cast<int>(p.m_eResult), p.m_bStillRetrying ? 1 : 0);
});
GameServerCallback<DownloadItemResult_t> s_cbDownload([](const DownloadItemResult_t& p) {
    Deliver(S2_STEAM_EVENT_DOWNLOAD_RESULT, p.m_nPublishedFileId, static_cast<int>(p.m_eResult),
            static_cast<int>(p.m_unAppID));
});
GameServerCallback<ItemInstalled_t> s_cbInstalled([](const ItemInstalled_t& p) {
    Deliver(S2_STEAM_EVENT_ITEM_INSTALLED, p.m_nPublishedFileId, 0, static_cast<int>(p.m_unAppID));
});

}  // namespace

void S2Steam_Init(bool (*on_owner_thread)()) {
    s_onOwnerThread = on_owner_thread;
    if (s_initTried) return;
    s_initTried = true;
    void* lib = OpenLoadedSteamApi();
    if (!lib) {
        Log("[s2script] WARN: libsteam_api.so is not loaded; @s2script/sdk/steam OFF\n");
        return;
    }
    s_getUser = reinterpret_cast<GetHSteamUserFn>(dlsym(lib, "SteamGameServer_GetHSteamUser"));
    s_findOrCreate = reinterpret_cast<FindOrCreateGameServerInterfaceFn>(
        dlsym(lib, "SteamInternal_FindOrCreateGameServerInterface"));
    s_register = reinterpret_cast<RegisterCallbackFn>(dlsym(lib, "SteamAPI_RegisterCallback"));
    s_unregister = reinterpret_cast<UnregisterCallbackFn>(dlsym(lib, "SteamAPI_UnregisterCallback"));
    // dlopen(RTLD_NOLOAD) took a reference; the engine keeps the library loaded for the process
    // lifetime, so dropping ours now leaves the resolved addresses valid.
    dlclose(lib);
    const char* missing = !s_getUser ? "SteamGameServer_GetHSteamUser"
                        : !s_findOrCreate ? "SteamInternal_FindOrCreateGameServerInterface"
                        : !s_register ? "SteamAPI_RegisterCallback"
                        : !s_unregister ? "SteamAPI_UnregisterCallback" : nullptr;
    if (missing) {
        char line[192];
        std::snprintf(line, sizeof line, "[s2script] WARN: libsteam_api.so has no %s; @s2script/sdk/steam OFF\n",
                      missing);
        Log(line);
        s_getUser = nullptr;
        s_findOrCreate = nullptr;
        s_register = nullptr;
        s_unregister = nullptr;
        return;
    }
    s_cbConnected.Register();
    s_cbDisconnected.Register();
    s_cbConnectFailure.Register();
    s_cbDownload.Register();
    s_cbInstalled.Register();
    Log("[s2script] Steam game-server callbacks registered (@s2script/sdk/steam)\n");
}

void S2Steam_Shutdown() {
    s_cbConnected.Unregister();
    s_cbDisconnected.Unregister();
    s_cbConnectFailure.Unregister();
    s_cbDownload.Unregister();
    s_cbInstalled.Unregister();
    s_onOwnerThread = nullptr;
    s_user = 0;
    s_server = nullptr;
    s_ugc = nullptr;
    // A later Load (Metamod reload of this module) resolves and registers again.
    s_initTried = false;
    std::lock_guard<std::mutex> lock(s_pendingMu);
    s_pending.clear();
}

void S2Steam_Drain() {
    std::vector<Pending> batch;
    {
        std::lock_guard<std::mutex> lock(s_pendingMu);
        if (s_pending.empty()) return;
        batch.swap(s_pending);
    }
    for (const auto& p : batch) s2script_core_dispatch_steam_event(p.kind, p.id, p.result, p.flag);
}

// --- engine ops ----------------------------------------------------------------------------------

int s2_steam_status() {
    ISteamGameServer* server = Server();
    if (!server || !Ugc()) return 0;
    int bits = 1;
    if (server->BLoggedOn()) bits |= 2;
    if (server->BSecure()) bits |= 4;
    return bits;
}

uint64_t s2_steam_server_id() {
    ISteamGameServer* server = Server();
    if (!server || !server->BLoggedOn()) return 0;
    return server->GetSteamID().ConvertToUint64();
}

uint32_t s2_steam_public_ip() {
    ISteamGameServer* server = Server();
    if (!server) return 0;
    const SteamIPAddress_t ip = server->GetPublicIP();
    return ip.m_eType == k_ESteamIPTypeIPv4 ? ip.m_unIPv4 : 0;
}

int s2_steam_ugc_download(uint64_t id, int high_priority) {
    ISteamUGC* ugc = Ugc();
    if (!ugc || id == 0) return 0;
    return ugc->DownloadItem(static_cast<PublishedFileId_t>(id), high_priority != 0) ? 1 : 0;
}

long long s2_steam_ugc_state(uint64_t id) {
    ISteamUGC* ugc = Ugc();
    if (!ugc || id == 0) return -1;
    return static_cast<long long>(ugc->GetItemState(static_cast<PublishedFileId_t>(id)));
}

int s2_steam_ugc_download_info(uint64_t id, uint64_t* downloaded, uint64_t* total) {
    ISteamUGC* ugc = Ugc();
    if (!ugc || id == 0 || !downloaded || !total) return 0;
    uint64 d = 0, t = 0;
    if (!ugc->GetItemDownloadInfo(static_cast<PublishedFileId_t>(id), &d, &t)) return 0;
    *downloaded = d;
    *total = t;
    return 1;
}

int s2_steam_ugc_install_info(uint64_t id, uint64_t* size_on_disk, char* folder, int cap, uint32_t* timestamp) {
    ISteamUGC* ugc = Ugc();
    if (!ugc || id == 0 || !size_on_disk || !folder || cap <= 0 || !timestamp) return 0;
    uint64 size = 0;
    uint32 ts = 0;
    folder[0] = '\0';
    if (!ugc->GetItemInstallInfo(static_cast<PublishedFileId_t>(id), &size, folder,
                                 static_cast<uint32>(cap), &ts)) {
        folder[0] = '\0';
        return 0;
    }
    folder[cap - 1] = '\0';
    *size_on_disk = size;
    *timestamp = ts;
    return 1;
}

int s2_steam_ugc_suspend(int suspend) {
    ISteamUGC* ugc = Ugc();
    if (!ugc) return 0;
    ugc->SuspendDownloads(suspend != 0);
    return 1;
}
