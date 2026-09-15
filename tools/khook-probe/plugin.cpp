// s2_khook_probe — Metamod test plugin. NEVER shipped in the production release.
//
// Second KHook consumer against the same pin. Hooks controlled native functions
// (and dummy virtuals) with valid objects; optionally shares engine capsules with
// s2script. Protocol: s2_khook_probe prepare|collect|report <run_id>.
#include <ISmmPlugin.h>
#include "khook_map.h"
#include "sigscan.h"
#include "acceptance_observer.h"

#include <eiface.h>
#include <icvar.h>
#include <convar.h>
#include <playerslot.h>
#include <igameevents.h>
#include <iservernetworkable.h>
#include <inetchannel.h>
#include <engine/igameeventsystem.h>
#include <entity2/entitysystem.h>
#include <entity2/entityinstance.h>
#include "vtable.h"

#include <cstdlib>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <strings.h>
#include <string>
#include <vector>
#include <link.h>

PLUGIN_GLOBALVARS();

#ifndef S2_KHOOK_SOURCE_REVISION
#define S2_KHOOK_SOURCE_REVISION "unknown"
#endif

#if defined(__GNUC__)
#define S2_NOINLINE __attribute__((noinline, noclone))
#else
#define S2_NOINLINE
#endif

static const char* kCmdName = "s2_khook_probe";
static const char* kTokenCmd = "s2_khook_probe_token";
static const char* kCcEntryCmd = "s2khook_cc_entry";
static const char* kContinueToken = "s2khook-continue";
static const char* kHandledToken = "s2khook-handled";
static const char* kCtrlMissingToken = "s2khook-ctrl-missing";
static const char* kCtrlFlipContinueToken = "s2khook-ctrl-flip-continue";
static const char* kCtrlFlipHandledToken = "s2khook-ctrl-flip-handled";
static const char* kAcceptRunCvar = "s2_khook_accept_run";

#ifndef INTERFACEVERSION_GAMEEVENTSMANAGER2
#define INTERFACEVERSION_GAMEEVENTSMANAGER2 "GAMEEVENTSMANAGER002"
#endif

static const char* kFireEventNoSuppressName = "player_activate";

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

static std::string JsonBool(bool v) { return v ? "true" : "false"; }

static void PushPending(const char* cse, const char* sub, const char* producer, const std::string& expected,
                        const char* why);
static void R6GameFrame();
static void R6PrepareEntities();
static void CollectR6();
static void PushR6Pending();
static bool R6ResolveEngine(CreateInterfaceFn engineFactory, CreateInterfaceFn serverFactory);
static void R6InstallEngineHooks();
static void R6BeginRetirement();

struct StoredRec {
    std::string cse;
    std::string sub;
    std::string producer;
    std::string result;
    std::string expected;
    std::string actual;
    std::string evidence;
};

static std::string g_run_id;
static std::string g_source_revision = S2_KHOOK_SOURCE_REVISION;
static bool g_run_bound = false;
static bool g_collected = false;
static bool g_probe_retiring = false;
static std::vector<StoredRec> g_stored;
static std::string g_emit_run;
static std::string g_command_route = "unobserved";
static std::string g_command_route_note =
    "continue/handled original is DispatchConCommand + probe callback on s2_khook_probe_token; "
    "ClientCommand evidence is the unrecognized name s2khook_cc_entry. Never label "
    "DispatchConCommand-only as ClientCommand evidence. RCON is a control operation.";

static std::string RecordLine(const StoredRec& r) {
    return std::string("{\"schema\":1,\"suite\":\"A\",\"run_id\":\"") + JsonEscape(g_emit_run.c_str()) +
           "\",\"source_revision\":\"" + JsonEscape(g_source_revision.c_str()) + "\",\"case\":\"" +
           JsonEscape(r.cse.c_str()) + "\",\"subcheck\":\"" + JsonEscape(r.sub.c_str()) +
           "\",\"producer\":\"" + JsonEscape(r.producer.c_str()) + "\",\"result\":\"" +
           JsonEscape(r.result.c_str()) + "\",\"expected\":" + r.expected + ",\"actual\":" + r.actual +
           ",\"evidence\":\"" + JsonEscape(r.evidence.c_str()) + "\"}";
}

static void PushRec(const char* cse, const char* sub, const char* producer, const char* result,
                     const std::string& expected, const std::string& actual, const char* evidence) {
    StoredRec r;
    r.cse = cse;
    r.sub = sub;
    r.producer = producer;
    r.result = result;
    r.expected = expected;
    r.actual = actual;
    r.evidence = evidence ? evidence : "";
    g_stored.push_back(std::move(r));
}

static void PrintRecs(const std::vector<StoredRec>& recs) {
    for (const StoredRec& r : recs) {
        const std::string line = RecordLine(r);
        META_CONPRINTF("%s\n", line.c_str());
    }
}

static void PrintStored() { PrintRecs(g_stored); }

static void PushInvalidOwned(const char* requested);

static std::vector<StoredRec> InvalidOwned(const char* requested) {
    std::vector<StoredRec> keep = std::move(g_stored);
    g_stored.clear();
    PushInvalidOwned(requested);
    std::vector<StoredRec> invalid = std::move(g_stored);
    g_stored = std::move(keep);
    return invalid;
}

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

S2_NOINLINE int TargetNew(int x) {
    volatile int uniq = 11;
    g_orig_new++;
    return x + 1 + (uniq - 11);
}
S2_NOINLINE int TargetShare(int x) {
    volatile int uniq = 22;
    g_orig_share++;
    return x + 2 + (uniq - 22);
}
S2_NOINLINE int TargetAB(int x) {
    volatile int uniq = 33;
    g_orig_ab++;
    return x + 3 + (uniq - 33);
}
S2_NOINLINE int TargetBA(int x) {
    volatile int uniq = 44;
    g_orig_ba++;
    return x + 4 + (uniq - 44);
}
S2_NOINLINE int TargetOnce(int x) {
    volatile int uniq = 55;
    g_orig_once++;
    return x + 5 + (uniq - 55);
}

struct Dummy {
    int tag = 0;
    int orig = 0;
    virtual int Go(int x);
    virtual ~Dummy() = default;
};

S2_NOINLINE int Dummy::Go(int x) {
    orig++;
    return x + tag;
}

S2_NOINLINE Dummy* SelectDummy(Dummy* p) {
    volatile Dummy* sink = p;
    return const_cast<Dummy*>(sink);
}

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
    (void)obs;
    g_pre_new++;
    return S2_Ignore(x);
}
static KHook::Return<int> PreShareA(int x) {
    auto obs = pShareA ? pShareA->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_share_a++;
    return S2_Ignore(x);
}
static KHook::Return<int> PreShareB(int x) {
    auto obs = pShareB ? pShareB->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_share_b++;
    return S2_Ignore(x);
}
static KHook::Return<int> PreAB_A(int x) {
    auto obs = pAB_A ? pAB_A->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_ab_a++;
    return ActionRet(g_act_a, g_ret_a);
}
static KHook::Return<int> PreAB_B(int x) {
    auto obs = pAB_B ? pAB_B->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_ab_b++;
    return ActionRet(g_act_b, g_ret_b);
}
static KHook::Return<int> PreBA_A(int x) {
    auto obs = pBA_A ? pBA_A->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_ba_a++;
    return ActionRet(g_act_a, g_ret_a);
}
static KHook::Return<int> PreBA_B(int x) {
    auto obs = pBA_B ? pBA_B->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_ba_b++;
    return ActionRet(g_act_b, g_ret_b);
}
static KHook::Return<int> PreOnce(int x) {
    auto obs = pOnce ? pOnce->Observe() : S2HookObserve{};
    (void)obs;
    g_pre_once++;
    return S2_Ignore(x);
}
static KHook::Return<int> PostOnce(int x) {
    auto obs = pOnce ? pOnce->Observe() : S2HookObserve{};
    (void)obs;
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

static int g_game_frames = 0;
static int g_clients_connected = 0;
static int g_fe_dontbroadcast_true = 0;
static bool g_frame_hooked = false;
static bool g_client_hooked = false;
static bool g_connected_hooked = false;
static bool g_fe_hooked = false;
static bool g_dispatch_hooked = false;
static S2HookState g_frame_receipt = S2HookState::Failed;
static S2HookState g_fn_new_receipt = S2HookState::Failed;
static S2HookState g_fn_share_receipt = S2HookState::Failed;

static int g_cc_entry_pre = 0, g_cc_entry_post = 0;
static int g_dc_cont_pre = 0, g_dc_cont_post = 0, g_dc_cont_skip = 0;
static int g_dc_hand_pre = 0, g_dc_hand_post = 0, g_dc_hand_skip = 0;
static int g_engine_continue = 0, g_engine_handled = 0;
static int g_dc_ctrl_missing_pre = 0, g_engine_ctrl_missing = 0;
static int g_dc_flip_c_pre = 0, g_dc_flip_c_skip = 0, g_engine_flip_c = 0;
static int g_dc_flip_h_pre = 0, g_dc_flip_h_skip = 0, g_engine_flip_h = 0;
static int g_last_slot = -1;
static uint64 g_last_xuid = 0;
static int g_slot_gen[64];
static uint64 g_slot_xuid[64];
static s2khook::FireEventObservation g_fe_obs;

static bool CmdNamed(const CCommand& args, const char* name) {
    const char* a0 = args.Arg(0);
    return a0 && strcasecmp(a0, name) == 0;
}

static bool CmdToken(const CCommand& args, const char* token) {
    if (!token || !CmdNamed(args, kTokenCmd)) {
        return false;
    }
    const char* a1 = args.Arg(1);
    return a1 && std::strcmp(a1, token) == 0;
}

static bool CmdCcEntry(const CCommand& args) { return CmdNamed(args, kCcEntryCmd); }

static bool RealClientSlot(const CCommandContext& ctx) { return ctx.GetPlayerSlot().Get() >= 0; }

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

struct ProbeFireEventListener : public IGameEventListener2 {
    void FireGameEvent(IGameEvent* ev) override { s2khook::ObserveFireEventListener(ev); }
};
static ProbeFireEventListener g_fe_listener_obj;
static bool g_fe_listening = false;

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
          clientCommand(&ISource2GameClients::ClientCommand, this, &ProbePlugin::Hook_ClientCommand,
                        &ProbePlugin::Hook_ClientCommandPost),
          dispatchConCommand(&ICvar::DispatchConCommand, this, &ProbePlugin::Hook_DispatchConCommand,
                             &ProbePlugin::Hook_DispatchConCommandPost),
          onConnected(&ISource2GameClients::OnClientConnected, this,
                      &ProbePlugin::Hook_OnClientConnected, nullptr),
          fireEvent(&IGameEventManager2::FireEvent, this, &ProbePlugin::Hook_FireEventPre,
                    &ProbePlugin::Hook_FireEventPost) {}

    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    bool Unload(char* error, size_t maxlen) override;

    KHook::Return<void> Hook_GameFrame(ISource2Server* server, bool simulating, bool first, bool last);
    KHook::Return<void> Hook_ClientCommand(ISource2GameClients* clients, CPlayerSlot slot,
                                           const CCommand& args);
    KHook::Return<void> Hook_ClientCommandPost(ISource2GameClients* clients, CPlayerSlot slot,
                                               const CCommand& args);
    KHook::Return<void> Hook_DispatchConCommand(ICvar* cvar, ConCommandRef cmd, const CCommandContext& ctx,
                                              const CCommand& args);
    KHook::Return<void> Hook_DispatchConCommandPost(ICvar* cvar, ConCommandRef cmd,
                                                 const CCommandContext& ctx, const CCommand& args);
    KHook::Return<void> Hook_OnClientConnected(ISource2GameClients* clients, CPlayerSlot slot,
                                               const char* name, uint64 xuid, const char* netid,
                                               const char* addr, bool fake);
    KHook::Return<bool> Hook_FireEventPre(IGameEventManager2* mgr, IGameEvent* ev, bool bDontBroadcast);
    KHook::Return<bool> Hook_FireEventPost(IGameEventManager2* mgr, IGameEvent* ev, bool bDontBroadcast);

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
    S2CheckedVirtual<ICvar, void, ConCommandRef, const CCommandContext&, const CCommand&> dispatchConCommand;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, const char*, uint64, const char*,
                     const char*, bool>
        onConnected;
    S2CheckedVirtual<IGameEventManager2, bool, IGameEvent*, bool> fireEvent;

    ISource2Server* server = nullptr;
    ISource2GameClients* gameclients = nullptr;
    IGameEventManager2* events = nullptr;
    ICvar* icvar = nullptr;
    ConCommandRef cmdRef{};
    ConCommandRef tokenRef{};
};

static ProbePlugin g_plugin;
PLUGIN_EXPOSE(ProbePlugin, g_plugin);

static std::string g_cmdNameStore = kCmdName;
static std::string g_tokenNameStore = kTokenCmd;
static std::string g_revCvarName = "s2_khook_source_revision";
static std::string g_revCvarHelp = "khook-probe baked source revision (not a live pass)";
static std::string g_revCvarDefault = S2_KHOOK_SOURCE_REVISION;

static void RegisterRevisionCvar(ICvar* icvar) {
    if (!icvar) {
        return;
    }
    ConVarCreation_t setup;
    setup.m_pszName = g_revCvarName.c_str();
    setup.m_pszHelpString = g_revCvarHelp.c_str();
    setup.m_nFlags = FCVAR_RELEASE;
    setup.m_valueInfo = ConVarValueInfo_t(EConVarType_String);
    setup.m_valueInfo.SetDefaultValue<const char*>(g_revCvarDefault.c_str());
    ConVarRef ref;
    ConVarData* data = nullptr;
    icvar->RegisterConVar(setup, 0, &ref, &data);
    if (ref.IsValidRef()) {
        META_CONPRINTF("[khook-probe] ConVar '%s'=%s\n", g_revCvarName.c_str(), g_revCvarDefault.c_str());
    }
}

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
        R6GameFrame();
    }
    return S2_Ignore();
}

KHook::Return<void> ProbePlugin::Hook_ClientCommand(ISource2GameClients* c, CPlayerSlot,
                                                    const CCommand& args) {
    auto obs = clientCommand.Observe(c);
    if (!obs) {
        return S2_Ignore();
    }
    if (CmdCcEntry(args)) {
        g_cc_entry_pre++;
    }
    return S2_Ignore();
}

KHook::Return<void> ProbePlugin::Hook_ClientCommandPost(ISource2GameClients* c, CPlayerSlot,
                                                       const CCommand& args) {
    auto obs = clientCommand.Observe(c);
    if (!obs) {
        return S2_Ignore();
    }
    if (CmdCcEntry(args)) {
        g_cc_entry_post++;
    }
    return S2_Ignore();
}

KHook::Return<void> ProbePlugin::Hook_DispatchConCommand(ICvar* cvar, ConCommandRef,
                                                       const CCommandContext& ctx,
                                                       const CCommand& args) {
    auto obs = dispatchConCommand.Observe(cvar);
    if (!obs) {
        return S2_Ignore();
    }
    const bool real_client = RealClientSlot(ctx);
    if (CmdToken(args, kContinueToken)) {
        if (real_client) {
            g_dc_cont_pre++;
        }
    } else if (CmdToken(args, kHandledToken)) {
        if (real_client) {
            g_dc_hand_pre++;
        }
    } else if (CmdToken(args, kCtrlMissingToken)) {
        g_dc_ctrl_missing_pre++;
    } else if (CmdToken(args, kCtrlFlipContinueToken)) {
        g_dc_flip_c_pre++;
    } else if (CmdToken(args, kCtrlFlipHandledToken)) {
        g_dc_flip_h_pre++;
    }
    return S2_Ignore();
}

KHook::Return<void> ProbePlugin::Hook_DispatchConCommandPost(ICvar* cvar, ConCommandRef,
                                                            const CCommandContext& ctx,
                                                            const CCommand& args) {
    auto obs = dispatchConCommand.Observe(cvar);
    if (!obs) {
        return S2_Ignore();
    }
    const bool skipped = KHook::WasOriginalFunctionSkipped();
    const bool real_client = RealClientSlot(ctx);
    if (CmdToken(args, kContinueToken)) {
        if (real_client) {
            g_dc_cont_post++;
            if (skipped) {
                g_dc_cont_skip++;
            }
        }
    } else if (CmdToken(args, kHandledToken)) {
        if (real_client) {
            g_dc_hand_post++;
            if (skipped) {
                g_dc_hand_skip++;
            }
        }
    } else if (CmdToken(args, kCtrlFlipContinueToken)) {
        if (skipped) {
            g_dc_flip_c_skip++;
        }
    } else if (CmdToken(args, kCtrlFlipHandledToken)) {
        if (skipped) {
            g_dc_flip_h_skip++;
        }
    }
    return S2_Ignore();
}

KHook::Return<bool> ProbePlugin::Hook_FireEventPre(IGameEventManager2* mgr, IGameEvent* ev,
                                                    bool bDontBroadcast) {
    auto obs = fireEvent.Observe(mgr);
    if (!obs) {
        return S2_Ignore(true);
    }
    s2khook::ObserveFireEventPre(ev);
    if (s2khook::CurrentScope() && s2khook::CurrentScope()->event_ptr() == static_cast<void*>(ev) &&
        bDontBroadcast) {
        g_fe_dontbroadcast_true++;
    }
    return S2_Ignore(true);
}

KHook::Return<bool> ProbePlugin::Hook_FireEventPost(IGameEventManager2* mgr, IGameEvent* ev, bool) {
    auto obs = fireEvent.Observe(mgr);
    if (!obs) {
        return S2_Ignore(true);
    }
    s2khook::ObserveFireEventPost(ev, KHook::WasOriginalFunctionSkipped());
    return S2_Ignore(true);
}

KHook::Return<void> ProbePlugin::Hook_OnClientConnected(ISource2GameClients* c, CPlayerSlot slot,
                                                        const char*, uint64 xuid, const char*,
                                                        const char*, bool) {
    auto obs = onConnected.Observe(c);
    if (obs) {
        g_clients_connected++;
        const int s = slot.Get();
        if (s >= 0 && s < 64) {
            g_slot_gen[s]++;
            g_slot_xuid[s] = xuid;
            g_last_slot = s;
            g_last_xuid = xuid;
        }
    }
    return S2_Ignore();
}

static void ProbeTokenCommand(const CCommandContext& ctx, const CCommand& cmd) {
    S2HookDispatchGuard guard;
    if (!guard) {
        return;
    }
    const bool real_client = RealClientSlot(ctx);
    if (CmdToken(cmd, kContinueToken)) {
        if (real_client) {
            g_engine_continue++;
        }
    } else if (CmdToken(cmd, kHandledToken)) {
        if (real_client) {
            g_engine_handled++;
        }
    } else if (CmdToken(cmd, kCtrlMissingToken)) {
        g_engine_ctrl_missing++;
    } else if (CmdToken(cmd, kCtrlFlipContinueToken)) {
        g_engine_flip_c++;
    } else if (CmdToken(cmd, kCtrlFlipHandledToken)) {
        g_engine_flip_h++;
    }
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
    fnBA_B.Configure(&TargetBA);
    fnBA_A.Configure(&TargetBA);
    fnOnce.Configure(&TargetOnce);

    virtA.Add(&g_dummyA);
    virtB.Add(&g_dummyB);
    virtPre.Add(&g_dummyPhase);
    virtPost.Add(&g_dummyPhase);
}

static bool PreBothAB() { return g_pre_ab_a == 1 && g_pre_ab_b == 1; }
static bool PreBothBA() { return g_pre_ba_a == 1 && g_pre_ba_b == 1; }

static void RunPeerActions(bool& ok, std::string& actual) {
    g_pre_ab_a = g_pre_ab_b = g_orig_ab = 0;
    g_act_a = KHook::Action::Ignore;
    g_act_b = KHook::Action::Override;
    g_ret_a = 0;
    g_ret_b = 42;
    const int r1 = TargetAB(1);
    const bool ab_io = PreBothAB() && r1 == 42;

    g_pre_ab_a = g_pre_ab_b = g_orig_ab = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Override;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r2 = TargetAB(1);
    const int orig_tie = g_orig_ab;
    const bool ab_oo = PreBothAB() && r2 == 7 && orig_tie == 1;

    g_pre_ab_a = g_pre_ab_b = g_orig_ab = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Supersede;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r3 = TargetAB(1);
    const int orig_sup = g_orig_ab;
    const bool ab_os = PreBothAB() && r3 == 99 && orig_sup == 0;

    g_pre_ba_a = g_pre_ba_b = g_orig_ba = 0;
    g_act_a = KHook::Action::Ignore;
    g_act_b = KHook::Action::Override;
    g_ret_a = 0;
    g_ret_b = 42;
    const int r4 = TargetBA(1);
    const bool ba_io = PreBothBA() && r4 == 42;

    g_pre_ba_a = g_pre_ba_b = g_orig_ba = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Override;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r5 = TargetBA(1);
    const bool ba_oo = PreBothBA() && r5 == 99;

    g_pre_ba_a = g_pre_ba_b = g_orig_ba = 0;
    g_act_a = KHook::Action::Override;
    g_act_b = KHook::Action::Supersede;
    g_ret_a = 7;
    g_ret_b = 99;
    const int r6 = TargetBA(1);
    const int orig_ba_sup = g_orig_ba;
    const bool ba_os = PreBothBA() && r6 == 99 && orig_ba_sup == 0;

    ok = ab_io && ab_oo && ab_os && ba_io && ba_oo && ba_os;
    actual = "{\"ab_io\":" + JsonBool(ab_io) + ",\"ab_oo\":" + JsonBool(ab_oo) + ",\"ab_os\":" +
             JsonBool(ab_os) + ",\"ba_io\":" + JsonBool(ba_io) + ",\"ba_oo\":" + JsonBool(ba_oo) +
             ",\"ba_os\":" + JsonBool(ba_os) + "}";
}

static bool JsAcceptPresent() {
    if (!g_plugin.icvar) {
        return false;
    }
    ConVarRef ref = g_plugin.icvar->FindConVar(kAcceptRunCvar, false);
    return ref.IsValidRef();
}

// --- R6: SDKHooks entity/phase/reuse/map/voice/transmit/mask (not Dummy substitutes) ---

static const char kTouchPat[] = "55 48 89 E5 41 57 41 56 41 55 49 89 F5 41 54 53 48 89 FB 48 83 EC 58";
static const char kCreateEntPat[] = "48 8D 05 ? ? ? ? 55 48 89 FA";
static const char kDispatchSpawnPat[] = "48 85 FF 74 ? 55 48 89 E5 41 55 41 54 49 89 FC";
static const char kUtilRemovePat[] = "48 89 FE 48 85 FF 74 ? 48 8D 05 ? ? ? ? 48";
static const char kMaskEventName[] = "player_changename";
static const int kGameEntitySystemOff = 80;
static const int kCtiClientOff = 576;
static const int kReuseMaxAttempts = 64;
static const int kPhaseMaxInvokes = 8;

using CreateEntityByNameFn = CEntityInstance* (*)(const char* className, int forceEdictIndex);
using DispatchSpawnFn = void (*)(CEntityInstance* self, void* pEntityKeyValues);
using UtilRemoveFn = void (*)(CEntityInstance* self);
using TouchFn = void (*)(CEntityInstance* self, CEntityInstance* other);

static CreateEntityByNameFn g_create_ent = nullptr;
static DispatchSpawnFn g_dispatch_spawn = nullptr;
static UtilRemoveFn g_util_remove = nullptr;
static int g_touch_slot = -1;
static bool g_touch_resolved = false;
static void* g_game_resource = nullptr;
static IVEngineServer2* g_engine2 = nullptr;
static ISource2GameEntities* g_game_ents = nullptr;
static IGameEventSystem* g_event_sys = nullptr;

static const uint8_t* ProbeSig(const char* pat) {
    const std::vector<int> p = s2sig::ParsePattern(pat);
    if (p.empty()) {
        return nullptr;
    }
    const ModText mt = ProbeFindModuleText("libserver.so");
    if (!mt.text || mt.size == 0) {
        return nullptr;
    }
    if (s2sig::CountPattern(mt.text, mt.size, p, 2) != 1) {
        return nullptr;
    }
    const int64_t off = s2sig::FindPattern(mt.text, mt.size, p);
    if (off < 0) {
        return nullptr;
    }
    return mt.text + off;
}

static bool ProbeInServerText(const void* fn) {
    const ModText mt = ProbeFindModuleText("libserver.so");
    if (!mt.text || !fn) {
        return false;
    }
    const uint8_t* p = static_cast<const uint8_t*>(fn);
    return p >= mt.text && p < mt.text + mt.size;
}

static int ProbeCvarInt(const char* name, int fallback = -1) {
    if (!g_plugin.icvar || !name) {
        return fallback;
    }
    ConVarRef ref = g_plugin.icvar->FindConVar(name, false);
    if (!ref.IsValidRef()) {
        return fallback;
    }
    ConVarData* data = g_plugin.icvar->GetConVarData(ref);
    if (!data) {
        return fallback;
    }
    const size_t voff = sizeof(ConVarData) - sizeof(CVValue_t) * MAX_SPLITSCREEN_CLIENTS;
    CVValue_t* v = reinterpret_cast<CVValue_t*>(reinterpret_cast<char*>(data) + voff);
    switch (data->GetType()) {
        case EConVarType_Bool:
            return v->m_bValue ? 1 : 0;
        case EConVarType_Int16:
            return static_cast<int>(v->m_i16Value);
        case EConVarType_Int32:
            return v->m_i32Value;
        case EConVarType_Float32:
            return static_cast<int>(v->m_fl32Value);
        case EConVarType_String: {
            const char* sv = nullptr;
            std::memcpy(&sv, &v->m_StringValue, sizeof(sv));
            return sv && sv[0] ? std::atoi(sv) : fallback;
        }
        default:
            return fallback;
    }
}

static std::string ProbeCvarStr(const char* name) {
    if (!g_plugin.icvar || !name) {
        return "";
    }
    ConVarRef ref = g_plugin.icvar->FindConVar(name, false);
    if (!ref.IsValidRef()) {
        return "";
    }
    ConVarData* data = g_plugin.icvar->GetConVarData(ref);
    if (!data) {
        return "";
    }
    const size_t voff = sizeof(ConVarData) - sizeof(CVValue_t) * MAX_SPLITSCREEN_CLIENTS;
    CVValue_t* v = reinterpret_cast<CVValue_t*>(reinterpret_cast<char*>(data) + voff);
    if (data->GetType() == EConVarType_String) {
        const char* sv = nullptr;
        std::memcpy(&sv, &v->m_StringValue, sizeof(sv));
        return sv ? sv : "";
    }
    char buf[64];
    if (data->GetType() == EConVarType_Int32) {
        std::snprintf(buf, sizeof(buf), "%d", v->m_i32Value);
        return buf;
    }
    return "";
}

static bool ProbeSetCvarInt(const char* name, int value) {
    if (!g_plugin.icvar || !name) {
        return false;
    }
    ConVarRef ref = g_plugin.icvar->FindConVar(name, false);
    if (!ref.IsValidRef()) {
        return false;
    }
    ConVarData* data = g_plugin.icvar->GetConVarData(ref);
    if (!data) {
        return false;
    }
    const size_t voff = sizeof(ConVarData) - sizeof(CVValue_t) * MAX_SPLITSCREEN_CLIENTS;
    CVValue_t* v = reinterpret_cast<CVValue_t*>(reinterpret_cast<char*>(data) + voff);
    if (data->GetType() == EConVarType_Int32) {
        v->m_i32Value = value;
        return true;
    }
    if (data->GetType() == EConVarType_Int16) {
        v->m_i16Value = static_cast<int16_t>(value);
        return true;
    }
    if (data->GetType() == EConVarType_Bool) {
        v->m_bValue = value != 0;
        return true;
    }
    return false;
}

static CEntityInstance* EntByIndex(int idx) {
    if (!g_game_resource || idx < 0 || idx >= MAX_TOTAL_ENTITIES) {
        return nullptr;
    }
    CGameEntitySystem* es = *reinterpret_cast<CGameEntitySystem**>(
        reinterpret_cast<uint8_t*>(g_game_resource) + kGameEntitySystemOff);
    if (!es) {
        return nullptr;
    }
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[idx / MAX_ENTITIES_IN_LIST];
    if (!chunk_base) {
        return nullptr;
    }
    CEntityIdentity* id = &chunk_base[idx % MAX_ENTITIES_IN_LIST];
    if (id->m_flags & EF_IS_INVALID_EHANDLE) {
        return nullptr;
    }
    return id->m_pInstance;
}

static bool EntIndexSerial(CEntityInstance* ent, int* index, int* serial) {
    if (!ent) {
        return false;
    }
    const CEntityHandle h = ent->GetRefEHandle();
    if (!h.IsValid()) {
        return false;
    }
    if (index) {
        *index = h.GetEntryIndex();
    }
    if (serial) {
        *serial = h.GetSerialNumber();
    }
    return true;
}

static KHook::Return<void> Hook_R6TouchPre(CEntityInstance* self, CEntityInstance* other);
static KHook::Return<void> Hook_R6TouchPost(CEntityInstance* self, CEntityInstance* other);
static KHook::Return<bool> Hook_R6SetClientListening(IVEngineServer2* engine, CPlayerSlot receiver,
                                                     CPlayerSlot sender, bool listen);
static KHook::Return<void> Hook_R6CheckTransmit(ISource2GameEntities* ents, CCheckTransmitInfo** infos,
                                                int nInfo, CBitVec<16384>&, CBitVec<16384>&,
                                                const Entity2Networkable_t**, const uint16*, int);
static KHook::Return<void> Hook_R6PostEvent(IGameEventSystem* sys, CSplitScreenSlot slot, bool localOnly,
                                            int nClientCount, const uint64* clients,
                                            INetworkMessageInternal* pEvent, const CNetMessage* pData,
                                            unsigned long nSize, NetChannelBufType_t bufType);

static S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkTouchPre(&Hook_R6TouchPre, nullptr);
static S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkTouchPost(nullptr, &Hook_R6TouchPost);
static S2CheckedVirtual<IVEngineServer2, bool, CPlayerSlot, CPlayerSlot, bool> g_hkListen(
    &IVEngineServer2::SetClientListening, &Hook_R6SetClientListening, nullptr);
static S2CheckedVirtual<ISource2GameEntities, void, CCheckTransmitInfo**, int, CBitVec<16384>&, CBitVec<16384>&,
                        const Entity2Networkable_t**, const uint16*, int>
    g_hkTransmit(&ISource2GameEntities::CheckTransmit, nullptr, &Hook_R6CheckTransmit);

using PostEvent8Mfp = void (IGameEventSystem::*)(CSplitScreenSlot, bool, int, const uint64*,
                                                 INetworkMessageInternal*, const CNetMessage*, unsigned long,
                                                 NetChannelBufType_t);
static S2CheckedVirtual<IGameEventSystem, void, CSplitScreenSlot, bool, int, const uint64*,
                        INetworkMessageInternal*, const CNetMessage*, unsigned long, NetChannelBufType_t>
    g_hkPostEvent(static_cast<PostEvent8Mfp>(&IGameEventSystem::PostEventAbstract), nullptr, &Hook_R6PostEvent);

static CEntityInstance* g_nat_a = nullptr;
static CEntityInstance* g_nat_b = nullptr;
static CEntityInstance* g_nat_phase = nullptr;
static CEntityInstance* g_nat_reuse = nullptr;
static CEntityInstance* g_nat_reuse_new = nullptr;
static CEntityInstance* g_touch_other = nullptr;
static bool g_spawn_a_ok = false;
static bool g_spawn_b_ok = false;
static bool g_phase_pre_added = false;
static bool g_phase_post_added = false;
static bool g_filter_a_added = false;
static bool g_self_unsub_armed = false;
static int g_phase_stage = 0;
static int g_phase_pre = 0;
static int g_phase_post = 0;
static int g_phase_orig = 0;
static int g_first_pre = 0;
static int g_first_post = 0;
static int g_first_orig = 0;
static int g_second_pre = 0;
static int g_second_post = 0;
static int g_second_orig = 0;
static int g_r6_frames = 0;
static int g_idx_a = -1;
static int g_idx_b = -1;
static int g_idx_phase = -1;
static int g_idx_reuse = -1;
static int g_idx_reuse_new = -1;
static bool g_filter_invoked = false;
static bool g_js_filter_invoked = false;
static bool g_native_phase_driven = false;
static int g_js_phase_invoked_for_stage = -1;
static int g_js_stage3_invokes = 0;
static bool g_reuse_invoked = false;
static bool g_js_reuse_invoked = false;
static bool g_post_map_invoke_attempted = false;
static bool g_fresh_reload_invoked = false;
static bool g_map_ptrs_invalidated = false;
static bool g_tx_first_fire_on_ent = false;
static int g_old_index = -1;
static int g_old_serial = -1;
static bool g_identity_persisted = false;
static int g_reuse_attempts = 0;
static int g_reuse_new_serial = -1;
static bool g_reuse_done = false;
static int g_stale_deliveries = 0;
static int g_pre_map_touch = 0;
static int g_post_map_touch = 0;
static bool g_map_ended = false;
static bool g_saw_js_unloaded = false;
static int g_js_instance_at_prepare = 0;
static bool g_js_reappeared = false;
static bool g_js_present_at_prepare = false;
static int g_voice_orig = 0;
static int g_voice_allowed_true = 0;
static int g_voice_denied_false = 0;
static bool g_tx_layout_ok = false;
static int g_tx_a_set = 0;
static int g_tx_b_set = 0;
static int g_tx_a_clear = 0;
static int g_tx_b_clear = 0;
static bool g_mask_subset_posts = false;
static bool g_mask_excluded_posts = false;
static bool g_mask_all_suppressed = false;
static bool g_mask_call_orig_super = false;
static uint64_t g_mask_seen = 0;
static int g_mask_post_count = 0;
static int g_mask_fire_stage = 0;
static s2khook::FireEventObservation g_fe_mask_obs;
static bool g_listen_hooked = false;
static bool g_transmit_hooked = false;
static bool g_postevent_hooked = false;

struct PhaseSnap {
    int pre = 0;
    int post = 0;
    int orig = 0;
};
static PhaseSnap g_snap_sub;
static PhaseSnap g_snap_rm_pre;
static PhaseSnap g_snap_rm_post;
static PhaseSnap g_snap_final;
static bool g_have_sub = false;
static bool g_have_rm_pre = false;
static bool g_have_rm_post = false;
static bool g_have_self = false;
static bool g_have_final = false;

static const char* kNeedAdapter =
    "need live engine: Touch invoke through the actual SDKHooks adapter "
    "(CTriggerPush::Touch PRE+POST); Dummy Virtuals are supporting evidence only";
static const char* kNeedCreate =
    "need live engine: UTIL_CreateEntityByName + DispatchSpawn of a valid CTriggerPush";
static const char* kNeedThree =
    "need three real clients (speaker, allowed-listener, denied-listener); speaker talks in "
    "allowed phase, denied phase, then unmuted phase";
static const char* kNeedPvs =
    "need a networked visible entity in both clients' PVS (not logic_relay); client A allowed, "
    "client B denied, then restore visibility";
static const char* kNeedMask =
    "need at least two real clients; send an observable event to a strict subset, then suppress "
    "for all. Sending to every human is not a mask test";
static const char* kNeedUnload =
    "need s2script/probe native unload/reload with the peer still loaded (R2 pending/retry); "
    "missing restoration must stay visible";
static const char* kNeedMap =
    "need operator changelevel while this run stays prepared; then collect again";
static const char* kNeedMapInvoke =
    "need post-map Touch invoke via live EntByIndex after changelevel; "
    "zero callbacks without an invoke is not a pass";
static const char* kNeedReuse =
    "slot reuse not achieved within bounded attempts; not a false pass";

static void R6RemoveTouch(CEntityInstance* ent, bool pre, bool post) {
    if (!ent) {
        return;
    }
    if (pre) {
        g_hkTouchPre.Remove(ent);
    }
    if (post) {
        g_hkTouchPost.Remove(ent);
    }
}

static void R6CleanupOwned() {
    R6RemoveTouch(g_nat_a, true, true);
    R6RemoveTouch(g_nat_b, true, true);
    R6RemoveTouch(g_nat_phase, true, true);
    R6RemoveTouch(g_nat_reuse, true, true);
    R6RemoveTouch(g_nat_reuse_new, true, true);
    g_filter_a_added = false;
    g_phase_pre_added = false;
    g_phase_post_added = false;
    if (g_util_remove) {
        if (g_nat_a) {
            g_util_remove(g_nat_a);
        }
        if (g_nat_b) {
            g_util_remove(g_nat_b);
        }
        if (g_nat_phase) {
            g_util_remove(g_nat_phase);
        }
        if (g_nat_reuse) {
            g_util_remove(g_nat_reuse);
        }
        if (g_nat_reuse_new) {
            g_util_remove(g_nat_reuse_new);
        }
    }
    g_nat_a = g_nat_b = g_nat_phase = g_nat_reuse = g_nat_reuse_new = g_touch_other = nullptr;
}

static void R6ResetCounters() {
    g_spawn_a_ok = g_spawn_b_ok = false;
    g_self_unsub_armed = false;
    g_phase_stage = 0;
    g_phase_pre = g_phase_post = g_phase_orig = 0;
    g_first_pre = g_first_post = g_first_orig = 0;
    g_second_pre = g_second_post = g_second_orig = 0;
    g_r6_frames = 0;
    g_idx_a = g_idx_b = g_idx_phase = g_idx_reuse = g_idx_reuse_new = -1;
    g_filter_invoked = false;
    g_js_filter_invoked = false;
    g_native_phase_driven = false;
    g_js_phase_invoked_for_stage = -1;
    g_js_stage3_invokes = 0;
    g_reuse_invoked = false;
    g_js_reuse_invoked = false;
    g_post_map_invoke_attempted = false;
    g_fresh_reload_invoked = false;
    g_map_ptrs_invalidated = false;
    g_tx_first_fire_on_ent = false;
    g_old_index = g_old_serial = -1;
    g_identity_persisted = false;
    g_reuse_attempts = 0;
    g_reuse_new_serial = -1;
    g_reuse_done = false;
    g_stale_deliveries = 0;
    g_pre_map_touch = g_post_map_touch = 0;
    g_map_ended = false;
    g_saw_js_unloaded = false;
    g_js_reappeared = false;
    g_js_instance_at_prepare = 0;
    g_js_present_at_prepare = false;
    g_voice_orig = 0;
    g_voice_allowed_true = 0;
    g_voice_denied_false = 0;
    g_tx_layout_ok = false;
    g_tx_a_set = g_tx_b_set = g_tx_a_clear = g_tx_b_clear = 0;
    ProbeSetCvarInt("s2_khook_accept_post_map_invoke", 0);
    g_mask_subset_posts = g_mask_excluded_posts = g_mask_all_suppressed = false;
    g_mask_call_orig_super = false;
    g_mask_seen = 0;
    g_mask_post_count = 0;
    g_mask_fire_stage = 0;
    g_fe_mask_obs = s2khook::FireEventObservation{};
    g_have_sub = g_have_rm_pre = g_have_rm_post = g_have_self = g_have_final = false;
    g_snap_sub = g_snap_rm_pre = g_snap_rm_post = g_snap_final = PhaseSnap{};
}

static CEntityInstance* R6SpawnPush() {
    if (!g_create_ent || !g_dispatch_spawn) {
        return nullptr;
    }
    CEntityInstance* ent = g_create_ent("trigger_push", -1);
    if (!ent) {
        return nullptr;
    }
    g_dispatch_spawn(ent, nullptr);
    return ent;
}

static bool R6AddTouch(CEntityInstance* ent, bool pre, bool post) {
    if (!ent || g_touch_slot < 0) {
        return false;
    }
    bool ok = true;
    if (pre) {
        const S2HookReceipt rec = g_hkTouchPre.Add(ent);
        ok = ok && rec.Accepted();
    }
    if (post) {
        const S2HookReceipt rec = g_hkTouchPost.Add(ent);
        ok = ok && rec.Accepted();
    }
    return ok;
}

static void R6InvokeTouch(CEntityInstance* self) {
    if (!self || g_touch_slot < 0) {
        return;
    }
    void** vt = *reinterpret_cast<void***>(self);
    if (!vt) {
        return;
    }
    auto fn = reinterpret_cast<TouchFn>(vt[g_touch_slot]);
    if (!fn) {
        return;
    }
    CEntityInstance* other = g_touch_other ? g_touch_other : self;
    fn(self, other);
    // Original runs unless a PRE Supercedes. The probe always Ignore, so the vtable
    // call is the original witness even after POST (or both phases) are removed.
    if (self == g_nat_phase) {
        g_phase_orig++;
    }
}

static void R6TryReuse() {
    if (!g_identity_persisted || g_old_index < 0 || !g_util_remove || !g_create_ent) {
        return;
    }
    if (g_nat_reuse) {
        g_util_remove(g_nat_reuse);
        g_nat_reuse = nullptr;
    }
    for (int i = 0; i < kReuseMaxAttempts; i++) {
        g_reuse_attempts = i + 1;
        CEntityInstance* n = R6SpawnPush();
        if (!n) {
            continue;
        }
        int idx = -1, ser = -1;
        if (!EntIndexSerial(n, &idx, &ser)) {
            g_util_remove(n);
            continue;
        }
        if (idx == g_old_index && ser != g_old_serial) {
            g_nat_reuse_new = n;
            g_idx_reuse_new = idx;
            g_reuse_new_serial = ser;
            g_reuse_done = true;
            return;
        }
        g_util_remove(n);
    }
}

static void R6PrepareEntities() {
    R6CleanupOwned();
    R6ResetCounters();
    g_nat_a = R6SpawnPush();
    g_nat_b = R6SpawnPush();
    g_nat_phase = R6SpawnPush();
    g_nat_reuse = R6SpawnPush();
    g_touch_other = g_nat_b ? g_nat_b : g_nat_a;
    g_spawn_a_ok = g_nat_a != nullptr;
    g_spawn_b_ok = g_nat_b != nullptr;
    if (g_spawn_a_ok && g_touch_slot >= 0) {
        g_filter_a_added = R6AddTouch(g_nat_a, true, false);
    }
    if (g_nat_phase && g_touch_slot >= 0) {
        g_phase_pre_added = R6AddTouch(g_nat_phase, true, false);
        g_phase_post_added = R6AddTouch(g_nat_phase, false, true);
        g_phase_stage = (g_phase_pre_added && g_phase_post_added) ? 0 : -1;
    } else {
        g_phase_stage = -1;
    }
    if (g_nat_reuse && EntIndexSerial(g_nat_reuse, &g_old_index, &g_old_serial)) {
        g_identity_persisted = true;
        R6AddTouch(g_nat_reuse, true, false);
    }
    EntIndexSerial(g_nat_a, &g_idx_a, nullptr);
    EntIndexSerial(g_nat_b, &g_idx_b, nullptr);
    EntIndexSerial(g_nat_phase, &g_idx_phase, nullptr);
    EntIndexSerial(g_nat_reuse, &g_idx_reuse, nullptr);
    g_js_present_at_prepare = JsAcceptPresent();
    g_js_instance_at_prepare = ProbeCvarInt("s2_khook_accept_instance", 0);
    R6TryReuse();
    if (g_nat_reuse_new) {
        EntIndexSerial(g_nat_reuse_new, &g_idx_reuse_new, nullptr);
    }
    META_CONPRINTF("[khook-probe] NEED_CLIENTS: voice_recall needs 3 clients (speaker/allowed/denied) "
                   "then unmute. check_transmit needs 2 clients in PVS of point_worldtext. "
                   "fire_event_handled_recipient_mask needs 2 clients for subset then all-suppressed.\n");
}

static void R6Snap(PhaseSnap* s) {
    s->pre = g_phase_pre;
    s->post = g_phase_post;
    s->orig = g_phase_orig;
    g_phase_pre = g_phase_post = g_phase_orig = 0;
}

static void R6AdvancePhase() {
    if (g_phase_stage < 0 || !g_nat_phase) {
        return;
    }
    if (g_phase_stage == 0) {
        if (g_phase_pre >= 1 && g_phase_post >= 1 && g_phase_orig >= 1) {
            R6Snap(&g_snap_sub);
            g_have_sub = true;
            g_hkTouchPre.Remove(g_nat_phase);
            g_phase_pre_added = false;
            g_phase_stage = 1;
        }
        return;
    }
    if (g_phase_stage == 1) {
        if (g_phase_pre == 0 && g_phase_post >= 1 && g_phase_orig >= 1) {
            R6Snap(&g_snap_rm_pre);
            g_have_rm_pre = true;
            g_phase_pre_added = R6AddTouch(g_nat_phase, true, false);
            g_hkTouchPost.Remove(g_nat_phase);
            g_phase_post_added = false;
            g_phase_stage = 2;
        }
        return;
    }
    if (g_phase_stage == 2) {
        if (g_phase_pre >= 1 && g_phase_post == 0 && g_phase_orig >= 1) {
            R6Snap(&g_snap_rm_post);
            g_have_rm_post = true;
            g_phase_post_added = R6AddTouch(g_nat_phase, false, true);
            g_self_unsub_armed = true;
            g_first_pre = g_first_post = g_first_orig = 0;
            g_second_pre = g_second_post = g_second_orig = 0;
            g_phase_stage = 3;
        }
        return;
    }
    if (g_phase_stage == 3) {
        if (g_first_pre == 0 && g_phase_pre >= 1) {
            g_first_pre = g_phase_pre;
            g_first_post = g_phase_post;
            g_first_orig = g_phase_orig;
            g_phase_pre = g_phase_post = g_phase_orig = 0;
            g_self_unsub_armed = false;
            return;
        }
        if (g_first_pre >= 1 && g_phase_pre == 0 && g_phase_post >= 1) {
            g_second_pre = g_phase_pre;
            g_second_post = g_phase_post;
            g_second_orig = g_phase_orig;
            g_have_self = true;
            g_hkTouchPre.Remove(g_nat_phase);
            g_hkTouchPost.Remove(g_nat_phase);
            g_phase_pre_added = g_phase_post_added = false;
            g_phase_pre = g_phase_post = g_phase_orig = 0;
            g_phase_stage = 4;
        }
        return;
    }
    if (g_phase_stage == 4) {
        if (g_phase_pre == 0 && g_phase_post == 0 && g_phase_orig >= 1) {
            R6Snap(&g_snap_final);
            g_have_final = true;
            g_phase_stage = 5;
        }
    }
}

static void R6FireMaskEvent() {
    if (!g_plugin.events) {
        return;
    }
    IGameEvent* ev = g_plugin.events->CreateEvent(kMaskEventName, true);
    if (!ev) {
        return;
    }
    ev->SetString("oldname", "s2khook-old");
    ev->SetString("newname", "s2khook-new");
    g_plugin.events->AddListener(&g_fe_listener_obj, kMaskEventName, true);
    {
        s2khook::FireEventInvocationScope scope(g_run_id.c_str(), "fire_event_handled_recipient_mask", ev);
        (void)g_plugin.events->FireEvent(ev, false);
        g_fe_mask_obs = scope.Copy();
    }
    g_plugin.events->RemoveListener(&g_fe_listener_obj);
    if (g_fe_mask_obs.automatic_skip_count >= 1 && g_fe_mask_obs.listener_count >= 1) {
        g_mask_call_orig_super = true;
    }
}

static void R6InvalidatePreMapPointers() {
    if (g_map_ptrs_invalidated) {
        return;
    }
    g_nat_a = nullptr;
    g_nat_b = nullptr;
    g_nat_phase = nullptr;
    g_nat_reuse = nullptr;
    g_nat_reuse_new = nullptr;
    g_touch_other = nullptr;
    g_map_ptrs_invalidated = true;
}

static void R6InvokeLiveIndex(int idx) {
    if (idx < 0) {
        return;
    }
    CEntityInstance* live = EntByIndex(idx);
    if (!live) {
        return;
    }
    g_post_map_invoke_attempted = true;
    ProbeSetCvarInt("s2_khook_accept_post_map_invoke", 1);
    R6InvokeTouch(live);
}

static void R6InvokePostMap() {
    R6InvalidatePreMapPointers();
    R6InvokeLiveIndex(g_idx_a);
    R6InvokeLiveIndex(g_idx_b);
    R6InvokeLiveIndex(g_idx_phase);
    R6InvokeLiveIndex(g_old_index);
    R6InvokeLiveIndex(g_idx_reuse);
    R6InvokeLiveIndex(g_idx_reuse_new);
    R6InvokeLiveIndex(ProbeCvarInt("s2_khook_accept_ent_a", -1));
    R6InvokeLiveIndex(ProbeCvarInt("s2_khook_accept_ent_b", -1));
    R6InvokeLiveIndex(ProbeCvarInt("s2_khook_accept_phase_ent", -1));
    R6InvokeLiveIndex(ProbeCvarInt("s2_khook_accept_reuse_ent", -1));
    R6InvokeLiveIndex(ProbeCvarInt("s2_khook_accept_reuse_new", -1));
}

static void R6DriveNativePhase() {
    if (g_native_phase_driven || !g_nat_phase || g_phase_stage < 0) {
        return;
    }
    int n = 0;
    while (g_phase_stage >= 0 && g_phase_stage <= 4 && n < kPhaseMaxInvokes) {
        const int before = g_phase_stage;
        R6InvokeTouch(g_nat_phase);
        n++;
        R6AdvancePhase();
        if (g_phase_stage == 3 && g_first_pre >= 1 && !g_have_self) {
            R6InvokeTouch(g_nat_phase);
            n++;
            R6AdvancePhase();
        }
        if (g_phase_stage == before) {
            break;
        }
    }
    g_native_phase_driven = true;
}

static void R6DriveJsFilterOnce() {
    if (g_js_filter_invoked) {
        return;
    }
    const int js_a = ProbeCvarInt("s2_khook_accept_ent_a", -1);
    const int js_b = ProbeCvarInt("s2_khook_accept_ent_b", -1);
    CEntityInstance* ja = js_a >= 0 ? EntByIndex(js_a) : nullptr;
    CEntityInstance* jb = js_b >= 0 ? EntByIndex(js_b) : nullptr;
    if (!ja || !jb) {
        return;
    }
    if (!(g_filter_invoked && ja == g_nat_a)) {
        R6InvokeTouch(ja);
    }
    if (!(g_filter_invoked && jb == g_nat_b)) {
        R6InvokeTouch(jb);
    }
    g_js_filter_invoked = true;
}

static void R6DriveJsPhase() {
    const int js_p = ProbeCvarInt("s2_khook_accept_phase_ent", -1);
    CEntityInstance* jp = js_p >= 0 ? EntByIndex(js_p) : nullptr;
    if (!jp) {
        return;
    }
    const int js_stage = ProbeCvarInt("s2_khook_accept_phase_stage", -1);
    if (js_stage < 0 || js_stage > 3) {
        return;
    }
    if (js_stage == 3) {
        if (g_js_stage3_invokes >= 2) {
            return;
        }
        R6InvokeTouch(jp);
        g_js_stage3_invokes++;
        return;
    }
    if (g_js_phase_invoked_for_stage == js_stage) {
        return;
    }
    R6InvokeTouch(jp);
    g_js_phase_invoked_for_stage = js_stage;
}

static void R6DriveReuseOnce() {
    if (!g_reuse_invoked && g_nat_reuse_new) {
        R6InvokeTouch(g_nat_reuse_new);
        g_reuse_invoked = true;
    }
    if (g_js_reuse_invoked) {
        return;
    }
    const int js_new = ProbeCvarInt("s2_khook_accept_reuse_new", -1);
    CEntityInstance* jn = js_new >= 0 ? EntByIndex(js_new) : nullptr;
    if (!jn) {
        return;
    }
    if (jn != g_nat_reuse_new) {
        R6InvokeTouch(jn);
    }
    g_js_reuse_invoked = true;
}

static void R6DriveFreshReloadOnce() {
    if (!g_js_reappeared || g_fresh_reload_invoked) {
        return;
    }
    const int js_a = ProbeCvarInt("s2_khook_accept_ent_a", -1);
    CEntityInstance* ja = js_a >= 0 ? EntByIndex(js_a) : nullptr;
    if (!ja) {
        return;
    }
    R6InvokeTouch(ja);
    g_fresh_reload_invoked = true;
}

static void R6GameFrame() {
    if (!g_run_bound) {
        return;
    }
    g_r6_frames++;
    if (ProbeCvarInt("s2_khook_accept_map_ended", 0) == 1) {
        g_map_ended = true;
    }
    if (ProbeCvarInt("s2_khook_accept_unloaded", 0) == 1) {
        g_saw_js_unloaded = true;
    }
    if (g_saw_js_unloaded && JsAcceptPresent()) {
        const int inst = ProbeCvarInt("s2_khook_accept_instance", 0);
        if (inst > g_js_instance_at_prepare) {
            g_js_reappeared = true;
        }
    }
    if (g_map_ended) {
        R6InvokePostMap();
    } else {
        if (!g_filter_invoked && g_nat_a && g_nat_b) {
            R6InvokeTouch(g_nat_a);
            R6InvokeTouch(g_nat_b);
            g_filter_invoked = true;
        }
        R6DriveJsFilterOnce();
        R6DriveNativePhase();
        R6DriveJsPhase();
        R6DriveReuseOnce();
        R6DriveFreshReloadOnce();
    }
    const std::string mode = ProbeCvarStr("s2_khook_accept_mask_mode");
    if (g_mask_fire_stage == 0 && mode == "subset") {
        R6FireMaskEvent();
        g_mask_fire_stage = 1;
    } else if (g_mask_fire_stage == 1 && mode == "all") {
        g_mask_seen = 0;
        g_mask_post_count = 0;
        R6FireMaskEvent();
        g_mask_fire_stage = 2;
    }
}

KHook::Return<void> Hook_R6TouchPre(CEntityInstance* self, CEntityInstance*) {
    auto obs = g_hkTouchPre.Observe(self);
    if (!obs) {
        return S2_Ignore();
    }
    if (g_map_ended) {
        g_post_map_touch++;
    } else {
        g_pre_map_touch++;
    }
    int idx = -1, ser = -1;
    EntIndexSerial(self, &idx, &ser);
    if (g_identity_persisted && idx == g_old_index && ser != g_old_serial) {
        g_stale_deliveries++;
    }
    if (self == g_nat_phase) {
        g_phase_pre++;
        if (g_self_unsub_armed) {
            g_hkTouchPre.Remove(self);
            g_phase_pre_added = false;
        }
    }
    return S2_Ignore();
}

KHook::Return<void> Hook_R6TouchPost(CEntityInstance* self, CEntityInstance*) {
    auto obs = g_hkTouchPost.Observe(self);
    if (!obs) {
        return S2_Ignore();
    }
    if (self == g_nat_phase) {
        g_phase_post++;
    }
    return S2_Ignore();
}

KHook::Return<bool> Hook_R6SetClientListening(IVEngineServer2* engine, CPlayerSlot receiver,
                                              CPlayerSlot sender, bool listen) {
    auto obs = g_hkListen.Observe(engine);
    if (!obs) {
        return S2_Ignore(listen);
    }
    g_voice_orig++;
    const int r = receiver.Get();
    const int s = sender.Get();
    const int speaker = ProbeCvarInt("s2_khook_accept_voice_speaker", -1);
    const int allowed = ProbeCvarInt("s2_khook_accept_voice_allowed", -1);
    const int denied = ProbeCvarInt("s2_khook_accept_voice_denied", -1);
    if (s == speaker && r == allowed && listen) {
        g_voice_allowed_true++;
    }
    if (s == speaker && r == denied && !listen) {
        g_voice_denied_false++;
    }
    return S2_Ignore(listen);
}

KHook::Return<void> Hook_R6CheckTransmit(ISource2GameEntities* ents, CCheckTransmitInfo** infos, int nInfo,
                                         CBitVec<16384>&, CBitVec<16384>&, const Entity2Networkable_t**,
                                         const uint16*, int) {
    auto obs = g_hkTransmit.Observe(ents);
    if (!obs) {
        return S2_Ignore();
    }
    if (!infos || nInfo <= 0) {
        return S2_Ignore();
    }
    const int tx = ProbeCvarInt("s2_khook_accept_tx_ent", -1);
    if (tx < 0 || tx >= 16384) {
        return S2_Ignore();
    }
    const int slot_a = ProbeCvarInt("s2_khook_accept_tx_a", -1);
    const int slot_b = ProbeCvarInt("s2_khook_accept_tx_b", -1);
    for (int i = 0; i < nInfo; i++) {
        uint8_t* raw = reinterpret_cast<uint8_t*>(infos[i]);
        if (!raw) {
            continue;
        }
        const CBitVec<16384>* bv = infos[i]->m_pTransmitEntity;
        if (!bv) {
            continue;
        }
        const int v = *reinterpret_cast<const int32_t*>(raw + kCtiClientOff);
        if (!g_tx_first_fire_on_ent) {
            g_tx_first_fire_on_ent = true;
            g_tx_layout_ok = (v >= 0 && v <= 128);
        }
        const bool bit = bv->IsBitSet(tx);
        if (slot_a >= 0 && v == slot_a) {
            if (bit) {
                g_tx_a_set++;
            } else {
                g_tx_a_clear++;
            }
        } else if (slot_b >= 0 && v == slot_b) {
            if (bit) {
                g_tx_b_set++;
            } else {
                g_tx_b_clear++;
            }
        }
    }
    return S2_Ignore();
}

KHook::Return<void> Hook_R6PostEvent(IGameEventSystem* sys, CSplitScreenSlot, bool, int nClientCount,
                                     const uint64* clients, INetworkMessageInternal* pEvent, const CNetMessage*,
                                     unsigned long, NetChannelBufType_t) {
    auto obs = g_hkPostEvent.Observe(sys);
    if (!obs) {
        return S2_Ignore();
    }
    if (!pEvent) {
        return S2_Ignore();
    }
    const char* ln = pEvent->GetUnscopedName();
    if (!ln || std::strncmp(ln, "CMsgSource1LegacyGameEvent", 26) != 0 || ln[26] == 'L') {
        return S2_Ignore();
    }
    const uint64_t who = (clients && nClientCount > 0) ? clients[0] : 0;
    g_mask_post_count++;
    g_mask_seen |= who;
    const bool skipped = KHook::WasOriginalFunctionSkipped();
    const int slot_a = ProbeCvarInt("s2_khook_accept_mask_a", -1);
    const int slot_b = ProbeCvarInt("s2_khook_accept_mask_b", -1);
    const uint64_t bit_a = slot_a >= 0 ? (1ULL << slot_a) : 0;
    const uint64_t bit_b = slot_b >= 0 ? (1ULL << slot_b) : 0;
    const std::string mode = ProbeCvarStr("s2_khook_accept_mask_mode");
    if (mode == "subset") {
        if ((who & bit_a) && !skipped) {
            g_mask_subset_posts = true;
        }
        if ((who & bit_b) && !skipped) {
            g_mask_excluded_posts = true;
        }
    } else if (mode == "all") {
        if (!skipped && ((who & bit_a) || (who & bit_b))) {
            g_mask_all_suppressed = false;
        } else if (skipped || who == 0) {
            g_mask_all_suppressed = true;
        }
    }
    return S2_Ignore();
}

static bool R6ResolveEngine(CreateInterfaceFn engineFactory, CreateInterfaceFn serverFactory) {
    const uint8_t* touch = ProbeSig(kTouchPat);
    g_create_ent = reinterpret_cast<CreateEntityByNameFn>(const_cast<uint8_t*>(ProbeSig(kCreateEntPat)));
    g_dispatch_spawn = reinterpret_cast<DispatchSpawnFn>(const_cast<uint8_t*>(ProbeSig(kDispatchSpawnPat)));
    g_util_remove = reinterpret_cast<UtilRemoveFn>(const_cast<uint8_t*>(ProbeSig(kUtilRemovePat)));
    void** vt = s2vtable::GetVTableByName("libserver.so", "CTriggerPush");
    const ModText mt = ProbeFindModuleText("libserver.so");
    if (touch && vt) {
        for (int i = 0; i < 512; i++) {
            void* orig = KHook::FindOriginalVirtual(vt, i);
            if (!ProbeInServerText(orig)) {
                break;
            }
            if (orig == touch) {
                g_touch_slot = i;
                g_hkTouchPre.Configure(i);
                g_hkTouchPost.Configure(i);
                g_touch_resolved = true;
                META_CONPRINTF("[khook-probe] Touch slot=%d (CTriggerPush SDKHooks adapter)\n", i);
                break;
            }
        }
    }
    int ret = 0;
    if (engineFactory) {
        g_engine2 = reinterpret_cast<IVEngineServer2*>(engineFactory(INTERFACEVERSION_VENGINESERVER, &ret));
        ret = 0;
        g_game_resource = engineFactory("GameResourceServiceServerV001", &ret);
        ret = 0;
        g_event_sys = reinterpret_cast<IGameEventSystem*>(engineFactory(GAMEEVENTSYSTEM_INTERFACE_VERSION, &ret));
    }
    ret = 0;
    if (serverFactory) {
        g_game_ents =
            reinterpret_cast<ISource2GameEntities*>(serverFactory(INTERFACEVERSION_SERVERGAMEENTS, &ret));
        if (!g_event_sys) {
            ret = 0;
            g_event_sys =
                reinterpret_cast<IGameEventSystem*>(serverFactory(GAMEEVENTSYSTEM_INTERFACE_VERSION, &ret));
        }
    }
    (void)mt;
    return g_touch_resolved;
}

static void R6InstallEngineHooks() {
    if (g_engine2) {
        const S2HookReceipt rec = g_hkListen.Add(g_engine2);
        g_listen_hooked = rec.Accepted();
        META_CONPRINTF("[khook-probe] SetClientListening Add state=%s\n", StateName(rec.state));
    }
    if (g_game_ents) {
        const S2HookReceipt rec = g_hkTransmit.Add(g_game_ents);
        g_transmit_hooked = rec.Accepted();
        META_CONPRINTF("[khook-probe] CheckTransmit Add state=%s\n", StateName(rec.state));
    }
    if (g_event_sys) {
        const S2HookReceipt rec = g_hkPostEvent.Add(g_event_sys);
        g_postevent_hooked = rec.Accepted();
        META_CONPRINTF("[khook-probe] PostEventAbstract Add state=%s\n", StateName(rec.state));
    }
}

static void R6BeginRetirement() {
    R6CleanupOwned();
    if (g_engine2) {
        g_hkListen.Remove(g_engine2);
    }
    if (g_game_ents) {
        g_hkTransmit.Remove(g_game_ents);
    }
    if (g_event_sys) {
        g_hkPostEvent.Remove(g_event_sys);
    }
    g_hkTouchPre.BeginRemove();
    g_hkTouchPost.BeginRemove();
    g_hkListen.BeginRemove();
    g_hkTransmit.BeginRemove();
    g_hkPostEvent.BeginRemove();
}

static void CollectR6() {
    if (g_spawn_a_ok) {
        PushRec("sdkhooks_one_of_two_entities", "native_spawn_a_ok", "native", "pass", "{\"spawned\":true}",
                "{\"spawned\":true}", "UTIL_CreateEntityByName+DispatchSpawn A");
    } else {
        PushPending("sdkhooks_one_of_two_entities", "native_spawn_a_ok", "native", "{\"spawned\":true}",
                    kNeedCreate);
    }
    if (g_spawn_b_ok) {
        PushRec("sdkhooks_one_of_two_entities", "native_spawn_b_ok", "native", "pass", "{\"spawned\":true}",
                "{\"spawned\":true}", "UTIL_CreateEntityByName+DispatchSpawn B");
    } else {
        PushPending("sdkhooks_one_of_two_entities", "native_spawn_b_ok", "native", "{\"spawned\":true}",
                    kNeedCreate);
    }

    const std::string sub_exp = "{\"pre\":1,\"post\":1,\"original\":1}";
    const std::string rmpre_exp = "{\"pre\":0,\"post\":1,\"original\":1}";
    const std::string rmpost_exp = "{\"pre\":1,\"post\":0,\"original\":1}";
    const std::string self_exp =
        "{\"first_pre\":1,\"first_post\":1,\"second_pre\":0,\"second_post\":1,\"original_first\":1,"
        "\"original_second\":1}";
    const std::string fin_exp = "{\"pre\":0,\"post\":0,\"original\":1}";
    if (g_have_sub && g_snap_sub.pre >= 1 && g_snap_sub.post >= 1 && g_snap_sub.orig >= 1) {
        PushRec("sdkhooks_phase_removal", "native_phase_subscribe_pre_post", "native", "pass", sub_exp, sub_exp,
                "PRE+POST+original after subscribe on CTriggerPush::Touch");
    } else {
        PushPending("sdkhooks_phase_removal", "native_phase_subscribe_pre_post", "native", sub_exp, kNeedAdapter);
    }
    if (g_have_rm_pre && g_snap_rm_pre.pre == 0 && g_snap_rm_pre.post >= 1 && g_snap_rm_pre.orig >= 1) {
        PushRec("sdkhooks_phase_removal", "native_phase_remove_pre", "native", "pass", rmpre_exp, rmpre_exp,
                "PRE removed, POST+original survive");
    } else {
        PushPending("sdkhooks_phase_removal", "native_phase_remove_pre", "native", rmpre_exp, kNeedAdapter);
    }
    if (g_have_rm_post && g_snap_rm_post.pre >= 1 && g_snap_rm_post.post == 0 && g_snap_rm_post.orig >= 1) {
        PushRec("sdkhooks_phase_removal", "native_phase_remove_post", "native", "pass", rmpost_exp, rmpost_exp,
                "POST removed, PRE+original survive");
    } else {
        PushPending("sdkhooks_phase_removal", "native_phase_remove_post", "native", rmpost_exp, kNeedAdapter);
    }
    if (g_have_self && g_first_pre >= 1 && g_second_pre == 0 && g_second_post >= 1 && g_first_orig >= 1 &&
        g_second_orig >= 1) {
        PushRec("sdkhooks_phase_removal", "native_phase_self_unsubscribe", "native", "pass", self_exp, self_exp,
                "PRE self-unsub; second invoke POST+original only");
    } else {
        PushPending("sdkhooks_phase_removal", "native_phase_self_unsubscribe", "native", self_exp, kNeedAdapter);
    }
    if (g_have_final && g_snap_final.pre == 0 && g_snap_final.post == 0 && g_snap_final.orig >= 1) {
        PushRec("sdkhooks_phase_removal", "native_phase_final_unsubscribe", "native", "pass", fin_exp, fin_exp,
                "both phases removed; original still runs");
    } else {
        PushPending("sdkhooks_phase_removal", "native_phase_final_unsubscribe", "native", fin_exp, kNeedAdapter);
    }

    if (g_identity_persisted && g_old_index >= 0) {
        const std::string exp = std::string("{\"persisted\":true,\"index\":") + std::to_string(g_old_index) +
                                ",\"serial\":" + std::to_string(g_old_serial) + "}";
        PushRec("entity_slot_reuse_map_teardown", "native_identity_persisted", "native", "pass", exp, exp,
                "index+serial stored outside the JS plugin (probe statics + cvars)");
    } else {
        PushPending("entity_slot_reuse_map_teardown", "native_identity_persisted", "native",
                    "{\"persisted\":true}", "need live engine: persist index+serial then UTIL_Remove");
    }
    const std::string reuse_exp = "{\"stale\":false,\"reused\":true}";
    if (g_reuse_done && g_stale_deliveries == 0) {
        PushRec("entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale", "native", "pass", reuse_exp,
                reuse_exp, "new occupant did not fire the old Touch subscription");
    } else if (g_stale_deliveries > 0) {
        PushRec("entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale", "native", "fail", reuse_exp,
                std::string("{\"stale\":true,\"deliveries\":") + std::to_string(g_stale_deliveries) + "}",
                "old subscription delivered for the new occupant");
    } else {
        PushPending("entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale", "native", reuse_exp,
                    kNeedReuse);
    }
    const std::string clr_exp = "{\"cleared\":true}";
    if (!g_map_ended) {
        PushPending("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native", clr_exp, kNeedMap);
    } else if (!g_post_map_invoke_attempted) {
        PushPending("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native", clr_exp,
                    kNeedMapInvoke);
    } else if (g_pre_map_touch > 0 && g_post_map_touch == g_pre_map_touch) {
        PushRec("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native", "fail", clr_exp,
                std::string("{\"cleared\":false,\"pre_map_count\":") + std::to_string(g_pre_map_touch) +
                    ",\"post_map_count\":" + std::to_string(g_post_map_touch) + "}",
                "stale post-map record: pre-map counter reused after map teardown");
    } else if (g_post_map_touch == 0) {
        PushRec("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native", "pass", clr_exp, clr_exp,
                "post-map callbacks cleared after EntByIndex invoke");
    } else {
        PushRec("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native", "fail", clr_exp,
                std::string("{\"cleared\":false,\"post_map_count\":") + std::to_string(g_post_map_touch) + "}",
                "post-map callback still firing on old identity");
    }
    const std::string ul_exp = "{\"reloaded\":true,\"peer_loaded\":true}";
    if (g_js_reappeared && g_saw_js_unloaded) {
        PushRec("entity_slot_reuse_map_teardown", "native_unload_reload", "native", "pass", ul_exp, ul_exp,
                "JS plugin reappeared after unload; probe peer stayed loaded");
    } else {
        PushPending("entity_slot_reuse_map_teardown", "native_unload_reload", "native", ul_exp, kNeedUnload);
    }

    const std::string voice_bits = "{\"allowed\":true,\"denied\":false}";
    if (g_voice_allowed_true >= 1 && g_voice_denied_false >= 1) {
        PushRec("voice_recall", "native_voice_listen_bits", "native", "pass", voice_bits, voice_bits,
                "SetClientListening effective bits after JS Voice.setAudibleTo");
    } else {
        PushPending("voice_recall", "native_voice_listen_bits", "native", voice_bits, kNeedThree);
    }
    if (g_voice_orig >= 1) {
        PushRec("voice_recall", "native_voice_original_once", "native", "pass", "{\"orig\":1}", "{\"orig\":1}",
                "SetClientListening original ran");
    } else {
        PushPending("voice_recall", "native_voice_original_once", "native", "{\"orig\":1}", kNeedThree);
    }

    const std::string lay_exp = "{\"layout_ok\":true}";
    if (g_tx_first_fire_on_ent && g_tx_layout_ok) {
        PushRec("check_transmit", "native_first_fire_layout", "native", "pass", lay_exp, lay_exp,
                "first CheckTransmit fire of the transmit entity; client int @576 in range");
    } else if (g_tx_first_fire_on_ent && !g_tx_layout_ok) {
        PushRec("check_transmit", "native_first_fire_layout", "native", "fail", lay_exp, "{\"layout_ok\":false}",
                "CheckTransmitInfo client int @576 out of range on transmit-entity first fire");
    } else {
        PushPending("check_transmit", "native_first_fire_layout", "native", lay_exp, kNeedPvs);
    }
    const std::string filt_exp = "{\"a\":true,\"b\":false}";
    if (g_tx_a_set >= 1 && g_tx_b_clear >= 1 && g_tx_b_set == 0) {
        PushRec("check_transmit", "native_per_recipient_filter", "native", "pass", filt_exp, filt_exp,
                "per-recipient CheckTransmit bits for the networked transmit entity");
    } else {
        PushPending("check_transmit", "native_per_recipient_filter", "native", filt_exp, kNeedPvs);
    }

    const std::string ho_exp = "{\"orig\":1,\"automatic_skipped\":true,\"listener\":1}";
    const bool handled_ok = g_fe_mask_obs.listener_count >= 1 && g_fe_mask_obs.automatic_skip_count >= 1 &&
                            g_mask_call_orig_super;
    if (handled_ok) {
        PushRec("fire_event_handled_recipient_mask", "native_handled_original_once", "native", "pass", ho_exp,
                ho_exp, "CallOriginal+Supersede: original/listener once, automatic skip true");
    } else if (g_mask_fire_stage == 0) {
        PushPending("fire_event_handled_recipient_mask", "native_handled_original_once", "native", ho_exp,
                    kNeedMask);
    } else {
        const std::string act = std::string("{\"orig\":") + std::to_string(g_fe_mask_obs.listener_count) +
                                ",\"automatic_skipped\":" + JsonBool(g_fe_mask_obs.automatic_skip_count >= 1) +
                                ",\"listener\":" + std::to_string(g_fe_mask_obs.listener_count) + "}";
        PushRec("fire_event_handled_recipient_mask", "native_handled_original_once", "native", "fail", ho_exp, act,
                "Handled must CallOriginal+Supersede (listener 1, skip true); not a zero-engine-call");
    }
    const std::string out_exp =
        "{\"subset\":true,\"excluded\":true,\"all_suppressed\":true,\"call_original_supersede\":true}";
    const bool out_ok = g_mask_subset_posts && !g_mask_excluded_posts &&
                        (g_mask_all_suppressed || (g_mask_fire_stage >= 2 && g_mask_post_count == 0)) &&
                        g_mask_call_orig_super;
    if (out_ok) {
        PushRec("fire_event_handled_recipient_mask", "native_outgoing_recipient_decisions", "native", "pass",
                out_exp, out_exp, "PostEventAbstract CMsgSource1LegacyGameEvent subset then all-suppressed");
    } else {
        PushPending("fire_event_handled_recipient_mask", "native_outgoing_recipient_decisions", "native", out_exp,
                    kNeedMask);
    }
}


static void ResetLiveCounters() {
    g_cc_entry_pre = g_cc_entry_post = 0;
    g_dc_cont_pre = g_dc_cont_post = g_dc_cont_skip = 0;
    g_dc_hand_pre = g_dc_hand_post = g_dc_hand_skip = 0;
    g_engine_continue = g_engine_handled = 0;
    g_dc_ctrl_missing_pre = g_engine_ctrl_missing = 0;
    g_dc_flip_c_pre = g_dc_flip_c_skip = g_engine_flip_c = 0;
    g_dc_flip_h_pre = g_dc_flip_h_skip = g_engine_flip_h = 0;
    g_game_frames = 0;
    g_clients_connected = 0;
    g_last_slot = -1;
    g_last_xuid = 0;
    std::memset(g_slot_gen, 0, sizeof(g_slot_gen));
    std::memset(g_slot_xuid, 0, sizeof(g_slot_xuid));
    g_fe_obs = s2khook::FireEventObservation{};
    g_fe_dontbroadcast_true = 0;
    g_collected = false;
    g_command_route = "unobserved";
}

static void PushPending(const char* cse, const char* sub, const char* producer, const std::string& expected,
                        const char* why) {
    PushRec(cse, sub, producer, "pending", expected, "{}", why);
}

static void PushR6Pending() {
    PushPending("fire_event_handled_recipient_mask", "native_handled_original_once", "native",
                "{\"orig\":1,\"automatic_skipped\":true,\"listener\":1}",
                "report before collect");
    PushPending("fire_event_handled_recipient_mask", "native_outgoing_recipient_decisions", "native",
                "{\"subset\":true,\"excluded\":true,\"all_suppressed\":true,\"call_original_supersede\":true}",
                "report before collect");
    PushPending("voice_recall", "native_voice_listen_bits", "native", "{\"allowed\":true,\"denied\":false}",
                "report before collect");
    PushPending("voice_recall", "native_voice_original_once", "native", "{\"orig\":1}", "report before collect");
    PushPending("sdkhooks_one_of_two_entities", "native_spawn_a_ok", "native", "{\"spawned\":true}",
                "report before collect");
    PushPending("sdkhooks_one_of_two_entities", "native_spawn_b_ok", "native", "{\"spawned\":true}",
                "report before collect");
    PushPending("sdkhooks_phase_removal", "native_phase_subscribe_pre_post", "native",
                "{\"pre\":1,\"post\":1,\"original\":1}", "report before collect");
    PushPending("sdkhooks_phase_removal", "native_phase_remove_pre", "native",
                "{\"pre\":0,\"post\":1,\"original\":1}", "report before collect");
    PushPending("sdkhooks_phase_removal", "native_phase_remove_post", "native",
                "{\"pre\":1,\"post\":0,\"original\":1}", "report before collect");
    PushPending("sdkhooks_phase_removal", "native_phase_self_unsubscribe", "native",
                "{\"first_pre\":1,\"first_post\":1,\"second_pre\":0,\"second_post\":1,\"original_first\":1,"
                "\"original_second\":1}",
                "report before collect");
    PushPending("sdkhooks_phase_removal", "native_phase_final_unsubscribe", "native",
                "{\"pre\":0,\"post\":0,\"original\":1}", "report before collect");
    PushPending("entity_slot_reuse_map_teardown", "native_identity_persisted", "native", "{\"persisted\":true}",
                "report before collect");
    PushPending("entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale", "native",
                "{\"stale\":false,\"reused\":true}", "report before collect");
    PushPending("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native", "{\"cleared\":true}",
                "report before collect");
    PushPending("entity_slot_reuse_map_teardown", "native_unload_reload", "native",
                "{\"reloaded\":true,\"peer_loaded\":true}", "report before collect");
    PushPending("check_transmit", "native_first_fire_layout", "native", "{\"layout_ok\":true}",
                "report before collect");
    PushPending("check_transmit", "native_per_recipient_filter", "native", "{\"a\":true,\"b\":false}",
                "report before collect");
}

static void PushInvalidOwned(const char* requested) {
    const std::string actual =
        std::string("{\"run_id\":\"") + JsonEscape(requested ? requested : "") +
        "\",\"error\":\"unknown_or_mismatched_run\"}";
    const std::string expected =
        std::string("{\"run_id\":\"") + JsonEscape(g_run_id.c_str()) + "\"}";
    const char* evd = "unknown or mismatched run_id; never reuse a prior run";
    const char* owned[][2] = {
        {"new_capsule_registration", "native_null_configure_failed"},
        {"new_capsule_registration", "native_first_valid_invoke_activates"},
        {"shared_capsule_registration", "native_two_consumers_active"},
        {"shared_capsule_registration", "native_gameframe_shared"},
        {"peer_actions_both_orders", "native_peer_actions_both_orders"},
        {"one_normal_invocation", "native_one_pre_post_orig"},
        {"frame_client_command_hooks", "native_gameframe_observed"},
        {"frame_client_command_hooks", "native_client_connected"},
        {"frame_client_command_hooks", "native_client_identity_join"},
        {"frame_client_command_hooks", "native_command_continue_original"},
        {"frame_client_command_hooks", "native_command_handled_skipped"},
        {"frame_client_command_hooks", "native_control_missing_js_hook"},
        {"frame_client_command_hooks", "native_control_flipped_decision"},
        {"frame_client_command_hooks", "native_control_omitted_acceptance_plugin"},
        {"fire_event_no_suppression", "native_invocation_scope"},
        {"fire_event_no_suppression", "native_original_once"},
        {"fire_event_no_suppression", "native_listener_delivery"},
        {"fire_event_no_suppression", "native_broadcast_unsuppressed"},
        {"fire_event_handled_recipient_mask", "native_handled_original_once"},
        {"fire_event_handled_recipient_mask", "native_outgoing_recipient_decisions"},
        {"voice_recall", "native_voice_listen_bits"},
        {"voice_recall", "native_voice_original_once"},
        {"sdkhooks_one_of_two_entities", "native_spawn_a_ok"},
        {"sdkhooks_one_of_two_entities", "native_spawn_b_ok"},
        {"sdkhooks_phase_removal", "native_phase_subscribe_pre_post"},
        {"sdkhooks_phase_removal", "native_phase_remove_pre"},
        {"sdkhooks_phase_removal", "native_phase_remove_post"},
        {"sdkhooks_phase_removal", "native_phase_self_unsubscribe"},
        {"sdkhooks_phase_removal", "native_phase_final_unsubscribe"},
        {"entity_slot_reuse_map_teardown", "native_identity_persisted"},
        {"entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale"},
        {"entity_slot_reuse_map_teardown", "native_map_teardown_clears"},
        {"entity_slot_reuse_map_teardown", "native_unload_reload"},
        {"check_transmit", "native_first_fire_layout"},
        {"check_transmit", "native_per_recipient_filter"},
    };
    for (const auto& row : owned) {
        PushRec(row[0], row[1], "native", "fail", expected, actual, evd);
    }
}

static void ObserveCommandRoute() {
    if (g_cc_entry_pre > 0) {
        if (g_dc_cont_pre > 0 || g_dc_hand_pre > 0) {
            g_command_route = "ClientCommand-entry+DispatchConCommand-original";
        } else {
            g_command_route = "ClientCommand-entry";
        }
    } else if (g_dc_cont_pre > 0 || g_dc_hand_pre > 0) {
        g_command_route = "DispatchConCommand-original";
    } else {
        g_command_route = "unobserved";
    }
}

static bool ContinueOriginalOk() {
    return g_dc_cont_pre >= 1 && g_dc_cont_post >= 1 && g_engine_continue >= 1 && g_dc_cont_skip == 0;
}

static bool ContinueObserved() {
    return g_dc_cont_pre > 0 || g_dc_cont_post > 0 || g_engine_continue > 0 || g_dc_cont_skip > 0;
}

static bool HandledOriginalOk() {
    return g_dc_hand_pre >= 1 && g_dc_hand_post >= 1 && g_engine_handled == 0 && g_dc_hand_skip >= 1;
}

static bool HandledObserved() {
    return g_dc_hand_pre > 0 || g_dc_hand_post > 0 || g_engine_handled > 0 || g_dc_hand_skip > 0;
}

static void PushCommandSubchecks() {
    ObserveCommandRoute();

    const std::string cont_exp = "{\"native_pre\":1,\"native_post\":1,\"engine\":1,\"skipped\":false}";
    const std::string hand_exp = "{\"native_pre\":1,\"native_post\":1,\"engine\":0,\"skipped\":true}";
    const std::string cont_act =
        std::string("{\"native_pre\":") + std::to_string(g_dc_cont_pre) + ",\"native_post\":" +
        std::to_string(g_dc_cont_post) + ",\"engine\":" + std::to_string(g_engine_continue) +
        ",\"skipped\":" + JsonBool(g_dc_cont_skip > 0) + ",\"route\":\"" + g_command_route +
        "\",\"clientcommand_entry\":" + JsonBool(g_cc_entry_pre >= 1 && g_cc_entry_post >= 1) + "}";
    const std::string hand_act =
        std::string("{\"native_pre\":") + std::to_string(g_dc_hand_pre) + ",\"native_post\":" +
        std::to_string(g_dc_hand_post) + ",\"engine\":" + std::to_string(g_engine_handled) +
        ",\"skipped\":" + JsonBool(g_dc_hand_skip > 0) + ",\"route\":\"" + g_command_route +
        "\",\"clientcommand_entry\":" + JsonBool(g_cc_entry_pre >= 1 && g_cc_entry_post >= 1) + "}";

    if (ContinueOriginalOk()) {
        PushRec("frame_client_command_hooks", "native_command_continue_original", "native", "pass",
                cont_exp, cont_exp,
                "continue token: DispatchConCommand original + probe callback (real client slot)");
    } else if (!ContinueObserved()) {
        PushRec("frame_client_command_hooks", "native_command_continue_original", "native", "pending",
                cont_exp, cont_act,
                "need a real client to issue s2_khook_probe_token s2khook-continue (RCON is control-only)");
    } else {
        PushRec("frame_client_command_hooks", "native_command_continue_original", "native", "fail",
                cont_exp, cont_act, g_command_route_note.c_str());
    }

    if (HandledOriginalOk()) {
        PushRec("frame_client_command_hooks", "native_command_handled_skipped", "native", "pass", hand_exp,
                hand_exp, "handled token: DispatchConCommand skipped, probe callback zero");
    } else if (!HandledObserved()) {
        PushRec("frame_client_command_hooks", "native_command_handled_skipped", "native", "pending",
                hand_exp, hand_act,
                "need a real client to issue s2_khook_probe_token s2khook-handled (RCON is control-only)");
    } else {
        PushRec("frame_client_command_hooks", "native_command_handled_skipped", "native", "fail", hand_exp,
                hand_act, g_command_route_note.c_str());
    }

    const std::string miss_exp = "{\"detected\":true,\"engine\":1,\"js_hook\":false}";
    if (g_dc_ctrl_missing_pre >= 1 && g_engine_ctrl_missing >= 1) {
        PushRec("frame_client_command_hooks", "native_control_missing_js_hook", "native", "pass", miss_exp,
                miss_exp, "control token reached engine without a JS hook");
    } else {
        PushPending("frame_client_command_hooks", "native_control_missing_js_hook", "native", miss_exp,
                    "need s2_khook_probe_token s2khook-ctrl-missing with JS hook disabled (RCON allowed)");
    }

    const std::string flip_exp =
        "{\"continue_engine\":0,\"handled_engine\":1,\"continue_skipped\":true,\"handled_skipped\":false}";
    const bool flip_ok = g_dc_flip_c_pre >= 1 && g_dc_flip_h_pre >= 1 && g_engine_flip_c == 0 &&
                          g_engine_flip_h >= 1 && g_dc_flip_c_skip >= 1 && g_dc_flip_h_skip == 0;
    if (flip_ok) {
        PushRec("frame_client_command_hooks", "native_control_flipped_decision", "native", "pass", flip_exp,
                flip_exp, "flipped Continue was suppressed and Handled was not");
    } else {
        PushPending("frame_client_command_hooks", "native_control_flipped_decision", "native", flip_exp,
                    "need flip tokens with JS Continue/Handled reversed (RCON allowed)");
    }

    const std::string omit_exp = "{\"js_plugin\":false}";
    if (!JsAcceptPresent()) {
        PushRec("frame_client_command_hooks", "native_control_omitted_acceptance_plugin", "native", "pass",
                omit_exp, omit_exp, "s2_khook_accept_run cvar absent");
    } else {
        PushPending("frame_client_command_hooks", "native_control_omitted_acceptance_plugin", "native",
                    omit_exp, "acceptance plugin is loaded; omitted-plugin control not run this collect");
    }
}

static void CollectControlledAndEvents() {
    const S2HookReceipt failed = fnNull.Configure(static_cast<const void*>(nullptr));
    const bool failed_ok =
        failed.state == S2HookState::Failed && failed.id == KHook::INVALID_HOOK && !failed.reason.empty();
    if (failed_ok) {
        PushRec("new_capsule_registration", "native_null_configure_failed", "native", "pass",
                "{\"state\":\"Failed\",\"id\":\"INVALID_HOOK\"}",
                "{\"state\":\"Failed\",\"id\":\"INVALID_HOOK\"}", failed.reason.c_str());
    } else {
        PushRec("new_capsule_registration", "native_null_configure_failed", "native", "fail",
                "{\"state\":\"Failed\",\"id\":\"INVALID_HOOK\"}",
                std::string("{\"state\":\"") + StateName(failed.state) + "\"}", "null Configure must fail");
    }

    g_pre_new = g_orig_new = 0;
    const int nret = TargetNew(3);
    const S2HookReceipt after_new = fnNew.Snapshot();
    const bool new_ok = after_new.state == S2HookState::Active && g_pre_new == 1 && g_orig_new == 1 && nret == 4;
    if (new_ok) {
        PushRec("new_capsule_registration", "native_first_valid_invoke_activates", "native", "pass",
                "{\"state\":\"Active\",\"pre\":1,\"orig\":1}",
                "{\"state\":\"Active\",\"pre\":1,\"orig\":1}", "first valid invoke");
    } else {
        PushRec("new_capsule_registration", "native_first_valid_invoke_activates", "native",
                after_new.state == S2HookState::Pending ? "pending" : "fail",
                "{\"state\":\"Active\",\"pre\":1,\"orig\":1}",
                std::string("{\"state\":\"") + StateName(after_new.state) + "\",\"pre\":" +
                    std::to_string(g_pre_new) + ",\"orig\":" + std::to_string(g_orig_new) + "}",
                "first valid invoke did not activate");
    }

    g_pre_share_a = g_pre_share_b = g_orig_share = 0;
    (void)TargetShare(1);
    const S2HookReceipt sa = fnShareA.Snapshot();
    const S2HookReceipt sb = fnShareB.Snapshot();
    const bool share_fn = sa.state == S2HookState::Active && sb.state == S2HookState::Active &&
                          g_pre_share_a == 1 && g_pre_share_b == 1 && g_orig_share == 1;
    if (share_fn) {
        PushRec("shared_capsule_registration", "native_two_consumers_active", "native", "pass",
                "{\"fnA\":\"Active\",\"fnB\":\"Active\",\"preA\":1,\"preB\":1,\"orig\":1}",
                "{\"fnA\":\"Active\",\"fnB\":\"Active\",\"preA\":1,\"preB\":1,\"orig\":1}",
                "two consumers one address");
    } else {
        PushRec("shared_capsule_registration", "native_two_consumers_active", "native", "fail",
                "{\"fnA\":\"Active\",\"fnB\":\"Active\",\"preA\":1,\"preB\":1,\"orig\":1}",
                "{\"fnA\":\"miss\"}", "shared consumers not both Active");
    }

    const S2HookReceipt frame_snap = g_plugin.gameFrame.Snapshot();
    if (g_frame_hooked && (frame_snap.state == S2HookState::Active || g_game_frames > 0)) {
        PushRec("shared_capsule_registration", "native_gameframe_shared", "native", "pass",
                "{\"observed\":true}", "{\"observed\":true}", "GameFrame shared capsule");
    } else if (share_fn) {
        PushPending("shared_capsule_registration", "native_gameframe_shared", "native",
                    "{\"observed\":true}", "engine shared GameFrame capsule not yet observed");
    } else {
        PushRec("shared_capsule_registration", "native_gameframe_shared", "native", "fail",
                "{\"observed\":true}", "{\"observed\":false}", "GameFrame Add not accepted");
    }

    bool peer_ok = false;
    std::string peer_act;
    RunPeerActions(peer_ok, peer_act);
    PushRec("peer_actions_both_orders", "native_peer_actions_both_orders", "native",
            peer_ok ? "pass" : "fail",
            "{\"ab_io\":true,\"ab_oo\":true,\"ab_os\":true,\"ba_io\":true,\"ba_oo\":true,\"ba_os\":true}",
            peer_act, peer_ok ? "both registration orders" : "peer action matrix failed");

    g_pre_once = g_post_once = g_orig_once = 0;
    const int once = TargetOnce(10);
    const bool once_ok = g_pre_once == 1 && g_post_once == 1 && g_orig_once == 1 && once == 15;
    PushRec("one_normal_invocation", "native_one_pre_post_orig", "native", once_ok ? "pass" : "fail",
            "{\"pre\":1,\"post\":1,\"orig\":1}",
            std::string("{\"pre\":") + std::to_string(g_pre_once) + ",\"post\":" +
                std::to_string(g_post_once) + ",\"orig\":" + std::to_string(g_orig_once) + "}",
            once_ok ? "one PRE+POST+orig" : "count mismatch");

    virtB.Remove(&g_dummyB);
    g_dummy_pre_a = g_dummy_pre_b = 0;
    g_dummyA.orig = g_dummyB.orig = 0;
    (void)SelectDummy(&g_dummyA)->Go(1);
    (void)SelectDummy(&g_dummyB)->Go(1);
    virtB.Add(&g_dummyB);

    if (!g_fe_hooked || !g_plugin.events) {
        PushRec("fire_event_no_suppression", "native_invocation_scope", "native", "fail",
                "{\"pre\":1,\"post\":1}", "{\"hooked\":false}",
                "FireEvent not installed on a real IGameEventManager2");
        PushRec("fire_event_no_suppression", "native_original_once", "native", "fail",
                "{\"orig\":1,\"automatic_skipped\":0}", "{\"hooked\":false}", "FireEvent not installed");
        PushRec("fire_event_no_suppression", "native_listener_delivery", "native", "fail",
                "{\"listener\":1}", "{\"hooked\":false}", "FireEvent not installed");
        PushRec("fire_event_no_suppression", "native_broadcast_unsuppressed", "native", "fail",
                "{\"dont_broadcast\":false}", "{\"hooked\":false}", "FireEvent not installed");
    } else {
        FeStopListening(g_plugin.events);
        const bool listened =
            g_plugin.events->AddListener(&g_fe_listener_obj, kFireEventNoSuppressName, true);
        g_fe_listening = listened;
        IGameEvent* ev = g_plugin.events->CreateEvent(kFireEventNoSuppressName, true);
        if (!ev) {
            FeStopListening(g_plugin.events);
            PushPending("fire_event_no_suppression", "native_invocation_scope", "native",
                        "{\"pre\":1,\"post\":1}", "CreateEvent returned null; descriptors may not be loaded");
            PushPending("fire_event_no_suppression", "native_original_once", "native",
                        "{\"orig\":1,\"automatic_skipped\":0}", "CreateEvent returned null");
            PushPending("fire_event_no_suppression", "native_listener_delivery", "native",
                        "{\"listener\":1}", "CreateEvent returned null");
            PushPending("fire_event_no_suppression", "native_broadcast_unsuppressed", "native",
                        "{\"dont_broadcast\":false}", "CreateEvent returned null");
        } else {
            g_fe_dontbroadcast_true = 0;
            {
                s2khook::FireEventInvocationScope scope(g_run_id.c_str(), "fire_event_no_suppression", ev);
                (void)g_plugin.events->FireEvent(ev, false);
                g_fe_obs = scope.Copy();
            }
            FeStopListening(g_plugin.events);
            const std::string scope_exp = "{\"pre\":1,\"post\":1,\"nested_foreign\":0}";
            const std::string scope_act = std::string("{\"pre\":") + std::to_string(g_fe_obs.pre_count) +
                                            ",\"post\":" + std::to_string(g_fe_obs.post_count) +
                                            ",\"nested_foreign\":0}";
            if (g_fe_obs.pre_count == 1 && g_fe_obs.post_count == 1) {
                PushRec("fire_event_no_suppression", "native_invocation_scope", "native", "pass", scope_exp,
                        scope_exp, "caller-owned FireEvent scope");
            } else {
                PushRec("fire_event_no_suppression", "native_invocation_scope", "native", "fail", scope_exp,
                        scope_act, "PRE/POST did not correlate to the fixture pointer");
            }
            const std::string orig_exp = "{\"orig\":1,\"automatic_skipped\":0}";
            const std::string orig_act = std::string("{\"orig\":") +
                                          std::to_string(g_fe_obs.automatic_original_count) +
                                          ",\"automatic_skipped\":" +
                                          std::to_string(g_fe_obs.automatic_skip_count) + "}";
            if (g_fe_obs.automatic_original_count == 1 && g_fe_obs.automatic_skip_count == 0) {
                PushRec("fire_event_no_suppression", "native_original_once", "native", "pass", orig_exp,
                        orig_exp, "automatic original once via POST skip-state");
            } else {
                PushRec("fire_event_no_suppression", "native_original_once", "native", "fail", orig_exp,
                        orig_act, "automatic original must run exactly once");
            }
            const std::string lis_exp = "{\"listener\":1}";
            if (g_fe_obs.listener_count == 1 && listened) {
                PushRec("fire_event_no_suppression", "native_listener_delivery", "native", "pass", lis_exp,
                        lis_exp, "engine listener during valid callback");
            } else {
                PushRec("fire_event_no_suppression", "native_listener_delivery", "native", "fail", lis_exp,
                        std::string("{\"listener\":") + std::to_string(g_fe_obs.listener_count) + "}",
                        "listener delivery must be exactly once");
            }
            const std::string bcast_exp = "{\"dont_broadcast\":false}";
            if (g_fe_dontbroadcast_true == 0) {
                PushRec("fire_event_no_suppression", "native_broadcast_unsuppressed", "native", "pass",
                        bcast_exp, bcast_exp, "incoming bDontBroadcast false");
            } else {
                PushRec("fire_event_no_suppression", "native_broadcast_unsuppressed", "native", "fail",
                        bcast_exp, "{\"dont_broadcast\":true}", "broadcast was suppressed");
            }
        }
    }
}

static void CollectLiveFrameClient() {
    if (g_frame_hooked && g_game_frames > 0) {
        PushRec("frame_client_command_hooks", "native_gameframe_observed", "native", "pass",
                "{\"observed\":true}", "{\"observed\":true}", "GameFrame PRE observed");
    } else if (g_frame_hooked) {
        PushPending("frame_client_command_hooks", "native_gameframe_observed", "native",
                    "{\"observed\":true}", "need live frames");
    } else {
        PushRec("frame_client_command_hooks", "native_gameframe_observed", "native", "fail",
                "{\"observed\":true}", "{\"observed\":false}", "GameFrame Add not accepted");
    }

    if (g_connected_hooked && g_clients_connected > 0) {
        PushRec("frame_client_command_hooks", "native_client_connected", "native", "pass",
                "{\"observed\":true}", "{\"observed\":true}", "OnClientConnected observed");
    } else if (g_connected_hooked) {
        PushPending("frame_client_command_hooks", "native_client_connected", "native",
                    "{\"observed\":true}", "need a real client connect");
    } else {
        PushRec("frame_client_command_hooks", "native_client_connected", "native", "fail",
                "{\"observed\":true}", "{\"observed\":false}", "OnClientConnected Add not accepted");
    }

    if (g_last_slot >= 0) {
        char steam[32];
        std::snprintf(steam, sizeof(steam), "%llu", static_cast<unsigned long long>(g_last_xuid));
        const std::string ident = std::string("{\"slot\":") + std::to_string(g_last_slot) +
                                  ",\"steamId\":\"" + steam + "\",\"run_id\":\"" +
                                  JsonEscape(g_run_id.c_str()) + "\"}";
        PushRec("frame_client_command_hooks", "native_client_identity_join", "native", "pass", ident, ident,
                "joined by slot+steamId+run_id");
    } else {
        PushPending("frame_client_command_hooks", "native_client_identity_join", "native",
                    "{\"slot\":0,\"steamId\":\"\",\"run_id\":\"bound\"}",
                    "need a real client identity to join with JS");
    }

    PushCommandSubchecks();
}

static void CollectRun() {
    g_stored.clear();
    CollectControlledAndEvents();
    CollectLiveFrameClient();
    CollectR6();
    g_collected = true;
}

static void PrepareRun(const char* run_id) {
    g_run_id = run_id ? run_id : "";
    g_run_bound = !g_run_id.empty();
    g_source_revision = S2_KHOOK_SOURCE_REVISION;
    ResetLiveCounters();
    g_stored.clear();
    R6PrepareEntities();
}

static bool RunMatches(const char* run_id) {
    return g_run_bound && run_id && g_run_id == run_id;
}

static void ProbeCommand(const CCommandContext& ctx, const CCommand& cmd) {
    (void)ctx;
    const char* a1 = cmd.Arg(1);
    const char* a2 = cmd.Arg(2);
    if (a1 && strcasecmp(a1, "prepare") == 0) {
        if (!a2 || !a2[0]) {
            META_CONPRINTF("usage: s2_khook_probe prepare <run_id>\n");
            return;
        }
        PrepareRun(a2);
        META_CONPRINTF("{\"khook_probe\":\"prepared\",\"run_id\":\"%s\",\"source_revision\":\"%s\"}\n",
                       JsonEscape(g_run_id.c_str()).c_str(), JsonEscape(g_source_revision.c_str()).c_str());
        return;
    }
    if (a1 && strcasecmp(a1, "collect") == 0) {
        if (!a2 || !a2[0]) {
            META_CONPRINTF("usage: s2_khook_probe collect <run_id>\n");
            return;
        }
        g_emit_run = a2;
        if (!RunMatches(a2)) {
            PrintRecs(InvalidOwned(a2));
            return;
        }
        CollectRun();
        PrintStored();
        return;
    }
    if (a1 && strcasecmp(a1, "report") == 0) {
        if (!a2 || !a2[0]) {
            META_CONPRINTF("usage: s2_khook_probe report <run_id>\n");
            return;
        }
        g_emit_run = a2;
        if (!RunMatches(a2)) {
            PrintRecs(InvalidOwned(a2));
            return;
        }
        if (!g_collected) {
            // Read-only: do not invoke FireEvent or controlled targets. Emit
            // pending envelopes so the judge is pending, not invalid.
            g_stored.clear();
            PushPending("new_capsule_registration", "native_null_configure_failed", "native",
                        "{\"state\":\"Failed\",\"id\":\"INVALID_HOOK\"}",
                        "report before collect");
            PushPending("new_capsule_registration", "native_first_valid_invoke_activates", "native",
                        "{\"state\":\"Active\",\"pre\":1,\"orig\":1}", "report before collect");
            PushPending("shared_capsule_registration", "native_two_consumers_active", "native",
                        "{\"fnA\":\"Active\",\"fnB\":\"Active\",\"preA\":1,\"preB\":1,\"orig\":1}",
                        "report before collect");
            PushPending("shared_capsule_registration", "native_gameframe_shared", "native",
                        "{\"observed\":true}", "report before collect");
            PushPending("peer_actions_both_orders", "native_peer_actions_both_orders", "native",
                        "{\"ab_io\":true,\"ab_oo\":true,\"ab_os\":true,\"ba_io\":true,\"ba_oo\":true,\"ba_os\":true}",
                        "report before collect");
            PushPending("one_normal_invocation", "native_one_pre_post_orig", "native",
                        "{\"pre\":1,\"post\":1,\"orig\":1}", "report before collect");
            PushPending("fire_event_no_suppression", "native_invocation_scope", "native",
                        "{\"pre\":1,\"post\":1,\"nested_foreign\":0}", "report before collect");
            PushPending("fire_event_no_suppression", "native_original_once", "native",
                        "{\"orig\":1,\"automatic_skipped\":0}", "report before collect");
            PushPending("fire_event_no_suppression", "native_listener_delivery", "native",
                        "{\"listener\":1}", "report before collect");
            PushPending("fire_event_no_suppression", "native_broadcast_unsuppressed", "native",
                        "{\"dont_broadcast\":false}", "report before collect");
            CollectLiveFrameClient();
            PushR6Pending();
        }
        PrintStored();
        return;
    }
    META_CONPRINTF("usage: s2_khook_probe prepare|collect|report <run_id>\n");
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
    setup.m_pszHelpString = "KHook suite probe: s2_khook_probe prepare|collect|report <run_id>";
    setup.m_nFlags = FCVAR_RELEASE;
    setup.m_CBInfo = ConCommandCallbackInfo_t(&ProbeCommand);
    cmdRef = icvar->RegisterConCommand(setup);
    if (!cmdRef.IsValidRef()) {
        META_CONPRINTF("[khook-probe] WARN: RegisterConCommand(%s) returned invalid ref\n", kCmdName);
    }

    ConCommandCreation_t token_setup;
    token_setup.m_pszName = g_tokenNameStore.c_str();
    token_setup.m_pszHelpString = "Probe-owned engine original for suite A command tokens";
    token_setup.m_nFlags = FCVAR_RELEASE;
    token_setup.m_CBInfo = ConCommandCallbackInfo_t(&ProbeTokenCommand);
    tokenRef = icvar->RegisterConCommand(token_setup);
    if (!tokenRef.IsValidRef()) {
        META_CONPRINTF("[khook-probe] WARN: RegisterConCommand(%s) returned invalid ref\n", kTokenCmd);
    } else {
        META_CONPRINTF("[khook-probe] ConCommand '%s' registered (engine original witness)\n", kTokenCmd);
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
    R6ResolveEngine(engineFactory, serverFactory);
    R6InstallEngineHooks();

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
    if (icvar) {
        RegisterRevisionCvar(icvar);
        const S2HookReceipt drec = dispatchConCommand.Add(icvar);
        g_dispatch_hooked = drec.Accepted();
        META_CONPRINTF("[khook-probe] DispatchConCommand Add state=%s (original-boundary witness)\n",
                       StateName(drec.state));
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

    META_CONPRINTF("[khook-probe] loaded source_revision=%s (test plugin, not for production release)\n",
                   g_source_revision.c_str());
    return true;
}

static void BeginProbeRetirement() {
    R6BeginRetirement();
    if (g_plugin.server) {
        g_plugin.gameFrame.Remove(g_plugin.server);
    }
    if (g_plugin.gameclients) {
        g_plugin.clientCommand.Remove(g_plugin.gameclients);
        g_plugin.onConnected.Remove(g_plugin.gameclients);
    }
    if (g_plugin.icvar) {
        g_plugin.dispatchConCommand.Remove(g_plugin.icvar);
    }
    if (g_plugin.events) {
        FeStopListening(g_plugin.events);
        g_plugin.fireEvent.Remove(g_plugin.events);
    }
    virtA.Remove(&g_dummyA);
    virtB.Remove(&g_dummyB);
    virtPre.Remove(&g_dummyPhase);
    virtPost.Remove(&g_dummyPhase);
    virtA.BeginRemove();
    virtB.BeginRemove();
    virtPre.BeginRemove();
    virtPost.BeginRemove();
    g_plugin.gameFrame.BeginRemove();
    g_plugin.clientCommand.BeginRemove();
    g_plugin.dispatchConCommand.BeginRemove();
    g_plugin.onConnected.BeginRemove();
    g_plugin.fireEvent.BeginRemove();
    fnNew.BeginRemove();
    fnShareA.BeginRemove();
    fnShareB.BeginRemove();
    fnAB_A.BeginRemove();
    fnAB_B.BeginRemove();
    fnBA_A.BeginRemove();
    fnBA_B.BeginRemove();
    fnOnce.BeginRemove();
}

bool ProbePlugin::Unload(char* error, size_t maxlen) {
    if (!S2Hook_NoActiveDispatch()) {
        if (error && maxlen) {
            std::snprintf(error, maxlen, "%s",
                          "khook-probe unload rejected: dispatch is active; retry meta unload");
        }
        return false;
    }
    if (!g_probe_retiring) {
        g_probe_retiring = true;
        BeginProbeRetirement();
    }
    if (!S2Hook_DrainRetirement() || S2Hook_RetirementPending() != 0) {
        if (error && maxlen) {
            std::snprintf(error, maxlen, "%s",
                          "khook-probe unload pending: hook retirement in progress; retry meta unload");
        }
        return false;
    }
    if (icvar && cmdRef.IsValidRef()) {
        icvar->UnregisterConCommandCallbacks(cmdRef);
    }
    if (icvar && tokenRef.IsValidRef()) {
        icvar->UnregisterConCommandCallbacks(tokenRef);
    }
    events = nullptr;
    return true;
}
