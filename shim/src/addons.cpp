// Native addon mounting. See addons.h for the model; this file is the engine side of it.
//
// Threading: the host-state and ReplyConnection hooks run on the main thread, the lifecycle
// notifications too, and engine ops come from the core on the main thread. SendNetMessage can run
// on a client's send job, so every piece of state below is guarded by one mutex. Nothing here
// calls into the core.
#include "addons.h"

#include "khook_map.h"

#include <filesystem/ifilesystem.h>
#include <networksystem/inetworkmessages.h>
#include <google/protobuf/descriptor.h>
#include <google/protobuf/message.h>

#include <ISmmPlugin.h>

PLUGIN_GLOBALVARS();

#include <algorithm>
#include <array>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <mutex>
#include <set>
#include <unordered_map>
#include <vector>

namespace {

using AddonList = std::vector<std::string>;

// --- small helpers --------------------------------------------------------------------------------

double Now() {
    using namespace std::chrono;
    return duration<double>(steady_clock::now().time_since_epoch()).count();
}

bool IsWorkshopId(const std::string& s) {
    return !s.empty() && s.size() <= 20 && s.find_first_not_of("0123456789") == std::string::npos &&
           s.find_first_not_of('0') != std::string::npos;
}

// Comma-separated ids, trimmed, empty entries dropped, duplicates dropped (first one wins).
AddonList ParseList(const char* csv) {
    AddonList out;
    if (!csv) return out;
    std::string cur;
    auto flush = [&] {
        size_t a = cur.find_first_not_of(" \t"), b = cur.find_last_not_of(" \t");
        if (a != std::string::npos) {
            std::string id = cur.substr(a, b - a + 1);
            if (std::find(out.begin(), out.end(), id) == out.end()) out.push_back(id);
        }
        cur.clear();
    };
    for (const char* p = csv; *p; ++p) {
        if (*p == ',') flush(); else cur.push_back(*p);
    }
    flush();
    return out;
}

std::string JoinList(const AddonList& list) {
    std::string out;
    for (const auto& id : list) {
        if (!out.empty()) out.push_back(',');
        out += id;
    }
    return out;
}

bool Contains(const AddonList& list, const std::string& id) {
    return std::find(list.begin(), list.end(), id) != list.end();
}

void AppendUnique(AddonList& list, const AddonList& more) {
    for (const auto& id : more)
        if (!Contains(list, id)) list.push_back(id);
}

int CopyOut(const std::string& s, char* out, int cap) {
    if (!out || cap <= 0) return 0;
    const size_t n = std::min(s.size(), static_cast<size_t>(cap - 1));
    std::memcpy(out, s.data(), n);
    out[n] = '\0';
    return static_cast<int>(n);
}

uint64_t ParseXuid(const char* s) {
    if (!s || !*s) return 0;
    char* end = nullptr;
    const unsigned long long v = std::strtoull(s, &end, 10);
    return (end && *end == '\0') ? static_cast<uint64_t>(v) : 0;
}

// --- tier0 exports (CUtlString, KeyValues) ------------------------------------------------------
//
// The engine's CUtlString and KeyValues live in libtier0, which is already loaded. Writing the
// engine's strings through tier0's own CUtlString::Set keeps every allocation on the engine's
// allocator. Looked up by symbol, never linked, so a missing export turns off what needs it.
struct Tier0 {
    void (*utlStringSet)(void* self, const char* value) = nullptr;            // CUtlString::Set
    const char* (*kvGetName)(const void* self) = nullptr;                     // KeyValues::GetName
    void* (*kvFindKey)(void* self, const char* key, bool create) = nullptr;   // KeyValues::FindKey
    const char* (*kvGetString)(void* self, const char* def, char* buf, size_t len) = nullptr;
};
Tier0 g_tier0;

bool ResolveTier0() {
    void* h = dlopen("libtier0.so", RTLD_NOW | RTLD_NOLOAD);
    if (!h) return false;
    g_tier0.utlStringSet = reinterpret_cast<decltype(g_tier0.utlStringSet)>(dlsym(h, "_ZN10CUtlString3SetEPKc"));
    g_tier0.kvGetName = reinterpret_cast<decltype(g_tier0.kvGetName)>(dlsym(h, "_ZNK9KeyValues7GetNameEv"));
    g_tier0.kvFindKey = reinterpret_cast<decltype(g_tier0.kvFindKey)>(dlsym(h, "_ZN9KeyValues7FindKeyEPKcb"));
    // KeyValues::GetString is an inline FindKey + CKeyValues_Data::Internal_GetString; KeyValues
    // derives from CKeyValues_Data first, so the key pointer is the data pointer.
    g_tier0.kvGetString = reinterpret_cast<decltype(g_tier0.kvGetString)>(
        dlsym(h, "_ZN15CKeyValues_Data18Internal_GetStringEPKcPcm"));
    dlclose(h);   // RTLD_NOLOAD took a reference; the engine keeps its own
    return g_tier0.utlStringSet != nullptr;
}

const char* ReadUtlString(const void* base, int offset) {
    if (!base || offset < 0) return "";
    const char* s = *reinterpret_cast<const char* const*>(static_cast<const unsigned char*>(base) + offset);
    return s ? s : "";
}

void WriteUtlString(void* base, int offset, const char* value) {
    if (!base || offset < 0 || !g_tier0.utlStringSet) return;
    g_tier0.utlStringSet(static_cast<unsigned char*>(base) + offset, value);
}

// --- state ----------------------------------------------------------------------------------------

// Active: fully in game. Its signon is over, so no SignonState it is sent is rewritten until the
// next map change starts a new signon.
enum class ConnState { None, Connecting, Joined, Active };

struct ClientState {
    double lastActive = 0;          // last time the server sent this client anything
    AddonList own;                  // addons only this client loads
    AddonList downloaded;           // addons this client already has
    std::string pending;            // the addon the client was last told to download
    ConnState conn = ConnState::None;
    double connectStart = 0;
};

struct Mount {
    std::string id;
    std::string path;               // the VPK path handed to AddSearchPath (no "_dir")
    bool tail = false;
};

S2AddonsEngine g_engine;
std::mutex g_mu;
std::vector<Mount> g_mounted;
AddonList g_extra;                  // carried by every map load after the map's own addon
AddonList g_clientGlobal;           // every client loads these too
std::string g_currentMap;           // the running map's own addon list ("" = stock map)
std::string g_loadingMap;           // the one the pending host-state request will load
std::unordered_map<uint64_t, ClientState> g_clients;
std::vector<int> g_timedOutSlots;   // kicked on the next frame (never inside an engine hook)
double g_extraTimeout = 10.0;
double g_connectTimeout = 30.0;
bool g_cacheClients = false;
double g_cacheSeconds = 0.0;
bool g_mountOn = false, g_hostStateOn = false, g_handshakeOn = false;
// Set by the first engine op that configures anything, cleared by s2_addons_reset. Until then the
// hooks only track which map is loading and change nothing, so a server that still runs another
// addon manager sees no difference.
bool g_active = false;

// The full ordered list a client must load: the map's own addon(s), the server's mounted extras,
// the global client list, then the client's own. Caller holds g_mu.
AddonList ClientAddonsLocked(uint64_t xuid) {
    AddonList out = ParseList(g_currentMap.c_str());
    for (const auto& m : g_mounted)
        if (!Contains(out, m.id)) out.push_back(m.id);
    AppendUnique(out, g_clientGlobal);
    if (xuid) {
        auto it = g_clients.find(xuid);
        if (it != g_clients.end()) AppendUnique(out, it->second.own);
    }
    return out;
}

bool AnyClientAddonsLocked() {
    if (!g_currentMap.empty() || !g_mounted.empty() || !g_clientGlobal.empty()) return true;
    for (const auto& kv : g_clients)
        if (!kv.second.own.empty()) return true;
    return false;
}

// A reconnect inside the timeout means the client fetched the addon it was sent away for. Runs from
// OnClientConnected, which the engine fires before it sends S2C_CONNECTION (ReplyConnection). A
// connect the client repeats while it is still downloading reaches ReplyConnection again WITHOUT
// this, which is what lets the connect timeout below tell "stuck" from "came back".
void CheckClientLocked(uint64_t xuid) {
    ClientState& c = g_clients[xuid];
    c.conn = ConnState::Joined;
    if (!c.pending.empty()) {
        if (Now() - c.lastActive <= g_extraTimeout && !Contains(c.downloaded, c.pending))
            c.downloaded.push_back(c.pending);
        c.pending.clear();
    }
    c.lastActive = Now();
}

// --- hooks ---------------------------------------------------------------------------------------

struct ClientReceiver {};

KHook::Return<void> HostStatePre(void* mgr, void* request);
KHook::Return<void> ReplyPre(void* server, void* client);
KHook::Return<void> ReplyPost(void* server, void* client);
KHook::Return<bool> SendNetMessagePre(ClientReceiver* client, const CNetMessage* msg, int bufType);

S2CheckedFunction<void, void*, void*> g_hostState(&HostStatePre, nullptr);
S2CheckedFunction<void, void*, void*> g_reply(&ReplyPre, &ReplyPost);
S2CheckedVirtual<ClientReceiver, bool, const CNetMessage*, int> g_send(&SendNetMessagePre, nullptr);
bool g_sendGlobalAdded = false;

std::array<S2CheckedBindingOps*, 3> Inventory() { return {{&g_hostState, &g_reply, &g_send}}; }

uint64_t ClientXuid(const void* client) {
    if (!client || g_engine.clientSteamIdOffset < 0) return 0;
    uint64_t v = 0;
    std::memcpy(&v, static_cast<const unsigned char*>(client) + g_engine.clientSteamIdOffset, sizeof v);
    return v;
}

int ClientSlot(const void* client) {
    if (!client || g_engine.clientSlotOffset < 0) return -1;
    int v = -1;
    std::memcpy(&v, static_cast<const unsigned char*>(client) + g_engine.clientSlotOffset, sizeof v);
    return v;
}

// Every map load. Works out which addon is the map's own (so it stays first and is never confused
// with ours), then rewrites the request's addon list to "map's own, then extras".
KHook::Return<void> HostStatePre(void* /*mgr*/, void* request) {
    auto obs = g_hostState.Observe();
    if (!S2Hook_EnterDispatch(obs) || !request) return S2_Ignore();
    auto* req = static_cast<unsigned char*>(request);

    std::lock_guard<std::mutex> lock(g_mu);
    std::string loading = g_currentMap;
    const std::string addons = ReadUtlString(req, g_engine.reqAddonsOffset);
    void* kv = g_engine.reqKeyValuesOffset >= 0
        ? *reinterpret_cast<void**>(req + g_engine.reqKeyValuesOffset) : nullptr;
    const char* kvName = (kv && g_tier0.kvGetName) ? g_tier0.kvGetName(kv) : nullptr;

    if (!kv) {
        // An end-of-match vote. A named addon is the map's; none on a stock map means "stock map";
        // none on a map that is not stock means the vote reloads the current workshop map.
        const std::string level = ReadUtlString(req, g_engine.reqLevelNameOffset);
        const std::string vpk = "maps/" + level + ".vpk";
        const bool stockMap = g_engine.fs && !level.empty() && g_engine.fs->FileExists(vpk.c_str(), "MOD");
        if (!addons.empty()) loading = addons;
        else if (stockMap) loading.clear();
    } else if (kvName && strcasecmp(kvName, "ChangeLevel") == 0) {
        // changelevel keeps the current workshop addon, unless it moves to a stock map.
        const std::string level = ReadUtlString(req, g_engine.reqLevelNameOffset);
        const std::string vpk = "maps/" + level + ".vpk";
        if (g_engine.fs && !level.empty() && g_engine.fs->FileExists(vpk.c_str(), "MOD")) loading.clear();
    } else if (kvName) {
        // "map_workshop" names its addon in customgamemode; any other request loads a stock map.
        loading.clear();
        if (strcasecmp(kvName, "map_workshop") == 0 && g_tier0.kvFindKey && g_tier0.kvGetString) {
            if (void* key = g_tier0.kvFindKey(kv, "customgamemode", false)) {
                char buf[64] = {0};
                const char* v = g_tier0.kvGetString(key, "", buf, sizeof buf);
                loading = v ? v : "";
            }
        }
    }
    // A community map's content is an official addon folder; the request already names it.
    if (!addons.empty() && g_engine.fs && g_engine.fs->IsDirectory(addons.c_str(), "OFFICIAL_ADDONS"))
        loading = addons;
    g_loadingMap = loading;

    if (!g_active || g_extra.empty()) return S2_Ignore();
    AddonList list = ParseList(loading.c_str());
    AppendUnique(list, g_extra);
    WriteUtlString(req, g_engine.reqAddonsOffset, JoinList(list).c_str());
    return S2_Ignore();
}

// S2C_CONNECTION copies the server's addon list. For the length of the original call, that list is
// swapped for this client's: everything it already has, plus the first one it still needs.
struct ReplySwap {
    void* server = nullptr;
    bool swapped = false;
    bool wasNull = false;
    std::string original;
};
thread_local ReplySwap t_reply;

KHook::Return<void> ReplyPre(void* server, void* client) {
    t_reply = ReplySwap{};
    auto obs = g_reply.Observe();
    if (!S2Hook_EnterDispatch(obs) || !server || !client) return S2_Ignore();
    const uint64_t xuid = ClientXuid(client);
    if (!xuid) return S2_Ignore();

    std::lock_guard<std::mutex> lock(g_mu);
    if (!g_active) return S2_Ignore();
    ClientState& c = g_clients[xuid];
    if (g_cacheClients && g_cacheSeconds > 0 && Now() - c.lastActive > g_cacheSeconds) {
        c.pending.clear();
        c.downloaded.clear();
    }
    c.lastActive = Now();

    AddonList want = ClientAddonsLocked(xuid);
    if (want.empty()) {
        c.pending.clear();
        return S2_Ignore();
    }
    if (c.conn != ConnState::Connecting) {
        c.conn = ConnState::Connecting;
        c.connectStart = Now();
    } else if (g_connectTimeout > 0 && Now() - c.connectStart > g_connectTimeout) {
        // Still on its first addon after the timeout: drop it on the next frame, and do not reply.
        const int slot = ClientSlot(client);
        if (slot >= 0) g_timedOutSlots.push_back(slot);
        return S2_Supersede();
    }
    if (!Contains(c.downloaded, want.front())) c.pending = want.front();
    // A client that is told about an addon it has not downloaded can fail its signature check and
    // drop at once, so the list holds only what it has plus the one it is about to fetch.
    AddonList send;
    for (const auto& id : want)
        if (Contains(c.downloaded, id) || id == c.pending) send.push_back(id);

    const char* raw = *reinterpret_cast<const char* const*>(
        static_cast<unsigned char*>(server) + g_engine.serverAddonNameOffset);
    t_reply.server = server;
    t_reply.wasNull = raw == nullptr;
    t_reply.original = raw ? raw : "";
    t_reply.swapped = true;
    WriteUtlString(server, g_engine.serverAddonNameOffset, JoinList(send).c_str());
    return S2_Ignore();
}

KHook::Return<void> ReplyPost(void* /*server*/, void* /*client*/) {
    if (t_reply.swapped && t_reply.server)
        WriteUtlString(t_reply.server, g_engine.serverAddonNameOffset,
                       t_reply.wasNull ? nullptr : t_reply.original.c_str());
    t_reply = ReplySwap{};
    return S2_Ignore();
}

// Every message the server sends a client. Keeps the client's "still here" time fresh, and turns
// a SignonState into a CHANGELEVEL that names the next addon the client still lacks.
KHook::Return<bool> SendNetMessagePre(ClientReceiver* client, const CNetMessage* msg, int /*bufType*/) {
    auto obs = g_send.Observe(client);
    if (!S2Hook_EnterDispatch(obs) || !client || !msg) return S2_Ignore(true);
    const uint64_t xuid = ClientXuid(client);
    if (!xuid) return S2_Ignore(true);

    std::lock_guard<std::mutex> lock(g_mu);
    if (!g_active || !AnyClientAddonsLocked()) return S2_Ignore(true);
    ClientState& c = g_clients[xuid];
    c.lastActive = Now();

    auto* net = const_cast<CNetMessage*>(msg);
    if (!g_engine.signonInfo || net->GetNetMessage() != g_engine.signonInfo) return S2_Ignore(true);
    auto* m = reinterpret_cast<google::protobuf::Message*>(net->AsProto());
    if (!m) return S2_Ignore(true);
    const auto* d = m->GetDescriptor();
    const auto* r = m->GetReflection();
    if (!d || !r || d->full_name() != "CNETMsg_SignonState") return S2_Ignore(true);
    const auto* fState = d->FindFieldByName("signon_state");
    const auto* fAddons = d->FindFieldByName("addons");
    if (!fState || !fAddons || fState->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_ENUM ||
        fAddons->cpp_type() != google::protobuf::FieldDescriptor::CPPTYPE_STRING || fAddons->is_repeated())
        return S2_Ignore(true);
    constexpr int kChangelevel = 7;   // SIGNONSTATE_CHANGELEVEL

    if (r->GetEnumValue(*m, fState) == kChangelevel) {
        // The engine's own map change. A client can only take one addon per change, so keep the
        // first and send the rest one by one afterwards.
        c.conn = ConnState::Joined;   // a new signon starts
        AddonList listed = ParseList(r->GetString(*m, fAddons).c_str());
        if (!listed.empty()) {
            if (listed.size() > 1) r->SetString(m, fAddons, listed.front());
            c.pending = listed.front();
        }
        return S2_Ignore(true);
    }

    if (c.conn == ConnState::Active) return S2_Ignore(true);
    AddonList want = ClientAddonsLocked(xuid);
    want.erase(std::remove_if(want.begin(), want.end(),
                              [&](const std::string& id) { return Contains(c.downloaded, id); }),
               want.end());
    if (want.empty()) return S2_Ignore(true);
    c.pending = want.front();
    r->SetString(m, fAddons, want.front());
    r->SetEnumValue(m, fState, kChangelevel);
    return S2_Ignore(true);
}

void Log(const char* fmt, const char* a = "", const char* b = "") {
    char buf[512];
    std::snprintf(buf, sizeof buf, fmt, a, b);
    if (g_SMAPI) META_CONPRINTF("[s2script] addons: %s\n", buf);
}

} // namespace

// --- lifecycle -----------------------------------------------------------------------------------

void S2Addons_Init(const S2AddonsEngine& engine) {
    g_engine = engine;
    const bool tier0 = ResolveTier0();

    g_mountOn = engine.fs != nullptr;
    Log(g_mountOn ? "mounting ON (VFileSystem017)" : "mounting OFF: filesystem interface unavailable");

    if (!tier0) {
        Log("map and client addon lists OFF: CUtlString::Set not exported by libtier0");
    } else if (!engine.hostStateRequestFn || engine.reqAddonsOffset < 0 || engine.reqKeyValuesOffset < 0 ||
               engine.reqLevelNameOffset < 0) {
        Log("map addon list OFF: %s", !engine.hostStateRequestFn ? "SetPendingHostStateRequest unresolved"
                                                               : "CHostStateRequest offsets absent from gamedata");
    } else {
        const auto receipt = g_hostState.Configure(engine.hostStateRequestFn);
        g_hostStateOn = receipt.Accepted();
        Log(g_hostStateOn ? "map addon list ON (SetPendingHostStateRequest hooked)%s"
                          : "map addon list OFF: hook refused (%s)", receipt.reason.c_str());
    }

    if (!tier0 || !engine.replyConnectionFn || !engine.sendNetMessage.vtable || !engine.signonInfo ||
        engine.serverAddonNameOffset < 0 || engine.clientSteamIdOffset < 0) {
        Log("client handshake OFF: %s",
            !engine.replyConnectionFn ? "ReplyConnection unresolved" :
            !engine.sendNetMessage.vtable ? "CServerSideClient::SendNetMessage unresolved" :
            !engine.signonInfo ? "CNETMsg_SignonState record not found" :
            "client/server offsets absent from gamedata");
        return;
    }
    const auto reply = g_reply.Configure(engine.replyConnectionFn);
    if (!reply.Accepted()) {
        Log("client handshake OFF: ReplyConnection hook refused (%s)", reply.reason.c_str());
        return;
    }
    g_send.Configure(engine.sendNetMessage.vtable_index);
    struct Holder { void** vptr; } holder{engine.sendNetMessage.vtable};
    const auto send = g_send.AddGlobal(reinterpret_cast<ClientReceiver*>(&holder));
    g_sendGlobalAdded = send.Accepted();
    if (!g_sendGlobalAdded) {
        // The ReplyConnection hook alone would hand out the first addon and never the rest; take
        // it back out so clients see the server's plain list.
        g_reply.BeginRemove();
        Log("client handshake OFF: SendNetMessage hook refused (%s)", send.reason.c_str());
        return;
    }
    g_handshakeOn = true;
    Log("client handshake ON (ReplyConnection + CServerSideClient::SendNetMessage slot %s)",
        std::to_string(engine.sendNetMessage.vtable_index).c_str());
}

void S2Addons_OnStartupServer() {
    std::lock_guard<std::mutex> lock(g_mu);
    g_currentMap = g_loadingMap;
    g_timedOutSlots.clear();
    // A new map is a new signon for everyone still connected.
    for (auto& kv : g_clients)
        if (kv.second.conn == ConnState::Active) kv.second.conn = ConnState::Joined;
    if (!g_engine.fs || !g_active) return;
    // Two or more addons leave empty search paths behind that break file writes.
    g_engine.fs->RemoveSearchPath("", "GAME");
    g_engine.fs->RemoveSearchPath("", "DEFAULT_WRITE_PATH");
    // Mount again in list order, as a client mounts them, so the same file wins on both sides.
    for (const auto& m : g_mounted) g_engine.fs->RemoveSearchPath(m.path.c_str(), "GAME");
    for (const auto& m : g_mounted)
        g_engine.fs->AddSearchPath(m.path.c_str(), "GAME", m.tail ? PATH_ADD_TO_TAIL : PATH_ADD_TO_HEAD,
                                   SEARCH_PATH_PRIORITY_VPK);
}

void S2Addons_OnClientConnected(uint64_t xuid) {
    if (!xuid || !g_handshakeOn) return;
    std::lock_guard<std::mutex> lock(g_mu);
    if (!g_active) return;
    CheckClientLocked(xuid);
}

void S2Addons_OnClientActive(uint64_t xuid) {
    if (!xuid) return;
    std::lock_guard<std::mutex> lock(g_mu);
    auto it = g_clients.find(xuid);
    if (it == g_clients.end()) return;
    it->second.conn = ConnState::Active;
    if (!g_cacheClients) it->second.downloaded.clear();
}

void S2Addons_OnClientDisconnect(uint64_t xuid) {
    if (!xuid) return;
    std::lock_guard<std::mutex> lock(g_mu);
    auto it = g_clients.find(xuid);
    if (it == g_clients.end()) return;
    it->second.lastActive = Now();
    it->second.conn = ConnState::None;
}

void S2Addons_Frame() {
    std::vector<int> slots;
    {
        std::lock_guard<std::mutex> lock(g_mu);
        if (g_timedOutSlots.empty()) return;
        slots.swap(g_timedOutSlots);
    }
    std::sort(slots.begin(), slots.end());
    slots.erase(std::unique(slots.begin(), slots.end()), slots.end());
    for (int slot : slots)
        if (g_engine.kick) g_engine.kick(slot, "Required Workshop addon download was not accepted in time");
}

bool S2AddonsHooksCanUnloadSync(const S2HookTerminalPermit& permit) {
    return S2HookInventoryCanRemoveSync(Inventory(), permit);
}

bool S2AddonsHooksUnloadSync(const S2HookTerminalPermit& permit) {
    if (!S2AddonsHooksCanUnloadSync(permit)) return false;
    if (g_sendGlobalAdded && g_engine.sendNetMessage.vtable) {
        struct Holder { void** vptr; } holder{g_engine.sendNetMessage.vtable};
        g_send.RemoveGlobal(reinterpret_cast<ClientReceiver*>(&holder));
        g_sendGlobalAdded = false;
    }
    return S2HookInventoryBeginRemoveSync(Inventory(), permit);
}

bool S2AddonsHooksRemovalComplete() { return S2HookInventoryRemovalComplete(Inventory()); }

// --- engine ops ----------------------------------------------------------------------------------

int s2_addons_status() {
    return (g_mountOn ? 1 : 0) | (g_hostStateOn ? 2 : 0) | (g_handshakeOn ? 4 : 0);
}

int s2_addons_mount(const char* id, const char* folder, int add_to_tail) {
    if (!g_engine.fs) return -1;
    const std::string key = id ? id : "";
    if (!IsWorkshopId(key)) return -2;
    std::string dir = folder ? folder : "";
    while (!dir.empty() && dir.back() == '/') dir.pop_back();
    // A multi-chunk addon is <id>_dir.vpk plus <id>_000.vpk...; the filesystem wants the name
    // without "_dir" either way and finds the chunks itself.
    const std::string base = dir + "/" + key;
    const std::string chunked = base + "_dir.vpk", single = base + ".vpk";
    if (dir.empty() || (!g_engine.fs->FileExists(chunked.c_str()) && !g_engine.fs->FileExists(single.c_str())))
        return -3;
    std::lock_guard<std::mutex> lock(g_mu);
    for (const auto& m : g_mounted)
        if (m.id == key) return 0;
    if (Contains(ParseList(g_currentMap.c_str()), key)) return 0;   // the map's own addon
    g_engine.fs->AddSearchPath(single.c_str(), "GAME", add_to_tail ? PATH_ADD_TO_TAIL : PATH_ADD_TO_HEAD,
                               SEARCH_PATH_PRIORITY_VPK);
    g_mounted.push_back({key, single, add_to_tail != 0});
    g_active = true;
    Log("mounted %s (%s)", key.c_str(), single.c_str());
    return 1;
}

int s2_addons_unmount(const char* id) {
    if (!g_engine.fs || !id) return 0;
    std::lock_guard<std::mutex> lock(g_mu);
    auto it = std::find_if(g_mounted.begin(), g_mounted.end(), [&](const Mount& m) { return m.id == id; });
    if (it == g_mounted.end()) return 0;
    g_engine.fs->RemoveSearchPath(it->path.c_str(), "GAME");
    Log("unmounted %s", it->id.c_str());
    g_mounted.erase(it);
    return 1;
}

void s2_addons_set_extra(const char* csv) {
    std::lock_guard<std::mutex> lock(g_mu);
    g_active = true;
    g_extra.clear();
    for (const auto& id : ParseList(csv))
        if (IsWorkshopId(id)) g_extra.push_back(id);
}

void s2_addons_set_client_global(const char* csv) {
    std::lock_guard<std::mutex> lock(g_mu);
    g_active = true;
    g_clientGlobal.clear();
    for (const auto& id : ParseList(csv))
        if (IsWorkshopId(id)) g_clientGlobal.push_back(id);
}

void s2_addons_set_client(const char* xuid, const char* csv) {
    const uint64_t key = ParseXuid(xuid);
    if (!key) return;
    std::lock_guard<std::mutex> lock(g_mu);
    g_active = true;
    AddonList& own = g_clients[key].own;
    own.clear();
    for (const auto& id : ParseList(csv))
        if (IsWorkshopId(id)) own.push_back(id);
}

void s2_addons_configure(double extra_timeout, double connect_timeout, int cache_clients, double cache_seconds) {
    std::lock_guard<std::mutex> lock(g_mu);
    g_active = true;
    g_extraTimeout = extra_timeout > 0 ? extra_timeout : 0;
    g_connectTimeout = connect_timeout > 0 ? connect_timeout : 0;
    g_cacheClients = cache_clients != 0;
    g_cacheSeconds = cache_seconds > 0 ? cache_seconds : 0;
}

void s2_addons_clear_client_cache(const char* xuid) {
    const uint64_t key = ParseXuid(xuid);
    std::lock_guard<std::mutex> lock(g_mu);
    for (auto& kv : g_clients) {
        if (key && kv.first != key) continue;
        kv.second.downloaded.clear();
        kv.second.pending.clear();
    }
}

void s2_addons_reset() {
    std::lock_guard<std::mutex> lock(g_mu);
    if (g_engine.fs)
        for (const auto& m : g_mounted) g_engine.fs->RemoveSearchPath(m.path.c_str(), "GAME");
    g_mounted.clear();
    g_extra.clear();
    g_clientGlobal.clear();
    g_clients.clear();
    g_timedOutSlots.clear();
    g_active = false;
}

int s2_addons_current_map(char* out, int cap) {
    std::lock_guard<std::mutex> lock(g_mu);
    return CopyOut(g_currentMap, out, cap);
}

int s2_addons_mounted(char* out, int cap) {
    std::lock_guard<std::mutex> lock(g_mu);
    AddonList ids;
    for (const auto& m : g_mounted) ids.push_back(m.id);
    return CopyOut(JoinList(ids), out, cap);
}

int s2_addons_client_list(const char* xuid, char* out, int cap) {
    std::lock_guard<std::mutex> lock(g_mu);
    return CopyOut(JoinList(ClientAddonsLocked(ParseXuid(xuid))), out, cap);
}
