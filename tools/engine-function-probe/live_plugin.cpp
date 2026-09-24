// Test-only resident Metamod consumer of the installed official KHook interface.
// Controlled native ABI evidence is deliberately labelled separately from engine
// Ignite/Acquire/HUD observations. It is never a replacement provider.
#include <ISmmPlugin.h>
#include <icvar.h>
#include <convar.h>
#include "engine_function_abi.h"
#include <unistd.h>
#include <chrono>
#include <functional>
#include <sstream>
#include <stdexcept>
#include <fstream>
#include <filesystem>
#include <dlfcn.h>
#include <eiface.h>
#include <schemasystem/schemasystem.h>
#include <entity2/entitysystem.h>
#include <entity2/entityinstance.h>
#include "gamedata.h"
#include "engine_resolver.h"
#include "../third_party/json.hpp"

PLUGIN_GLOBALVARS();
#ifndef S2FN_SOURCE_REVISION
#error source-bound probe revision required
#endif
#ifndef S2FN_BUILD_TOKEN
#error source-bound probe build token required
#endif
using namespace s2fn;
namespace {
struct FixtureObject { std::uint32_t magic = 0x5132464e; };
FixtureObject receiver, opaque;
std::uint64_t original_count = 0;
float observed_lifetime = 0;
std::int32_t observed_flags = 0;
void* observed_attacker = nullptr;
float observed_size = 0;
__attribute__((noinline,noclone)) void IgniteAbi(FixtureObject* self, float lifetime, std::int32_t flags, void* attacker, float size) {
    if (self != &receiver) throw std::runtime_error("controlled receiver mismatch");
    ++original_count; observed_lifetime = lifetime; observed_flags = flags; observed_attacker = attacker; observed_size = size;
}
__attribute__((noinline,noclone)) std::int32_t AcquireAbi(FixtureObject* self, void* item, std::int32_t method, void* unknown) {
    ++original_count; return self == &receiver && item == &opaque && method == 2 && unknown == nullptr ? 3 : -1;
}
__attribute__((noinline,noclone)) void HudAbi(FixtureObject* self, void* controller, void* layout, void* text) {
    ++original_count;
    if (self != &receiver || controller != &opaque || layout != &receiver || text != nullptr)
        throw std::runtime_error("controlled HUD ABI mismatch");
}
__attribute__((noinline,noclone)) float Novel(std::uint32_t a, float b, void* c, std::uint64_t d) {
    ++original_count; return a + b + (c == &opaque ? 3.f : 0.f) + d;
}
using NovelPeer = KHook::Function<float, std::uint32_t, float, void*, std::uint64_t>;
std::unique_ptr<NovelPeer> peer;
std::uint64_t peer_pre = 0, peer_post = 0;
float peer_effective = 0;
bool peer_skipped = false;
KHook::Return<float> PeerPre(std::uint32_t, float, void*, std::uint64_t) { ++peer_pre; return {KHook::Action::Override, 90.f}; }
KHook::Return<float> PeerPost(std::uint32_t, float, void*, std::uint64_t) {
    ++peer_post; peer_effective = KHook::GetCurrentReturn<float>(); peer_skipped = KHook::WasOriginalFunctionSkipped();
    return {KHook::Action::Ignore, 0.f};
}
struct Sink final : DispatchSink {
    std::function<void(DispatchFrame&)> fn;
    std::uint64_t pre = 0, post = 0;
    std::string error;
    void Dispatch(DispatchFrame& frame) override {
        if (frame.phase == Phase::Pre) ++pre; else ++post;
        if (fn) fn(frame);
    }
    void Error(const char* text) noexcept override { error = text; }
};
struct Owned {
    Sink sink;
    std::unique_ptr<RuntimeBinding> binding;
};
std::vector<std::unique_ptr<Owned>> owned;
std::string run, resident;
std::uint64_t generation = 0, last_retired = 0;
bool retiring = false;
ICvar* cvars = nullptr;
ConCommandRef command_ref;
std::vector<std::string> records;
std::string Escape(const std::string& text) {
    std::string out;
    for (char c : text) { if (c == '"' || c == '\\') out += '\\'; if (c == '\n') out += "\\n"; else out += c; }
    return out;
}
void Record(const std::string& name, bool ok, const std::string& facts = "{}", const std::string& provenance = "controlled-native-abi") {
    std::ostringstream out;
    out << "{\"kind\":\"engine-function-observation\",\"case\":\"" << name
        << "\",\"result\":\"" << (ok ? "pass" : "fail") << "\",\"provenance\":\"" << provenance << "\",\"source\":\""
        << S2FN_SOURCE_REVISION << "\",\"token\":\"" << S2FN_BUILD_TOKEN << "\",\"run\":\"" << run
        << "\",\"generation\":" << generation << ",\"resident\":\"" << resident << "\",\"pid\":" << getpid()
        << ",\"facts\":" << facts << '}';
    records.push_back(out.str());
}
void Require(bool value, const char* why) { if (!value) throw std::runtime_error(why); }
Owned& Bind(AbiSignature signature, const void* target) {
    auto item = std::make_unique<Owned>();
    auto made = RuntimeBinding::Create(std::move(signature), item->sink); Require(static_cast<bool>(made), made.error.c_str());
    item->binding = std::move(made.value);
    auto receipt = item->binding->Configure(target); Require(receipt.Accepted(), receipt.reason.c_str());
    owned.push_back(std::move(item)); return *owned.back();
}
void StartRemoval() {
    retiring = true;
    for (auto& item : owned) item->binding->BeginRemove();
}
bool CollectRemoval() {
    if (!retiring) return owned.empty();
    for (const auto& item : owned) if (!item->binding->RemovalComplete()) return false;
    const auto count = owned.size(); owned.clear(); // only after both acknowledgements
    S2Hook_DrainRetirement(); retiring = false; last_retired = generation;
    auto volatile call = static_cast<float(*)(std::uint32_t,float,void*,std::uint64_t)>(&Novel);
    const auto before = peer_post;
    const float result = call(2, 1.25f, &opaque, 4);
    Record("removal-before-free", count == 5 && peer_post == before + 1 && result == 90.f,
        "{\"bindings_destroyed_after_completion\":" + std::to_string(count) + ",\"peer_observed_after_removal\":" +
        std::to_string(peer_post - before) + ",\"effective\":" + std::to_string(result) + "}");
    return true;
}
#include "live_engine.inc"
void Arm(std::uint64_t next) {
    Require(owned.empty() && !retiring, "previous generation has not drained");
    Require(next > generation && (generation == 0 || last_retired == generation), "generation order mismatch");
    generation = next;
    AbiSignature s; s.receiver = "entity"; s.parameters = {{"f32"},{"i32"},{"ptr"},{"f32"}};
    Bind(s, reinterpret_cast<void*>(&IgniteAbi));
    s.parameters = {{"ptr"},{"i32"},{"ptr"}}; s.returns = {"i32"}; Bind(s,reinterpret_cast<void*>(&AcquireAbi));
    s.parameters = {{"ptr"},{"ptr"},{"ptr"}}; s.returns = {"void"}; Bind(s,reinterpret_cast<void*>(&HudAbi));
    s = {}; s.parameters = {{"u32"},{"f32"},{"ptr"},{"u64"}}; s.returns = {"f32"}; Bind(s,reinterpret_cast<void*>(&Novel));
    PrepareReal(g_SMAPI);
    Record("generation-armed", true, "{\"bindings\":5}");
}
void Exercise() {
    Require(owned.size() == 5 && !retiring, "generation not armed");
    auto& ignite = *owned[0];
    ignite.sink.fn = [](DispatchFrame& f) {
        Require(f.receiver.Get<void*>() == &receiver, "Ignite receiver not delivered");
        if (f.phase == Phase::Pre) { f.arguments[0] = NativeValue::From<float>(5.f); f.changed = true; }
    };
    NativeValue iv[]{NativeValue::From(&receiver),NativeValue::From<float>(10.f),NativeValue::From<std::int32_t>(4),NativeValue::From(&opaque),NativeValue::From<float>(0.5f)};
    auto before = original_count; auto result = ignite.binding->Call(iv,5);
    Record("ignite-scalar-compatibility", result && ignite.sink.error.empty() && original_count == before+1 &&
        observed_lifetime == 5.f && observed_flags == 4 && observed_attacker == &opaque && observed_size == .5f && ignite.sink.post == 1,
        "{\"original_delta\":"+std::to_string(original_count-before)+",\"lifetime_after_recall\":"+std::to_string(observed_lifetime)+",\"post\":"+std::to_string(ignite.sink.post)+"}");
    NativeValue av[]{NativeValue::From(&receiver),NativeValue::From(&opaque),NativeValue::From<std::int32_t>(2),NativeValue::From<void*>(nullptr)};
    before=original_count; result=owned[1]->binding->Call(av,4);
    Record("acquire-compatibility", result && result.value.Get<std::int32_t>()==3 && original_count==before+1 && owned[1]->sink.post==1);
    NativeValue hv[]{NativeValue::From(&receiver),NativeValue::From(&opaque),NativeValue::From(&receiver),NativeValue::From<void*>(nullptr)};
    before=original_count; result=owned[2]->binding->Call(hv,4);
    Record("hud-compatibility", result && original_count==before+1 && owned[2]->sink.pre==1 && owned[2]->sink.post==1);
    auto& novel=*owned[3]; bool nested=false; bool returned=false;
    novel.sink.fn=[&](DispatchFrame& f) {
        if (f.phase == Phase::Pre && !nested) {
            nested=true; NativeValue args[]{NativeValue::From<std::uint32_t>(1),NativeValue::From<float>(2.f),NativeValue::From(&opaque),NativeValue::From<std::uint64_t>(3)};
            auto inner=novel.binding->Call(args,4); returned=inner && inner.value.Get<float>()==90.f; nested=false;
        }
    };
    NativeValue nv[]{NativeValue::From<std::uint32_t>(2),NativeValue::From<float>(1.25f),NativeValue::From(&opaque),NativeValue::From<std::uint64_t>(4)};
    auto peers=peer_post; before=original_count; result=novel.binding->Call(nv,4);
    Record("novel-reentry-peer", result && result.value.Get<float>()==90.f && returned && original_count==before+2 && peer_post==peers+2 && novel.sink.pre==2 && novel.sink.post==2);
    novel.sink.fn=[](DispatchFrame& f) { if(f.phase==Phase::Pre){f.action=KHook::Action::Supersede;f.result=NativeValue::From<float>(77.f);} };
    before=original_count; result=novel.binding->Call(nv,4);
    Record("typed-suppression-peer", result && result.value.Get<float>()==77.f && original_count==before && peer_skipped && peer_effective==77.f);
    for(std::size_t i=0;i<4;++i) { auto& item=owned[i]; if(!item->sink.error.empty()) Record("closure-error",false,"{\"error\":\""+Escape(item->sink.error)+"\"}"); item->sink.fn={}; }
}
void Command(const CCommandContext&, const CCommand& cmd) {
    try {
        const std::string op=cmd.Arg(1);
        if(op=="runtime") {
            META_CONPRINTF("{\"kind\":\"engine-function-runtime\",\"source\":\"%s\",\"token\":\"%s\",\"resident\":\"%s\",\"pid\":%ld}\n",S2FN_SOURCE_REVISION,S2FN_BUILD_TOKEN,resident.c_str(),static_cast<long>(getpid()));
            for(const auto& row:records) META_CONPRINTF("%s\n",row.c_str());
            return;
        }
        const std::string requested=cmd.Arg(2);
        Require(!requested.empty() && requested.find_first_not_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-")==std::string::npos,"invalid run token");
        if(op=="arm") {
            Require(std::string(cmd.Arg(4))==S2FN_BUILD_TOKEN,"fixture build token mismatch");
            Require(run.empty() || requested==run,"resident probe already bound to a different run");
            run=requested; Arm(std::stoull(cmd.Arg(3)));
        } else {
            Require(requested==run,"run mismatch");
            if(op=="exercise") Exercise();
            else if(op=="acquire-results") CollectReal();
            else if(op=="retire") { Require(std::stoull(cmd.Arg(3))==generation,"retire generation mismatch"); StartRemoval(); }
            else if(op=="collect") CollectRemoval();
            else throw std::runtime_error("unknown operation");
        }
    } catch(const std::exception& e) { Record("probe-error",false,"{\"error\":\""+Escape(e.what())+"\"}"); }
    for(const auto& row:records) META_CONPRINTF("%s\n",row.c_str());
}
class Probe final : public ISmmPlugin {
public:
    bool Load(PluginId id,ISmmAPI* ismm,char* error,size_t length,bool) override {
        PLUGIN_SAVEVARS();
        auto factory=ismm->GetEngineFactory(false); int status=0;
        cvars=factory?static_cast<ICvar*>(factory(CVAR_INTERFACE_VERSION,&status)):nullptr;
        if(!cvars || !KHook::__exported__khook) { ismm->Format(error,length,"official KHook/ICvar unavailable"); return false; }
        resident=std::to_string(getpid())+"-"+std::to_string(std::chrono::steady_clock::now().time_since_epoch().count());
        peer=std::make_unique<NovelPeer>(&Novel,&PeerPre,&PeerPost);
        ConCommandCreation_t setup; setup.m_pszName="s2_engine_probe"; setup.m_pszHelpString="test-only bounded ABI probe"; setup.m_nFlags=FCVAR_RELEASE; setup.m_CBInfo=ConCommandCallbackInfo_t(&Command);
        command_ref=cvars->RegisterConCommand(setup);
        return command_ref.IsValidRef();
    }
    bool Unload(char* error,size_t length) override {
        if(!owned.empty()) { StartRemoval(); if(!CollectRemoval()) { g_SMAPI->Format(error,length,"runtime bindings retiring; retry after callbacks drain"); return false; } }
        if(acquire_peer) { acquire_peer->BeginRemove(true); if(!acquire_peer->RemovalComplete()) return false; acquire_peer.reset(); }
        peer.reset(); if(command_ref.IsValidRef()) cvars->UnregisterConCommandCallbacks(command_ref); return true;
    }
    const char* GetAuthor() override{return "s2script";}
    const char* GetName() override{return "s2_engine_function_probe";}
    const char* GetDescription() override{return "Test-only stock-provider runtime ABI evidence";}
    const char* GetURL() override{return "https://s2script.com";}
    const char* GetLicense() override{return "MIT OR Apache-2.0";}
    const char* GetVersion() override{return "0.1.0-test";}
    const char* GetDate() override{return __DATE__;}
    const char* GetLogTag() override{return "S2FNPROBE";}
};
Probe plugin;
}
PLUGIN_EXPOSE(Probe,plugin);
