// Exercises the shipped installation, typed KHook wrappers, invocation records and accessors.
// IKHook substitutes only the native provider/JIT; actual Recall and checked helpers are used.
#include "engine_hooks.h"
#include "engine_calls.h"
#include "hook_dispatch.h"
#include "khook_map.h"
#include <algorithm>
#include <cstring>
#include <functional>
#include <iostream>
#include <map>
#include <vector>

static int failures = 0;
#define CHECK(c, why) do { if (!(c)) { std::cerr << "FAIL: " << why << "\n"; ++failures; } } while (0)
namespace KHook { IKHook* __exported__khook = nullptr; }

namespace {
struct Registration { void* address; void* context; void* removed; void* pre; void* post; };
struct Frame {
    Registration* registration;
    void* continuation;
    KHook::Action action = KHook::Action::Ignore;
    int32_t engine = 0, override_value = 0;
    bool recalled = false, skipped = false;
};
class Provider final : public KHook::IKHook {
public:
    std::map<KHook::HookID_t, Registration> entries;
    KHook::HookID_t next = 100;
    Frame* frame = nullptr;
    void* removal_context = nullptr;
    bool fail_setup = false;
    int original_reads = 0, override_submissions = 0;
    std::function<void()> before_original, before_post, after_post;
    KHook::HookID_t SetupHook(void* fn, void* ctx, void* removed, void* pre, void* post,
                            void*, void*, unsigned int, bool) override {
        if (fail_setup) return KHook::INVALID_HOOK;
        const auto id = next++;
        entries.emplace(id, Registration{fn, ctx, removed, pre, post});
        return id;
    }
    KHook::HookID_t SetupVirtualHook(void**, int, void*, void*, void*, void*, void*, void*, unsigned int, bool) override {
        return KHook::INVALID_HOOK;
    }
    void RemoveHook(KHook::HookID_t id, bool async, void (*done)(KHook::HookID_t, void*), void* ctx) override {
        CHECK(!async, "terminal removal is synchronous");
        auto it = entries.find(id);
        if (it == entries.end()) return;
        removal_context = it->second.context;
        reinterpret_cast<void (*)(KHook::HookID_t)>(it->second.removed)(id);
        removal_context = nullptr;
        entries.erase(it);
        if (done) done(id, ctx);
    }
    void* GetContextPtr() override { return removal_context ? removal_context : frame->registration->context; }
    void* GetOriginalFunction() override { return frame->registration->address; }
    void* GetOriginalValuePtr() override {
        ++original_reads;
        CHECK(!frame->skipped, "never read an original return after skip");
        return &frame->engine;
    }
    void* GetOverrideValuePtr() override { return &frame->override_value; }
    void* GetCurrentValuePtr(bool) override {
        return frame->action >= KHook::Action::Override ? &frame->override_value : &frame->engine;
    }
    void DestroyReturnValue() override {}
    void* FindOriginal(void* fn) override { return fn; }
    void* FindOriginalVirtual(void** vt, int i) override { return vt[i]; }
    void* LookupSignature(void*, std::size_t, const char*) override { return nullptr; }
    bool WasOriginalFunctionSkipped() override { return frame->skipped; }
    void* DoRecall(KHook::Action a, void* value, std::size_t size, void* init, void* deinit) override {
        frame->recalled = true;
        SaveReturnValue(a, value, size, init, deinit, false);
        return frame->continuation;
    }
    void SaveReturnValue(KHook::Action a, void* value, std::size_t size, void*, void*, bool original) override {
        if (original && size) std::memcpy(&frame->engine, value, size);
        if (a == KHook::Action::Override) ++override_submissions;
        // Pinned stock rule: equal actions retain the earlier result.
        if (a > frame->action) {
            frame->action = a;
            if (size) std::memcpy(&frame->override_value, value, size);
        }
    }
    void Peer(KHook::Action a, int32_t value) {
        SaveReturnValue(a, &value, sizeof value, nullptr, nullptr, false);
    }
    Registration* Find(void* fn) {
        for (auto& [id, entry] : entries) if (entry.address == fn) return &entry;
        return nullptr;
    }
    template <typename R, typename... A> static R Continue(A... args);
    template <typename R, typename... A> R Invoke(R (*fn)(A...), A... args) {
        auto* entry = Find(reinterpret_cast<void*>(fn));
        CHECK(entry, "target was installed with stock typed helper");
        if (!entry) { if constexpr (!std::is_void_v<R>) return R{}; else return; }
        Frame invocation{entry, reinterpret_cast<void*>(&Continue<R, A...>)};
        Frame* saved = frame;
        frame = &invocation;
        reinterpret_cast<R (*)(A...)>(entry->pre)(args...);
        if (!invocation.recalled) Continue<R, A...>(args...);
        const int32_t effective = *static_cast<int32_t*>(GetCurrentValuePtr(false));
        frame = saved;
        if constexpr (!std::is_void_v<R>) return effective;
    }
};
Provider provider;
template <typename R, typename... A> R Provider::Continue(A... args) {
    auto& p = provider;
    if (p.before_original) p.before_original();
    p.frame->skipped = p.frame->action == KHook::Action::Supersede;
    if (!p.frame->skipped) {
        auto fn = reinterpret_cast<R (*)(A...)>(p.frame->registration->address);
        if constexpr (std::is_void_v<R>) fn(args...); else p.frame->engine = fn(args...);
    }
    if (p.before_post) p.before_post();
    reinterpret_cast<R (*)(A...)>(p.frame->registration->post)(args...);
    if (p.after_post) p.after_post();
    if constexpr (!std::is_void_v<R>) return *static_cast<int32_t*>(p.GetCurrentValuePtr(false));
}

std::map<const void*, s2resolve::Resolution> resolutions;
void Put(std::vector<uint8_t>& b, size_t at, uint64_t n, size_t count) {
    for (size_t i = 0; i < count; ++i) b.at(at+i) = uint8_t(n >> (8*i));
}
void ImageFor(const void* target, bool narrow_mismatch = false) {
    std::vector<uint8_t> b(0x1100);
    std::memcpy(b.data(), "\177ELF\2\1\1", 7);
    Put(b,16,3,2); Put(b,18,62,2); Put(b,20,1,4); Put(b,32,64,8);
    Put(b,52,64,2); Put(b,54,56,2); Put(b,56,3,2);
    auto segment = [&](int i, int type, int flags, int off, int size) {
        const int p=64+i*56;
        Put(b,p,type,4); Put(b,p+4,flags,4); Put(b,p+8,off,8); Put(b,p+16,off,8);
        Put(b,p+32,size,8); Put(b,p+40,size,8); Put(b,p+48,1,8);
    };
    segment(0,1,4,0,0x800); segment(1,1,5,0x1000,0x100); segment(2,4,4,0x300,20);
    Put(b,0x300,4,4); Put(b,0x304,4,4); Put(b,0x308,3,4);
    std::memcpy(b.data()+0x30c,"GNU",4); Put(b,0x310,0x3412cdab,4);
    b[0x1000]=0xc3;
    if (narrow_mismatch) { const uint8_t code[]={0x48,0x89,0x55,0xf0,0xc3}; std::memcpy(b.data()+0x1000,code,sizeof code); }
    const uintptr_t base=reinterpret_cast<uintptr_t>(target)-0x1000;
    s2original::Identity id{7,9,base,"abcd1234"};
    std::string reason;
    auto image=s2original::FromElf(b,id,id,{{base,base+0x800,0,7,9},{base+0x1000,base+0x1100,0x1000,7,9}},reason);
    CHECK(image, "fixture ELF produces verified Image");
    resolutions[target]={reinterpret_cast<uintptr_t>(target),image,"fixture","original image"};
}
std::function<int(int,void*)> dispatch;
int Dispatch(int id,void* view) { return dispatch ? dispatch(id,view) : 0; }
int originals=0;
std::vector<int> order;
void* last_view=nullptr;
void VoidTarget(void*) { ++originals; order.push_back(3); CHECK(S2Hook_ActiveCount()>0,"Observe remains held through original in Recall"); }
void OtherVoid(void*) { ++originals; }
void MissingTarget(void*) {}
void RejectTarget(void*) {}
void MarkerTarget(void*) {}
void BadWidth(void*,float,int32_t,int32_t,int32_t) {}
float got_float=0; int32_t got_ints[3]{}; int64_t got_wide[3]{};
void Narrow(void*,float f,int32_t a,int32_t b,int32_t c) { ++originals; got_float=f; got_ints[0]=a; got_ints[1]=b; got_ints[2]=c; }
void Wide(void*,float f,int32_t a,int64_t b,int64_t c) { ++originals; got_float=f; got_ints[0]=a; got_wide[0]=b; got_wide[1]=c; }
void* receiver=reinterpret_cast<void*>(uintptr_t{0x12345678});
constexpr int64_t high_a=static_cast<int64_t>(UINT64_C(0xf123456789abcdef));
constexpr int64_t high_b=static_cast<int64_t>(UINT64_C(0x8123456789abcdef));
template <typename F> bool Install(int id,int shape,F fn) {
    ImageFor(reinterpret_cast<const void*>(fn));
    char reason[256]{};
    const bool ok=S2_HookInstall(id,shape,reinterpret_cast<int64_t>(fn),reason,sizeof reason)==0;
    CHECK(ok, reason);
    return ok;
}
void MutationAndNesting() {
    dispatch=[](int id,void* v) {
        last_view=v;
        if (id==1 || id==2) {
            CHECK(S2_HookWriteF32(v,0,7.25f)==0,"float edit accepted");
            CHECK(S2_HookWriteI32(v,1,-17)==0,"integer edit accepted");
            if(id==1) { S2_HookWriteI32(v,2,23); S2_HookWriteI32(v,3,-31); }
            CHECK(S2_HookWriteF32(v,1,4)==-1,"wrong scalar class refused");
            CHECK(S2_HookWriteI32(v,4,4)==-1,"opaque/out of range position refused");
        }
        return 1;
    };
    provider.Invoke(&Narrow,receiver,1.f,int32_t{2},int32_t{3},int32_t{4});
    CHECK(got_float==7.25f && got_ints[0]==-17 && got_ints[1]==23 && got_ints[2]==-31,"all narrow edits reach original");
    provider.Invoke(&Wide,receiver,1.f,int32_t{2},high_a,high_b);
    CHECK(got_float==7.25f && got_ints[0]==-17 && got_wide[0]==high_a && got_wide[1]==high_b,"wide edits preserve full opaque bits");
    float f;
    CHECK(S2_HookReadF32(last_view,0,&f)==-1,"view expires after Recall");
    int depth=0;
    dispatch=[&](int id,void* v) {
        uint32_t handle;
        CHECK(S2_HookReceiverHandle(v,&handle)==0,"nested view live");
        if(id==0 && depth<20) {
            ++depth;
            provider.Invoke(&VoidTarget,receiver);
            provider.Invoke(&OtherVoid,receiver);
            --depth;
            CHECK(S2_HookReceiverHandle(v,&handle)==0,"same/different ID nesting restores enclosing view");
        }
        return 0;
    };
    provider.Invoke(&VoidTarget,receiver);
    CHECK(S2Hook_ActiveCount()==0,"all nested Observe holds released");
}
void Bypass() {
    int calls=0;
    dispatch=[&](int,void*) { ++calls; return 0; };
    S2_HookArmBypass(2);
    provider.Invoke(&VoidTarget,receiver);
    provider.Invoke(&Wide,receiver,1.f,int32_t{2},high_a,high_b);
    CHECK(calls==1,"bypass belongs to its ID");
    CHECK(got_wide[0]==high_a && got_wide[1]==high_b,"a bypassed call still reaches the original at full width");
    provider.Invoke(&Wide,receiver,1.f,int32_t{2},high_a,high_b);
    CHECK(calls==2,"bypass consumed once");
    S2_HookArmBypass(2); S2_HookDisarmBypass(2);
    provider.Invoke(&Wide,receiver,1.f,int32_t{2},high_a,high_b);
    CHECK(calls==3,"early outbound return disarms latch");

    int pre_count=0;
    bool nested=false;
    dispatch=[&](int,void*) { ++pre_count; return 0; };
    provider.before_original=[&] {
        if (!nested) { nested=true; provider.Invoke(&Narrow,receiver,1.f,int32_t{1},int32_t{2},int32_t{3}); }
    };
    S2_HookArmBypass(1);
    provider.Invoke(&Narrow,receiver,1.f,int32_t{1},int32_t{2},int32_t{3});
    CHECK(pre_count==1,"bypassed outer still permits normal same-ID nested dispatch");
    provider.before_original={}; dispatch={};
}

void RejectionAndLifecycle() {
    char reason[256]{};
    const auto count=provider.entries.size();
    CHECK(S2_HookInstall(0,0,reinterpret_cast<int64_t>(&VoidTarget),reason,sizeof reason)==0 && provider.entries.size()==count,"same install is an idempotent success");
    CHECK(S2_HookInstall(0,0,reinterpret_cast<int64_t>(&OtherVoid),reason,sizeof reason)==-1,"occupied ID cannot change target");
    CHECK(S2_HookInstall(9,0,reinterpret_cast<int64_t>(&VoidTarget),reason,sizeof reason)==-1,"another local descriptor cannot claim target");
    CHECK(S2_HookInstall(-1,0,1,reason,sizeof reason)==-1,"negative hook ID rejected");
    CHECK(S2_HookInstall(64,0,1,reason,sizeof reason)==-1,"hook ID beyond table rejected");
    CHECK(S2_HookInstall(9,99,1,reason,sizeof reason)==-1,"unknown shape rejected");
    CHECK(S2_HookInstall(9,0,0,reason,sizeof reason)==-1,"null target rejected");
    ImageFor(reinterpret_cast<void*>(&RejectTarget));
    provider.fail_setup=true;
    CHECK(S2_HookInstall(9,0,reinterpret_cast<int64_t>(&RejectTarget),reason,sizeof reason)==-1 && std::strstr(reason,"INVALID_HOOK"),"provider refusal preserves named receipt failure");
    provider.fail_setup=false;
    CHECK(S2_HookInstall(9,0,reinterpret_cast<int64_t>(&RejectTarget),reason,sizeof reason)==0,"failed registration does not poison slot");
    auto* entry=provider.Find(reinterpret_cast<void*>(&RejectTarget));
    auto* helper=static_cast<S2CheckedFunction<void,void*>*>(static_cast<KHook::Function<void,void*>*>(entry->context));
    CHECK(helper->Snapshot().state==S2HookState::Pending && helper->Snapshot().id>=100,"actual provider receipt is Pending, not descriptor ID");
    provider.Invoke(&RejectTarget,receiver);
    CHECK(helper->Snapshot().state==S2HookState::Active,"first production callback activates its receipt");

    int depth=0, pre_count=0;
    dispatch=[&](int id,void* view) {
        ++pre_count;
        if (id==1 && depth==0) {
            ++depth;
            S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
            provider.Invoke(&Narrow,receiver,1.f,int32_t{4},int32_t{5},int32_t{6});
            S2Hook_SetLifecycle(S2HookLifecycle::Running);
            --depth;
            int32_t a=0;
            CHECK(S2_HookReadI32(view,1,&a)==0 && a==12,"rejected nested invocation restores outer view");
        }
        return 0;
    };
    provider.Invoke(&Narrow,receiver,1.f,int32_t{12},int32_t{2},int32_t{3});
    CHECK(pre_count==1 && S2Hook_ActiveCount()==0,"rejected dispatch leaks neither JS nor Observe holds");
    dispatch={};

    S2_HookResetAll();
    CHECK(provider.Find(reinterpret_cast<void*>(&VoidTarget)) && !S2EngineHooksRemovalComplete(),"premature reset retains native bindings");
    CHECK(!S2EngineHooksCanUnloadSync(S2Hook_CurrentTerminalPermit()),"removal requires a valid terminal permit");
    bool refused=false;
    dispatch=[&](int,void* view) {
        auto permit=S2Hook_CurrentTerminalPermit();
        refused=!S2EngineHooksUnloadSync(permit);
        S2_HookResetAll();
        uint32_t handle=0;
        CHECK(S2_HookReceiverHandle(view,&handle)==0,"reset inside callback preserves live view");
        return 0;
    };
    provider.Invoke(&VoidTarget,receiver);
    CHECK(refused,"terminal permit never removes its own active capsule");
    dispatch={};
    S2CheckedFunction<void,void*> marker(nullptr,nullptr);
    CHECK(marker.Configure(&MarkerTarget).Accepted(),"independent terminal marker installed");
    {
        auto observed=marker.Observe();
        const auto permit=S2Hook_CurrentTerminalPermit();
        CHECK(S2EngineHooksCanUnloadSync(permit),"terminal marker can preflight all declarative bindings");
        CHECK(S2EngineHooksUnloadSync(permit) && S2EngineHooksRemovalComplete(),"terminal inventory removes actual receipt IDs and completes");
        S2_HookArmBypass(0);
        S2_HookResetAll();
        CHECK(!S2Hook_BypassTake(0),"successful terminal reset clears bypass state");
    }
    CHECK(marker.BeginRemove(false) && marker.RemovalComplete(),"marker removal completes off-stack");
    CHECK(provider.entries.empty(),"all provider registrations removed exactly once");
}
} // namespace

int S2_AddressIsExecutable(const void* p) { return p!=nullptr; }
int S2_ModuleViewForAddress(const void*,const unsigned char**,size_t*,const unsigned char**,const unsigned char**) { return 0; }
uint32_t S2_EntityHandleFromPtr(void* p) { return p==receiver?42:S2_ENTITY_HANDLE_NONE; }
void* S2_ResolveEntity(int,int) { return nullptr; }
bool S2_EngineCallResolutionForAddress(const void* p,s2resolve::Resolution& out) {
    auto it=resolutions.find(p); out=it==resolutions.end()?s2resolve::Resolution{}:it->second; return it!=resolutions.end();
}

int main() {
    KHook::__exported__khook=&provider;
    S2Hook_SetOps({&Dispatch});
    const bool installed=Install(0,0,&VoidTarget) && Install(1,1,&Narrow) && Install(2,2,&Wide) &&
        Install(5,0,&OtherVoid);
    // The retired pickup-gate / HUD-click shape ids install nothing.
    char retired[256]{};
    CHECK(S2_HookInstall(3,3,reinterpret_cast<int64_t>(&OtherVoid),retired,sizeof retired)==-1 && std::strstr(retired,"unknown hook shape"),"retired shape 3 is unknown");
    CHECK(S2_HookInstall(4,4,reinterpret_cast<int64_t>(&OtherVoid),retired,sizeof retired)==-1 && std::strstr(retired,"unknown hook shape"),"retired shape 4 is unknown");
    if(installed) { MutationAndNesting(); Bypass(); RejectionAndLifecycle(); }
    char reason[256]{};
    CHECK(S2_HookInstall(6,0,reinterpret_cast<int64_t>(&MissingTarget),reason,sizeof reason)==-1 && std::strstr(reason,"image"),"missing original image is a named failure");
    ImageFor(reinterpret_cast<void*>(&BadWidth),true);
    CHECK(S2_HookInstall(6,1,reinterpret_cast<int64_t>(&BadWidth),reason,sizeof reason)==-1 && std::strstr(reason,"32-bit"),"width check reads original image, not live patched bytes");
    std::cout << "engine_hook_invocation: " << failures << " failure(s)\n";
    return failures?1:0;
}
