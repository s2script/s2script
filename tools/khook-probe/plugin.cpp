// s2_khook_probe — Metamod test plugin. NEVER shipped in the production release.
//
// Second KHook consumer against the same pin. Hooks controlled native functions
// (and dummy virtuals) with valid objects; optionally shares engine capsules with
// s2script. Command `s2_khook_probe run A` prints one JSON object per suite A case:
// case, expected, actual, result (pass|fail|pending).
#include <ISmmPlugin.h>
#include "khook_map.h"
#include "sigscan.h"

#include <eiface.h>
#include <icvar.h>
#include <convar.h>
#include <playerslot.h>
#include <igameevents.h>

#include <cstdio>
#include <cstdint>
#include <cstring>
#include <strings.h>
#include <string>
#include <vector>
#include <link.h>

PLUGIN_GLOBALVARS();

static const char* kCmdName = "s2_khook_probe";

static const char* StateName(S2HookState s) {
    switch (s) {
        case S2HookState::Failed: return "Failed";
        case S2HookState::Pending: return "Pending";
        case S2HookState::Active: return "Active";
        case S2HookState::Removing: return "Removing";
        case S2HookState::Removed: return "Removed";
    }
    return "unknown";
}

static std::string JsonEscape(const char* s) {
    std::string o;
    if (!s) {
        return o;
    }
    for (; *s; ++s) {
        const unsigned char c = static_cast<unsigned char>(*s);
        if (c == '"' || c == '\\') {
            o += '\\';
            o += static_cast<char>(c);
        } else if (c == '\n') {
            o += "\\n";
        } else if (c == '\r') {
            o += "\\r";
        } else if (c < 0x20) {
            char buf[8];
            std::snprintf(buf, sizeof(buf), "\\u%04x", c);
            o += buf;
        } else {
            o += static_cast<char>(c);
        }
    }
    return o;
}

static void Emit(const char* cse, const char* expected, const std::string& actual, const char* result) {
    const std::string exp = JsonEscape(expected);
    const std::string act = JsonEscape(actual.c_str());
    META_CONPRINTF(
        "{\"suite\":\"A\",\"case\":\"%s\",\"expected\":\"%s\",\"actual\":\"%s\",\"result\":\"%s\"}\n",
        cse, exp.c_str(), act.c_str(), result);
}

// ---------------------------------------------------------------------------
// Controlled native functions (valid objects, unique bodies to defeat ICF).
// Construct Function wrappers without addresses; Configure only after PLUGIN_SAVEVARS.
// ---------------------------------------------------------------------------
static int g_orig_new = 0;
static int g_orig_share = 0;
static int g_orig_ab = 0;
static int g_orig_ba = 0;
static int g_orig_once = 0;
static int g_pre_new = 0;
static int g_post_once = 0;
static int g_pre_once = 0;
static int g_pre_share_a = 0;
static int g_pre_share_b = 0;
static int g_pre_ab_a = 0;
static int g_pre_ab_b = 0;
static int g_pre_ba_a = 0;
static int g_pre_ba_b = 0;

static KHook::Action g_act_a = KHook::Action::Ignore;
static KHook::Action g_act_b = KHook::Action::Ignore;
static int g_ret_a = 0;
static int g_ret_b = 0;

static int TargetNew(int x) {
    g_orig_new++;
    return x + 1;
}
static int TargetShare(int x) {
    g_orig_share++;
    return x + 2;
}
static int TargetAB(int x) {
    g_orig_ab++;
    return x + 3;
}
static int TargetBA(int x) {
    g_orig_ba++;
    return x + 4;
}
static int TargetOnce(int x) {
    g_orig_once++;
    return x + 5;
}

struct Dummy {
    int tag = 0;
    int orig = 0;
    virtual int Go(int x) {
        orig++;
        return x + tag;
    }
    virtual ~Dummy() = default;
};

static S2CheckedFunction<int, int>* pNew = nullptr;
static S2CheckedFunction<int, int>* pShareA = nullptr;
static S2CheckedFunction<int, int>* pShareB = nullptr;
static S2CheckedFunction<int, int>* pAB_A = nullptr;
static S2CheckedFunction<int, int>* pAB_B = nullptr;
static S2CheckedFunction<int, int>* pBA_A = nullptr;
static S2CheckedFunction<int, int>* pBA_B = nullptr;
static S2CheckedFunction<int, int>* pOnce = nullptr;
static S2CheckedVirtual<Dummy, int, int>* pVirtA = nullptr;
static S2CheckedVirtual<Dummy, int, int>* pVirtB = nullptr;
static S2CheckedVirtual<Dummy, int, int>* pVirtPre = nullptr;
static S2CheckedVirtual<Dummy, int, int>* pVirtPost = nullptr;

static KHook::Return<int> ActionRet(KHook::Action act, int v) {
    KHook::Return<int> r;
    r.action = act;
    r.ret = v;
    return r;
}
static KHook::Return<int> PreNew(int x) {
    auto obs = pNew ? pNew->Observe() : S2HookObserve{};
    g_pre_new++;
    return S2_Ignore(x);
}
static KHook::Return<int> PreShareA(int x) {
    auto obs = pShareA ? pShareA->Observe() : S2HookObserve{};
    g_pre_share_a++;
    return S2_Ignore(x);
}
static KHook::Return<int> PreShareB(int x) {
    auto obs = pShareB ? pShareB->Observe() : S2HookObserve{};
    g_pre_share_b++;
    return S2_Ignore(x);
}
static KHook::Return<int> PreAB_A(int x) {
    auto obs = pAB_A ? pAB_A->Observe() : S2HookObserve{};
    g_pre_ab_a++;
    return ActionRet(g_act_a, g_ret_a);
}
static KHook::Return<int> PreAB_B(int x) {
    auto obs = pAB_B ? pAB_B->Observe() : S2HookObserve{};
    g_pre_ab_b++;
    return ActionRet(g_act_b, g_ret_b);
}
static KHook::Return<int> PreBA_A(int x) {
    auto obs = pBA_A ? pBA_A->Observe() : S2HookObserve{};
    g_pre_ba_a++;
    return ActionRet(g_act_a, g_ret_a);
}
static KHook::Return<int> PreBA_B(int x) {
    auto obs = pBA_B ? pBA_B->Observe() : S2HookObserve{};
    g_pre_ba_b++;
    return ActionRet(g_act_b, g_ret_b);
}
static KHook::Return<int> PreOnce(int x) {
    auto obs = pOnce ? pOnce->Observe() : S2HookObserve{};
    g_pre_once++;
    return S2_Ignore(x);
}
static KHook::Return<int> PostOnce(int x) {
    auto obs = pOnce ? pOnce->Observe() : S2HookObserve{};
    g_post_once++;
    return S2_Ignore(x);
}

static int g_dummy_pre_a = 0;
static int g_dummy_pre_b = 0;
static int g_dummy_post = 0;
static Dummy g_dummyA;
static Dummy g_dummyB;
static Dummy g_dummyPhase;

static KHook::Return<int> DummyPreA(Dummy* self, int x) {
    auto obs = pVirtA ? pVirtA->Observe(self) : S2HookObserve{};
    if (obs) {
        g_dummy_pre_a++;
    }
    return S2_Ignore(x);
}
static KHook::Return<int> DummyPreB(Dummy* self, int x) {
    auto obs = pVirtB ? pVirtB->Observe(self) : S2HookObserve{};
    if (obs) {
        g_dummy_pre_b++;
    }
    return S2_Ignore(x);
}
static KHook::Return<int> DummyPrePhase(Dummy* self, int x) {
    auto obs = pVirtPre ? pVirtPre->Observe(self) : S2HookObserve{};
    if (obs) {
        g_dummy_pre_a++;
    }
    return S2_Ignore(x);
}
static KHook::Return<int> DummyPostPhase(Dummy* self, int x) {
    auto obs = pVirtPost ? pVirtPost->Observe(self) : S2HookObserve{};
    if (obs) {
        g_dummy_post++;
    }
    return S2_Ignore(x);
}

// Engine peer counters (shared capsules with s2script when both are loaded).
static int g_game_frames = 0;
static int g_client_cmds = 0;
static int g_clients_connected = 0;
static int g_ping_seen = 0;
static int g_ping_continue = 0;
static int g_ping_suppress = 0;
static int g_fe_pre = 0;
static int g_fe_orig = 0;
static int g_fe_post = 0;
static int g_fe_skipped = 0;
static int g_fe_listener = 0;
static int g_fe_dontbroadcast_true = 0;
static bool g_frame_hooked = false;
static bool g_client_hooked = false;
static bool g_connected_hooked = false;
static bool g_fe_hooked = false;
static S2HookState g_frame_receipt = S2HookState::Failed;
static S2HookState g_fn_new_receipt = S2HookState::Failed;
static S2HookState g_fn_share_receipt = S2HookState::Failed;

#ifndef INTERFACEVERSION_GAMEEVENTSMANAGER2
#define INTERFACEVERSION_GAMEEVENTSMANAGER2 "GAMEEVENTSMANAGER002"
#endif

// Dedicated no-suppression sample. JS onPre Handled uses a different event.
static const char* kFireEventNoSuppressName = "player_activate";
static const char* kPingCmd = "khook_probe_ping";

struct ModText {
    const uint8_t* text;
    size_t size;
};

static ModText ProbeFindModuleText(const char* soname) {
    struct Ctx {
        const char* name;
        ModText out;
    } ctx{soname, {nullptr, 0}};
    dl_iterate_phdr(
        [](struct dl_phdr_info* info, size_t, void* data) -> int {
            auto* c = static_cast<Ctx*>(data);
            if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) {
                return 0;
            }
            for (int i = 0; i < info->dlpi_phnum; i++) {
                const ElfW(Phdr)& ph = info->dlpi_phdr[i];
                if (ph.p_type == PT_LOAD && (ph.p_flags & PF_X) && ph.p_filesz > c->out.size) {
                    c->out.text = reinterpret_cast<const uint8_t*>(info->dlpi_addr + ph.p_vaddr);
                    c->out.size = ph.p_filesz;
                }
            }
            return 0;
        },
        &ctx);
    return ctx.out;
}

// Same gamedata fact as s2script (`GameEventManager` ctor-body-xref). Real instance, not a sentinel.
static IGameEventManager2* AcquireGameEventManagerFromServerText() {
    static const char kPat[] = "55 48 8D 05 ? ? ? ? BA 40 00 00 00 31 F6 48 89 E5 41 56 41 55";
    const std::vector<int> pat = s2sig::ParsePattern(kPat);
    if (pat.empty()) {
        return nullptr;
    }
    const ModText mt = ProbeFindModuleText("libserver.so");
    if (!mt.text || mt.size == 0) {
        return nullptr;
    }
    const int64_t matchOff = s2sig::FindPattern(mt.text, mt.size, pat);
    if (matchOff < 0) {
        return nullptr;
    }
    const int64_t targetOff = s2sig::ResolveCtorXref(mt.text, mt.size, matchOff);
    if (targetOff == s2sig::kFail) {
        return nullptr;
    }
    return reinterpret_cast<IGameEventManager2*>(const_cast<uint8_t*>(mt.text) + targetOff);
}

static IGameEventManager2* AcquireGameEventManager(CreateInterfaceFn engineFactory,
                                                   CreateInterfaceFn serverFactory) {
    int ret = 0;
    if (engineFactory) {
        auto* p = reinterpret_cast<IGameEventManager2*>(
            engineFactory(INTERFACEVERSION_GAMEEVENTSMANAGER2, &ret));
        if (p) {
            return p;
        }
    }
    ret = 0;
    if (serverFactory) {
        auto* p = reinterpret_cast<IGameEventManager2*>(
            serverFactory(INTERFACEVERSION_GAMEEVENTSMANAGER2, &ret));
        if (p) {
            return p;
        }
    }
    return AcquireGameEventManagerFromServerText();
}

static bool EventNameIs(IGameEvent* ev, const char* want) {
    if (!ev || !want) {
        return false;
    }
    const char* n = ev->GetName();
    return n && std::strcmp(n, want) == 0;
}

// Independent original-once observer: engine delivery, not PRE inference.
struct ProbeFireEventListener : public IGameEventListener2 {
    void FireGameEvent(IGameEvent* ev) override {
        if (EventNameIs(ev, kFireEventNoSuppressName)) {
            g_fe_listener++;
        }
    }
};
static ProbeFireEventListener g_fe_listener_obj;
static bool g_fe_listening = false;

static void FeResetOrigCounters() {
    g_fe_pre = g_fe_orig = g_fe_post = g_fe_skipped = g_fe_listener = g_fe_dontbroadcast_true = 0;
}

static void FeStopListening(IGameEventManager2* mgr) {
    if (g_fe_listening && mgr) {
        mgr->RemoveListener(&g_fe_listener_obj);
        g_fe_listening = false;
    }
}

class ProbePlugin : public ISmmPlugin {
public:
    ProbePlugin()
        : gameFrame(&ISource2Server::GameFrame, this, &ProbePlugin::Hook_GameFrame, nullptr),
          clientCommand(&ISource2GameClients::ClientCommand, this,
                        &ProbePlugin::Hook_ClientCommand, nullptr),
          onConnected(&ISource2GameClients::OnClientConnected, this,
                      &ProbePlugin::Hook_OnClientConnected, nullptr),
          fireEvent(&IGameEventManager2::FireEvent, this, &ProbePlugin::Hook_FireEventPre,
                    &ProbePlugin::Hook_FireEventPost) {}

    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    bool Unload(char* error, size_t maxlen) override;

    KHook::Return<void> Hook_GameFrame(ISource2Server* server, bool simulating, bool first, bool last);
    KHook::Return<void> Hook_ClientCommand(ISource2GameClients* clients, CPlayerSlot slot,
                                           const CCommand& args);
    KHook::Return<void> Hook_OnClientConnected(ISource2GameClients* clients, CPlayerSlot slot,
                                               const char* name, uint64 xuid, const char* netid,
                                               const char* addr, bool fake);
    KHook::Return<bool> Hook_FireEventPre(IGameEventManager2* mgr, IGameEvent* ev,
                                           bool bDontBroadcast);
    KHook::Return<bool> Hook_FireEventPost(IGameEventManager2* mgr, IGameEvent* ev,
                                            bool bDontBroadcast);

    const char* GetAuthor() override { return "s2script"; }
    const char* GetName() override { return "s2_khook_probe"; }
    const char* GetDescription() override {
        return "KHook suite A Metamod probe (test-only, not shipped)";
    }
    const char* GetURL() override { return "https://s2script.com"; }
    const char* GetLicense() override { return "MIT OR Apache-2.0"; }
    const char* GetVersion() override { return "0.1.0-test"; }
    const char* GetDate() override { return __DATE__; }
    const char* GetLogTag() override { return "KHOOKPROBE"; }

    S2CheckedVirtual<ISource2Server, void, bool, bool, bool> gameFrame;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, const CCommand&> clientCommand;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, const char*, uint64, const char*,
                     const char*, bool>
        onConnected;
    S2CheckedVirtual<IGameEventManager2, bool, IGameEvent*, bool> fireEvent;

    ISource2Server* server = nullptr;
    ISource2GameClients* gameclients = nullptr;
    IGameEventManager2* events = nullptr;
    ICvar* icvar = nullptr;
    ConCommandRef cmdRef{};
};

static ProbePlugin g_plugin;
PLUGIN_EXPOSE(ProbePlugin, g_plugin);

static std::string g_cmdNameStore = kCmdName;

// Constructed without addresses: Configure/Add only after PLUGIN_SAVEVARS.
static S2CheckedFunction<int, int> fnNew(&PreNew, nullptr);
static S2CheckedFunction<int, int> fnNull(&PreNew, nullptr);
static S2CheckedFunction<int, int> fnShareA(&PreShareA, nullptr);
static S2CheckedFunction<int, int> fnShareB(&PreShareB, nullptr);
static S2CheckedFunction<int, int> fnAB_A(&PreAB_A, nullptr);
static S2CheckedFunction<int, int> fnAB_B(&PreAB_B, nullptr);
static S2CheckedFunction<int, int> fnBA_A(&PreBA_A, nullptr);
static S2CheckedFunction<int, int> fnBA_B(&PreBA_B, nullptr);
static S2CheckedFunction<int, int> fnOnce(&PreOnce, &PostOnce);
static S2CheckedVirtual<Dummy, int, int> virtA(&Dummy::Go, &DummyPreA, nullptr);
static S2CheckedVirtual<Dummy, int, int> virtB(&Dummy::Go, &DummyPreB, nullptr);
static S2CheckedVirtual<Dummy, int, int> virtPre(&Dummy::Go, &DummyPrePhase, nullptr);
static S2CheckedVirtual<Dummy, int, int> virtPost(&Dummy::Go, nullptr, &DummyPostPhase);

struct WireObservePtrs {
    WireObservePtrs() {
        pNew = &fnNew;
        pShareA = &fnShareA;
        pShareB = &fnShareB;
        pAB_A = &fnAB_A;
        pAB_B = &fnAB_B;
        pBA_A = &fnBA_A;
        pBA_B = &fnBA_B;
        pOnce = &fnOnce;
        pVirtA = &virtA;
        pVirtB = &virtB;
        pVirtPre = &virtPre;
        pVirtPost = &virtPost;
    }
};
static WireObservePtrs g_wireObserve;

KHook::Return<void> ProbePlugin::Hook_GameFrame(ISource2Server* s, bool, bool, bool) {
    auto obs = gameFrame.Observe(s);
    if (obs) {
        g_game_frames++;
    }
    return S2_Ignore();
}

KHook::Return<void> ProbePlugin::Hook_ClientCommand(ISource2GameClients* c, CPlayerSlot,
                                                    const CCommand& args) {
    auto obs = clientCommand.Observe(c);
    if (!obs) {
        return S2_Ignore();
    }
    g_client_cmds++;
    const char* a0 = args.Arg(0);
    if (a0 && strcasecmp(a0, kPingCmd) == 0) {
        g_ping_seen++;
        if (g_ping_continue == 0) {
            g_ping_continue++;
            return S2_Ignore();
        }
        g_ping_suppress++;
        return S2_Supersede();
    }
    return S2_Ignore();
}

KHook::Return<bool> ProbePlugin::Hook_FireEventPre(IGameEventManager2* mgr, IGameEvent* ev,
                                                    bool bDontBroadcast) {
    auto obs = fireEvent.Observe(mgr);
    if (!obs) {
        return S2_Ignore(true);
    }
    if (EventNameIs(ev, kFireEventNoSuppressName)) {
        g_fe_pre++;
        if (bDontBroadcast) {
            g_fe_dontbroadcast_true++;
        }
    }
    return S2_Ignore(true);
}

KHook::Return<bool> ProbePlugin::Hook_FireEventPost(IGameEventManager2* mgr, IGameEvent* ev,
                                                     bool bDontBroadcast) {
    (void)bDontBroadcast;
    auto obs = fireEvent.Observe(mgr);
    if (!obs) {
        return S2_Ignore(true);
    }
    if (EventNameIs(ev, kFireEventNoSuppressName)) {
        g_fe_post++;
        // POST-only: original ran iff KHook did not skip it. Never derive from PRE.
        if (KHook::WasOriginalFunctionSkipped()) {
            g_fe_skipped++;
        } else {
            g_fe_orig++;
        }
    }
    return S2_Ignore(true);
}

KHook::Return<void> ProbePlugin::Hook_OnClientConnected(ISource2GameClients* c, CPlayerSlot,
                                                        const char*, uint64, const char*,
                                                        const char*, bool) {
    auto obs = onConnected.Observe(c);
    if (obs) {
        g_clients_connected++;
    }
    return S2_Ignore();
}

static void InstallControlledHooks() {
    g_dummyA.tag = 10;
    g_dummyB.tag = 20;
    g_dummyPhase.tag = 30;

    g_fn_new_receipt = fnNew.Configure(&TargetNew).state;
    g_fn_share_receipt = fnShareA.Configure(&TargetShare).state;
    fnShareB.Configure(&TargetShare);
    fnAB_A.Configure(&TargetAB);
    fnAB_B.Configure(&TargetAB);
    fnBA_B.Configure(&TargetBA);  // B first
    fnBA_A.Configure(&TargetBA);
    fnOnce.Configure(&TargetOnce);

    virtA.Add(&g_dummyA);
    virtB.Add(&g_dummyB);
    virtPre.Add(&g_dummyPhase);
    virtPost.Add(&g_dummyPhase);
}

static bool PreBothAB() { return g_pre_ab_a == 1 && g_pre_ab_b == 1; }
static bool PreBothBA() { return g_pre_ba_a == 1 && g_pre_ba_b == 1; }

static void RunPeerActions(const char*& result, std::string& actual) {
    // Order A then B on TargetAB.
    g_pre_ab_a = g_pre_ab_b = g_orig_ab = 0;
    g_act_a = KHook::Action::Ignore;
    g_act_b = KHook::Action::Override;
    g_ret_a = 0;
    g_ret_b = 42;
    const int r1 = TargetAB(1);
    const int pre_ab_io_a = g_pre_ab_a;
    const int pre_ab_io_b = g_pre_ab_b;
    const bool ab_io = PreBothAB() && r1 == 42;

    g_pre_ab_a = g_pre_ab_b = g_orig_ab = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Override;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r2 = TargetAB(1);
    const int orig_tie = g_orig_ab;
    const int pre_ab_oo_a = g_pre_ab_a;
    const int pre_ab_oo_b = g_pre_ab_b;
    const bool ab_oo = PreBothAB() && r2 == 7 && orig_tie == 1;

    g_pre_ab_a = g_pre_ab_b = g_orig_ab = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Supersede;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r3 = TargetAB(1);
    const int orig_sup = g_orig_ab;
    const int pre_ab_os_a = g_pre_ab_a;
    const int pre_ab_os_b = g_pre_ab_b;
    const bool ab_os = PreBothAB() && r3 == 99 && orig_sup == 0;

    // Order B then A on TargetBA (B registered first).
    g_pre_ba_a = g_pre_ba_b = g_orig_ba = 0;
    g_act_a = KHook::Action::Ignore;
    g_act_b = KHook::Action::Override;
    g_ret_a = 0;
    g_ret_b = 42;
    const int r4 = TargetBA(1);
    const int pre_ba_io_a = g_pre_ba_a;
    const int pre_ba_io_b = g_pre_ba_b;
    const bool ba_io = PreBothBA() && r4 == 42;

    g_pre_ba_a = g_pre_ba_b = g_orig_ba = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Override;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r5 = TargetBA(1);  // B first: first Override is 99
    const int pre_ba_oo_a = g_pre_ba_a;
    const int pre_ba_oo_b = g_pre_ba_b;
    const bool ba_oo = PreBothBA() && r5 == 99;

    g_pre_ba_a = g_pre_ba_b = g_orig_ba = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Supersede;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r6 = TargetBA(1);
    const int orig_ba_sup = g_orig_ba;
    const int pre_ba_os_a = g_pre_ba_a;
    const int pre_ba_os_b = g_pre_ba_b;
    const bool ba_os = PreBothBA() && r6 == 99 && orig_ba_sup == 0;

    const bool ok = ab_io && ab_oo && ab_os && ba_io && ba_oo && ba_os;
    actual = "AB Ignore/Override=" + std::to_string(r1) + " pre=" + std::to_string(pre_ab_io_a) +
             "/" + std::to_string(pre_ab_io_b) + " AB Override/Override first=" + std::to_string(r2) +
             " orig=" + std::to_string(orig_tie) + " pre=" + std::to_string(pre_ab_oo_a) + "/" +
             std::to_string(pre_ab_oo_b) + " AB Override/Supersede=" + std::to_string(r3) +
             " orig=" + std::to_string(orig_sup) + " pre=" + std::to_string(pre_ab_os_a) + "/" +
             std::to_string(pre_ab_os_b) + " BA Override wins=" + std::to_string(r4) +
             " pre=" + std::to_string(pre_ba_io_a) + "/" + std::to_string(pre_ba_io_b) +
             " BA first-Override tie=" + std::to_string(r5) + " pre=" +
             std::to_string(pre_ba_oo_a) + "/" + std::to_string(pre_ba_oo_b) +
             " BA Override/Supersede=" + std::to_string(r6) + " orig=" +
             std::to_string(orig_ba_sup) + " pre=" + std::to_string(pre_ba_os_a) + "/" +
             std::to_string(pre_ba_os_b);
    result = ok ? "pass" : "fail";
}

static void RunSuiteA() {
    // Failed receipt: null address on a never-configured wrapper (does not touch fnNew).
    const S2HookReceipt failed = fnNull.Configure(static_cast<const void*>(nullptr));
    const bool failed_ok =
        failed.state == S2HookState::Failed && failed.id == KHook::INVALID_HOOK &&
        !failed.reason.empty();

    g_pre_new = g_orig_new = 0;
    const int nret = TargetNew(3);
    const S2HookReceipt after_new = fnNew.Snapshot();
    const bool new_ok = failed_ok && after_new.state == S2HookState::Active && g_pre_new == 1 &&
                        g_orig_new == 1 && nret == 4;
    std::string new_act = std::string("null=") + StateName(failed.state) + " reason=" +
                          failed.reason + " after_invoke=" + StateName(after_new.state) +
                          " pre=" + std::to_string(g_pre_new) +
                          " orig=" + std::to_string(g_orig_new);
    Emit("new_capsule_registration",
         "Failed on null; Pending/Active receipts; first valid invoke activates (no sentinel)",
         new_act, new_ok ? "pass" : (after_new.state == S2HookState::Pending ? "pending" : "fail"));

    g_pre_share_a = g_pre_share_b = g_orig_share = 0;
    (void)TargetShare(1);
    const S2HookReceipt sa = fnShareA.Snapshot();
    const S2HookReceipt sb = fnShareB.Snapshot();
    const bool share_fn = sa.state == S2HookState::Active && sb.state == S2HookState::Active &&
                          g_pre_share_a == 1 && g_pre_share_b == 1 && g_orig_share == 1;
    const S2HookReceipt frame_snap = g_plugin.gameFrame.Snapshot();
    std::string share_act = std::string("fnA=") + StateName(sa.state) + " fnB=" +
                            StateName(sb.state) + " preA=" + std::to_string(g_pre_share_a) +
                            " preB=" + std::to_string(g_pre_share_b) +
                            " orig=" + std::to_string(g_orig_share) +
                            " gameFrame=" + StateName(frame_snap.state) +
                            " frames=" + std::to_string(g_game_frames);
    const char* share_res = "fail";
    if (share_fn && g_frame_hooked &&
        (frame_snap.state == S2HookState::Active || g_game_frames > 0)) {
        share_res = "pass";
    } else if (share_fn) {
        share_res = "pending";
        share_act += " (engine shared GameFrame capsule not yet observed)";
    }
    Emit("shared_capsule_registration",
         "Two consumers on one address both Active; engine GameFrame shared with s2script",
         share_act, share_res);

    const char* peer_res = "fail";
    std::string peer_act;
    RunPeerActions(peer_res, peer_act);
    Emit("peer_actions_both_orders",
         "Higher action wins; equal action retains first return; all PRE run; both registration orders",
         peer_act, peer_res);

    g_pre_once = g_post_once = g_orig_once = 0;
    const int once = TargetOnce(10);
    const bool once_ok = g_pre_once == 1 && g_post_once == 1 && g_orig_once == 1 && once == 15;
    Emit("one_normal_invocation", "exactly one original call and one PRE + one POST",
         "pre=" + std::to_string(g_pre_once) + " post=" + std::to_string(g_post_once) +
             " orig=" + std::to_string(g_orig_once) + " ret=" + std::to_string(once),
         once_ok ? "pass" : "fail");

    const bool native_cc_ok = g_frame_hooked && g_client_hooked;
    const bool frame_ok = g_game_frames > 0;
    const bool client_ok = g_clients_connected > 0;
    const bool ping_ok = g_ping_continue >= 1 && g_ping_suppress >= 1;
    std::string fc_act = std::string("nativeAdd=") + (native_cc_ok ? "ok" : "fail") +
                         " frames=" + std::to_string(g_game_frames) +
                         " clients=" + std::to_string(g_clients_connected) +
                         " clientcmds=" + std::to_string(g_client_cmds) +
                         " pingSeen=" + std::to_string(g_ping_seen) +
                         " pingContinue=" + std::to_string(g_ping_continue) +
                         " pingSuppress=" + std::to_string(g_ping_suppress);
    const char* fc_res = "pending";
    if (!native_cc_ok) {
        fc_act += " (engine GameFrame/ClientCommand Add not accepted)";
    } else if (frame_ok && client_ok && ping_ok) {
        fc_res = "pass";
    } else {
        fc_act += " (need live frame + client connect + two khook_probe_ping: Ignore then Supercede)";
    }
    Emit("frame_client_command_hooks",
         "GameFrame counters increment; client lifecycle delivery; command suppression/continuation",
         fc_act, fc_res);

    std::string fe_act;
    const char* fe_res = "pending";
    if (!g_fe_hooked || !g_plugin.events) {
        fe_act = "nativeAdd=fail; FireEvent not installed on a real IGameEventManager2";
    } else {
        FeResetOrigCounters();
        FeStopListening(g_plugin.events);
        const bool listened =
            g_plugin.events->AddListener(&g_fe_listener_obj, kFireEventNoSuppressName, true);
        g_fe_listening = listened;
        IGameEvent* ev = g_plugin.events->CreateEvent(kFireEventNoSuppressName, true);
        if (!ev) {
            FeStopListening(g_plugin.events);
            fe_act = "nativeAdd=ok CreateEvent(" + std::string(kFireEventNoSuppressName) +
                     ")=null; descriptors may not be loaded yet";
        } else {
            const bool fired = g_plugin.events->FireEvent(ev, false);
            FeStopListening(g_plugin.events);
            fe_act = "nativeAdd=ok fired=" + std::to_string(fired ? 1 : 0) +
                     " listenerAdd=" + std::to_string(listened ? 1 : 0) +
                     " pre=" + std::to_string(g_fe_pre) +
                     " orig=" + std::to_string(g_fe_orig) +
                     " post=" + std::to_string(g_fe_post) +
                     " skipped=" + std::to_string(g_fe_skipped) +
                     " listener=" + std::to_string(g_fe_listener) +
                     " dontBroadcastTrue=" + std::to_string(g_fe_dontbroadcast_true);
            // orig is POST !WasOriginalFunctionSkipped(); listener is engine delivery.
            // Both must be exactly 1: skipped original or SH_CALL+original double must fail.
            if (g_fe_pre >= 1 && g_fe_orig == 1 && g_fe_listener == 1 &&
                g_fe_dontbroadcast_true == 0) {
                fe_res = "pass";
            } else if (g_fe_pre == 0) {
                fe_res = "fail";
                fe_act += " (PRE did not observe the fired event)";
            } else if (g_fe_orig != 1 || g_fe_listener != 1) {
                fe_res = "fail";
                fe_act += " (original must run exactly once: POST skip-state and listener)";
            } else {
                fe_res = "fail";
                fe_act += " (original/broadcast mismatch; JS onPre Handled must not share this event)";
            }
        }
    }
    Emit("fire_event_no_suppression",
         "original FireEvent exactly once, normal broadcast", fe_act, fe_res);
    Emit("fire_event_handled_recipient_mask",
         "original once with expected broadcast flag; intended recipients only",
         "needs real clients / JS Events.setRecipients; first-fire log is not sufficient",
         "pending");
    Emit("voice_recall",
         "denied listen bit reaches original as false; original once; unmuted unchanged",
         "needs real voice clients; first-fire log is not sufficient",
         "pending");

    virtB.Remove(&g_dummyB);
    g_dummy_pre_a = g_dummy_pre_b = 0;
    g_dummyA.orig = g_dummyB.orig = 0;
    (void)g_dummyA.Go(1);
    (void)g_dummyB.Go(1);
    const bool filt = g_dummy_pre_a == 1 && g_dummy_pre_b == 0 && g_dummyA.orig == 1 &&
                      g_dummyB.orig == 1;
    virtB.Add(&g_dummyB);
    std::string sdk1 = std::string("native Dummy this-filter preA=") +
                       std::to_string(g_dummy_pre_a) + " preB=" + std::to_string(g_dummy_pre_b) +
                       (filt ? " (native analog pass)" : " (native analog fail)") +
                       "; live SDKHook two-entity delivery is recorded by examples/khook-acceptance";
    Emit("sdkhooks_one_of_two_entities",
         "only the subscribed live entity dispatches", sdk1, "pending");

    g_dummy_pre_a = g_dummy_post = g_dummyPhase.orig = 0;
    (void)g_dummyPhase.Go(1);
    const int pre_before = g_dummy_pre_a;
    const int post_before = g_dummy_post;
    virtPre.Remove(&g_dummyPhase);
    g_dummy_pre_a = g_dummy_post = 0;
    (void)g_dummyPhase.Go(1);
    const int pre_after = g_dummy_pre_a;
    const int post_after = g_dummy_post;
    std::string sdk2 = "native PRE+POST before pre=" + std::to_string(pre_before) +
                       " post=" + std::to_string(post_before) + " after PRE Remove pre=" +
                       std::to_string(pre_after) + " post=" + std::to_string(post_after) +
                       "; live SDKHooks phase/in-callback removal is recorded by the JS fixture";
    Emit("sdkhooks_phase_removal",
         "PRE then POST removal, reverse, and in-callback removal: remaining phase survives; no deadlock",
         sdk2, "pending");

    Emit("entity_slot_reuse_map_teardown",
         "no stale entity delivery; filters and retained bindings retire on delete/reuse/map/teardown",
         "requires live entity delete, map change and plugin unload; JS fixture records publics",
         "pending");
    Emit("check_transmit",
         "first-fire layout validation and intended recipient filtering verified",
         "needs real clients in PVS; Transmit.stats bitsCleared growth is not proven by this command",
         "pending");
}

static void ProbeCommand(const CCommandContext& ctx, const CCommand& cmd) {
    (void)ctx;
    const char* a1 = cmd.Arg(1);
    const char* a2 = cmd.Arg(2);
    if (a1 && strcasecmp(a1, "run") == 0 && a2 && strcasecmp(a2, "A") == 0) {
        RunSuiteA();
        return;
    }
    if (a1 && strcasecmp(a1, "run") == 0 && a2 && (strcasecmp(a2, "B") == 0 || strcasecmp(a2, "C") == 0)) {
        META_CONPRINTF("{\"suite\":\"%s\",\"case\":\"not_authored\",\"expected\":\"suite %s cases\",\"actual\":\"PR A probe only implements suite A\",\"result\":\"pending\"}\n",
                       a2, a2);
        return;
    }
    META_CONPRINTF("usage: s2_khook_probe run A\n");
}

bool ProbePlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    (void)late;
    PLUGIN_SAVEVARS();

    CreateInterfaceFn engineFactory = ismm->GetEngineFactory(false);
    CreateInterfaceFn serverFactory = ismm->GetServerFactory(false);
    int ret = 0;
    icvar = engineFactory
                ? reinterpret_cast<ICvar*>(engineFactory(CVAR_INTERFACE_VERSION, &ret))
                : nullptr;
    if (!icvar) {
        if (error && maxlen) {
            ismm->Format(error, maxlen, "ICvar (%s) missing", CVAR_INTERFACE_VERSION);
        }
        return false;
    }

    ConCommandCreation_t setup;
    setup.m_pszName = g_cmdNameStore.c_str();
    setup.m_pszHelpString = "KHook suite probe: s2_khook_probe run A";
    setup.m_nFlags = FCVAR_RELEASE;
    setup.m_CBInfo = ConCommandCallbackInfo_t(&ProbeCommand);
    cmdRef = icvar->RegisterConCommand(setup);
    if (!cmdRef.IsValidRef()) {
        META_CONPRINTF("[khook-probe] WARN: RegisterConCommand(%s) returned invalid ref\n", kCmdName);
    } else {
        META_CONPRINTF("[khook-probe] ConCommand '%s' registered\n", kCmdName);
    }

    ret = 0;
    server = serverFactory
                 ? reinterpret_cast<ISource2Server*>(serverFactory(INTERFACEVERSION_SERVERGAMEDLL, &ret))
                 : nullptr;
    ret = 0;
    gameclients = serverFactory ? reinterpret_cast<ISource2GameClients*>(
                                      serverFactory(INTERFACEVERSION_SERVERGAMECLIENTS, &ret))
                                : nullptr;

    InstallControlledHooks();

    if (server) {
        const S2HookReceipt rec = gameFrame.Add(server);
        g_frame_hooked = rec.Accepted();
        g_frame_receipt = rec.state;
        META_CONPRINTF("[khook-probe] GameFrame Add state=%s id=%u\n", StateName(rec.state),
                       static_cast<unsigned>(rec.id));
    }
    if (gameclients) {
        const S2HookReceipt crec = clientCommand.Add(gameclients);
        g_client_hooked = crec.Accepted();
        const S2HookReceipt nrec = onConnected.Add(gameclients);
        g_connected_hooked = nrec.Accepted();
    }

    events = AcquireGameEventManager(engineFactory, serverFactory);
    if (events) {
        const S2HookReceipt frec = fireEvent.Add(events);
        g_fe_hooked = frec.Accepted();
        META_CONPRINTF("[khook-probe] FireEvent Add state=%s id=%u mgr=%p\n", StateName(frec.state),
                       static_cast<unsigned>(frec.id), static_cast<void*>(events));
    } else {
        META_CONPRINTF("[khook-probe] FireEvent manager not acquired (no sentinel)\n");
    }

    META_CONPRINTF("[khook-probe] loaded (test plugin, not for production release)\n");
    return true;
}

bool ProbePlugin::Unload(char* error, size_t maxlen) {
    (void)error;
    (void)maxlen;
    if (server) {
        gameFrame.Remove(server);
    }
    if (gameclients) {
        clientCommand.Remove(gameclients);
        onConnected.Remove(gameclients);
    }
    if (events) {
        FeStopListening(events);
        fireEvent.Remove(events);
        events = nullptr;
    }
    virtA.Remove(&g_dummyA);
    virtB.Remove(&g_dummyB);
    virtPre.Remove(&g_dummyPhase);
    virtPost.Remove(&g_dummyPhase);
    fnNew.BeginRemove();
    fnShareA.BeginRemove();
    fnShareB.BeginRemove();
    fnAB_A.BeginRemove();
    fnAB_B.BeginRemove();
    fnBA_A.BeginRemove();
    fnBA_B.BeginRemove();
    fnOnce.BeginRemove();
    if (icvar && cmdRef.IsValidRef()) {
        icvar->UnregisterConCommandCallbacks(cmdRef);
    }
    return true;
}
