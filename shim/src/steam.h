// Steam game-server services (@s2script/sdk/steam): the server's own Steam session and Steam
// Workshop (ISteamUGC) downloads.
//
// Engine-generic: every Source 2 dedicated server runs the Steamworks game-server API, so nothing
// here names a game. The engine owns the Steam session (it calls SteamGameServer_Init and runs the
// callbacks every frame); this TU only borrows it.
//
// Nothing is linked: libsteam_api.so is already loaded by the engine, and the four entry points we
// need are looked up in that loaded copy. The interfaces are requested by the version strings of the
// pinned hl2sdk headers we compile against, so the compiled vtable calls and the interface Steam hands
// back always agree; a Steam client that no longer serves that version answers null and the feature
// turns off with a named reason instead of calling a mismatched vtable.
#ifndef S2SCRIPT_STEAM_H
#define S2SCRIPT_STEAM_H

#include <cstdint>

// Event kinds handed to the core. Mirrors core/src/steam.rs `SteamEvent`.
enum S2SteamEventKind {
    S2_STEAM_EVENT_CONNECTED = 0,       // SteamServersConnected_t
    S2_STEAM_EVENT_DISCONNECTED = 1,    // SteamServersDisconnected_t (result)
    S2_STEAM_EVENT_CONNECT_FAILURE = 2, // SteamServerConnectFailure_t (result, stillRetrying)
    S2_STEAM_EVENT_DOWNLOAD_RESULT = 3, // DownloadItemResult_t (id, result, appId)
    S2_STEAM_EVENT_ITEM_INSTALLED = 4,  // ItemInstalled_t (id, appId)
};

// Resolve libsteam_api.so and register the game-server callbacks. `on_owner_thread` says whether the
// calling thread may enter the core; a callback that fires anywhere else is queued for S2Steam_Drain.
// Logs one line naming the reason when Steam is unavailable. Safe to call more than once.
void S2Steam_Init(bool (*on_owner_thread)());
// Unregister every callback. Called before the core shuts down, so Steam never calls into a core
// that is gone, or into this module after Metamod unmaps it.
void S2Steam_Shutdown();
// Deliver callbacks that fired off the owner thread. Called at the top of the game frame.
void S2Steam_Drain();

// Engine ops (core/engine-ops.jsonc, group "Steam game server + workshop").
// Bit 0: interfaces resolved. Bit 1: logged on to Steam. Bit 2: VAC secure.
int s2_steam_status();
// The server's SteamID64, or 0 before it has logged on.
uint64_t s2_steam_server_id();
// The server's public IPv4 address as Steam sees it (host byte order), or 0.
uint32_t s2_steam_public_ip();
// ISteamUGC::DownloadItem. 1 when Steam accepted the request (a DownloadItemResult follows).
int s2_steam_ugc_download(uint64_t id, int high_priority);
// ISteamUGC::GetItemState flags, or -1 when Steam is unavailable.
long long s2_steam_ugc_state(uint64_t id);
// ISteamUGC::GetItemDownloadInfo. 1 when the item has download info.
int s2_steam_ugc_download_info(uint64_t id, uint64_t* downloaded, uint64_t* total);
// ISteamUGC::GetItemInstallInfo. 1 when installed; `folder` is NUL-terminated within `cap`.
int s2_steam_ugc_install_info(uint64_t id, uint64_t* size_on_disk, char* folder, int cap, uint32_t* timestamp);
// ISteamUGC::SuspendDownloads. 1 when Steam was reachable.
int s2_steam_ugc_suspend(int suspend);

#endif
