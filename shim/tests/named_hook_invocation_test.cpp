// Drives the production named callbacks through the pinned KHook wrappers. The provider
// substitutes only stock registration/JIT mechanics and the engine-specific operations table.
class CVariantDefaultAllocator;
template <typename A> class CVariantBase;
typedef CVariantBase<CVariantDefaultAllocator> CVariant;
#include "named_hooks.h"

#include <array>
#include <cstdint>
#include <cstring>
#include <functional>
#include <iostream>
#include <map>
#include <type_traits>
#include <vector>

static int failures = 0;
#define CHECK(c, why) do { if (!(c)) { std::cerr << "FAIL: " << why << "\n"; ++failures; } } while (0)
namespace KHook { IKHook* __exported__khook = nullptr; }

namespace {
struct Registration {
    void* address = nullptr;
    void* context = nullptr;
    void* removed = nullptr;
    void* pre = nullptr;
    void* post = nullptr;
    void** vtable = nullptr;
    int index = -1;
};
struct Frame {
    Registration* registration = nullptr;
    void* continuation = nullptr;
    KHook::Action action = KHook::Action::Ignore;
    alignas(16) std::array<unsigned char, 16> engine{};
    alignas(16) std::array<unsigned char, 16> override_value{};
    bool recalled = false;
    bool skipped = false;
};
class Provider final : public KHook::IKHook {
public:
    std::map<KHook::HookID_t, Registration> entries;
    KHook::HookID_t next = 100;
    Frame* frame = nullptr;
    void* removal_context = nullptr;
    bool fail_setup = false;

    KHook::HookID_t SetupHook(void* fn, void* ctx, void* removed, void* pre, void* post,
                              void*, void*, unsigned int, bool) override {
        if (fail_setup) return KHook::INVALID_HOOK;
        const auto id=next++;
        entries.emplace(id,Registration{fn,ctx,removed,pre,post,nullptr,-1});
        return id;
    }
    KHook::HookID_t SetupVirtualHook(void** vt, int index, void* ctx, void* removed,
                                     void* pre, void* post, void*, void*, unsigned int, bool) override {
        if (fail_setup || !vt || index<0) return KHook::INVALID_HOOK;
        const auto id=next++;
        entries.emplace(id,Registration{vt[index],ctx,removed,pre,post,vt,index});
        return id;
    }
    void RemoveHook(KHook::HookID_t id, bool async,
                    void (*done)(KHook::HookID_t,void*), void* done_ctx) override {
        CHECK(!async,"terminal removal is synchronous");
        auto it=entries.find(id);
        if (it==entries.end()) return;
        removal_context=it->second.context;
        reinterpret_cast<void (*)(KHook::HookID_t)>(it->second.removed)(id);
        removal_context=nullptr;
        entries.erase(it);
        if (done) done(id,done_ctx);
    }
    void* GetContextPtr() override {
        return removal_context ? removal_context : (frame ? frame->registration->context : nullptr);
    }
    void* GetOriginalFunction() override { return frame->registration->address; }
    void* GetOriginalValuePtr() override { return frame->engine.data(); }
    void* GetOverrideValuePtr() override { return frame->override_value.data(); }
    void* GetCurrentValuePtr(bool) override {
        return frame->action>=KHook::Action::Override ? frame->override_value.data() : frame->engine.data();
    }
    void DestroyReturnValue() override {}
    void* FindOriginal(void* fn) override { return fn; }
    void* FindOriginalVirtual(void** vt,int index) override { return vt[index]; }
    void* LookupSignature(void*,std::size_t,const char*) override { return nullptr; }
    bool WasOriginalFunctionSkipped() override { return frame->skipped; }
    void* DoRecall(KHook::Action action,void* value,std::size_t size,void*,void*) override {
        frame->recalled=true;
        SaveReturnValue(action,value,size,nullptr,nullptr,false);
        return frame->continuation;
    }
    void SaveReturnValue(KHook::Action action,void* value,std::size_t size,void*,void*,bool original) override {
        CHECK(size<=16,"fixture return storage is wide enough");
        if (original && size) std::memcpy(frame->engine.data(),value,size);
        if (action>frame->action) {
            frame->action=action;
            if (size) std::memcpy(frame->override_value.data(),value,size);
        }
    }
    Registration* Find(void* fn) {
        for (auto& [id,e] : entries) if (!e.vtable && e.address==fn) return &e;
        return nullptr;
    }
    Registration* FindVirtual(void** vt,int index) {
        for (auto& [id,e] : entries) if (e.vtable==vt && e.index==index) return &e;
        return nullptr;
    }

    template <typename R,typename... A> static R Continue(A... args);
    template <typename R,typename... A> R InvokeEntry(Registration* entry,A... args) {
        CHECK(entry,"target was registered through the checked typed helper");
        if (!entry) { if constexpr (!std::is_void_v<R>) return R{}; else return; }
        Frame invocation; invocation.registration=entry;
        invocation.continuation=reinterpret_cast<void*>(&Continue<R,A...>);
        Frame* saved=frame; frame=&invocation;
        reinterpret_cast<R (*)(A...)>(entry->pre)(args...);
        if (!invocation.recalled) Continue<R,A...>(args...);
        if constexpr (!std::is_void_v<R>) {
            R result{};
            std::memcpy(&result,GetCurrentValuePtr(false),sizeof result);
            frame=saved;
            return result;
        } else {
            frame=saved;
            return;
        }
    }
    template <typename R,typename... A> R Invoke(R (*fn)(A...),A... args) {
        return InvokeEntry<R,A...>(Find(reinterpret_cast<void*>(fn)),args...);
    }
};
Provider provider;

template <typename R,typename... A> R Provider::Continue(A... args) {
    auto& p=provider;
    p.frame->skipped=p.frame->action==KHook::Action::Supersede;
    if (!p.frame->skipped) {
        auto fn=reinterpret_cast<R (*)(A...)>(p.frame->registration->address);
        if constexpr (std::is_void_v<R>) fn(args...);
        else {
            R value=fn(args...);
            p.SaveReturnValue(KHook::Action::Ignore,&value,sizeof value,nullptr,nullptr,true);
        }
    }
    reinterpret_cast<R (*)(A...)>(p.frame->registration->post)(args...);
    if constexpr (!std::is_void_v<R>) {
        R result{}; std::memcpy(&result,p.GetCurrentValuePtr(false),sizeof result); return result;
    }
}

extern void* outer_victim;
extern void* outer_info;
extern void* inner_victim;
extern void* inner_info;
extern void* outer_manifest;
extern void* inner_manifest;
extern std::array<unsigned char,0xa0> nested_cmd;
int damage_originals=0, chat_originals=0, output_originals=0, usercmd_originals=0, precache_originals=0;
int64_t DamageTarget(void* victim,void* info,void*,void*) {
    ++damage_originals;
    if (victim==inner_victim) {
        CHECK(S2NamedDamageInfo()==outer_info && S2NamedDamageVictim()==outer_victim,
              "nested damage original sees only the still-live outer callback frame");
    } else {
        CHECK(S2NamedDamageInfo()==nullptr && S2NamedDamageVictim()==nullptr,
              "damage frame does not span its own original execution");
    }
    CHECK(victim && info,"damage full-width pointers reach original");
    return INT64_C(0x1122334455667788);
}
void ChatTarget(void*,void*,bool,int,const char*) { ++chat_originals; }
void OutputTarget(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    ++output_originals;
}
int UsercmdTarget(void*,void* commands,int,bool paused,float margin) {
    ++usercmd_originals;
    if (commands==nested_cmd.data())
        CHECK(S2NamedCurrentUsercmd()!=nullptr,"nested usercmd original restores the live outer frame");
    else
        CHECK(S2NamedCurrentUsercmd()==nullptr,"usercmd frame does not span its own original execution");
    CHECK(paused && margin==2.5f,"usercmd bool and float preserve ABI positions");
    return 37;
}
struct Receiver { void** vptr; int tag; };
void PrecacheTarget(Receiver* receiver,void* manifest) {
    ++precache_originals;
    CHECK(receiver && receiver->tag>0,"precache original receives actual receiver");
    if (manifest==inner_manifest)
        CHECK(S2NamedCurrentPrecacheManifest()==outer_manifest,
              "nested precache original restores the live outer manifest frame");
    else
        CHECK(S2NamedCurrentPrecacheManifest()==nullptr,
              "precache frame does not span its own original execution");
}

void* outer_victim=reinterpret_cast<void*>(uintptr_t{0x1111222233334444});
void* outer_info=reinterpret_cast<void*>(uintptr_t{0x5555666677778888});
void* inner_victim=reinterpret_cast<void*>(uintptr_t{0x9999aaaabbbbcccc});
void* inner_info=reinterpret_cast<void*>(uintptr_t{0xddddeeeeffff1111});
bool in_damage_pre=false,in_damage_post=false;
int damage_pre_calls=0,damage_post_calls=0;
bool damage_terminal_refused=false;
void DamagePreOp() {
    ++damage_pre_calls;
    void* victim=S2NamedDamageVictim(); void* info=S2NamedDamageInfo();
    CHECK(victim && info,"damage PRE exposes this callback's pointers");
    const auto permit=S2Hook_CurrentTerminalPermit();
    if (permit.IsValid()) damage_terminal_refused=!S2NamedHooksCanUnloadSync(permit);
    if (!in_damage_pre && victim==outer_victim) {
        in_damage_pre=true;
        CHECK(provider.Invoke(&DamageTarget,inner_victim,inner_info,static_cast<void*>(nullptr),static_cast<void*>(nullptr))==INT64_C(0x1122334455667788),
              "nested damage PRE preserves the original return");
        CHECK(S2NamedDamageVictim()==outer_victim && S2NamedDamageInfo()==outer_info,
              "nested damage PRE restores outer pointers");
        in_damage_pre=false;
    }
}
void DamagePostOp() {
    ++damage_post_calls;
    void* victim=S2NamedDamageVictim(); void* info=S2NamedDamageInfo();
    CHECK(victim && info,"damage POST exposes this callback's own pointers");
    if (!in_damage_post && victim==outer_victim) {
        in_damage_post=true;
        provider.Invoke(&DamageTarget,inner_victim,inner_info,static_cast<void*>(nullptr),static_cast<void*>(nullptr));
        CHECK(S2NamedDamageVictim()==outer_victim && S2NamedDamageInfo()==outer_info,
              "nested damage POST restores outer pointers");
        in_damage_post=false;
    }
}

int chat_result=0,chat_calls=0;
int ChatOp(void* controller,void* command,bool team,int number,const char* text) {
    ++chat_calls;
    CHECK(controller==outer_victim && command==outer_info && team && number==19 &&
              std::strcmp(text,"opaque")==0,"chat callback preserves exact ABI arguments");
    return chat_result;
}
int output_result=0,output_calls=0;
int OutputOp(CEntityIOOutput* out,CEntityInstance* act,CEntityInstance* caller,
             const CVariant* value,float delay,void* u1,char* u2) {
    ++output_calls;
    CHECK(out && act && caller && value && delay==1.25f && u1 && u2,
          "output callback preserves exact ABI arguments");
    return output_result;
}

bool in_usercmd=false;
int usercmd_dispatches=0,usercmd_neutralized=0;
std::array<unsigned char,0xa0> nested_cmd{};
int UsercmdSlot(void* self) { CHECK(self==outer_victim,"usercmd derives slot from actual receiver"); return 7; }
int UsercmdDispatch(int slot) {
    ++usercmd_dispatches;
    CHECK(slot==7,"usercmd dispatch receives derived slot");
    auto* cmd=static_cast<unsigned char*>(S2NamedCurrentUsercmd());
    CHECK(cmd,"usercmd dispatch has a scoped command");
    const int verdict=*cmd;
    *cmd=static_cast<unsigned char>(*cmd+10);
    if (!in_usercmd && verdict==1) {
        in_usercmd=true; nested_cmd[0x10]=3;
        CHECK(provider.Invoke(&UsercmdTarget,outer_victim,static_cast<void*>(nested_cmd.data()),1,true,2.5f)==37,
              "nested usercmd preserves engine return");
        CHECK(S2NamedCurrentUsercmd()==cmd,"nested usercmd restores outer command");
        in_usercmd=false;
    }
    return verdict;
}
void UsercmdNeutralize() {
    ++usercmd_neutralized;
    auto* cmd=static_cast<unsigned char*>(S2NamedCurrentUsercmd());
    CHECK(cmd,"neutralization runs inside current command frame");
    *cmd=0;
}

Receiver* expected_receiver=nullptr;
void* outer_manifest=reinterpret_cast<void*>(uintptr_t{0x1010101010101010});
void* inner_manifest=reinterpret_cast<void*>(uintptr_t{0x2020202020202020});
bool in_precache=false;
int precache_calls=0;
void PrecacheOp() {
    ++precache_calls;
    CHECK(S2NamedCurrentPrecacheManifest(),"precache callback exposes current manifest");
    if (!in_precache && S2NamedCurrentPrecacheManifest()==outer_manifest) {
        in_precache=true;
        auto* entry=provider.FindVirtual(expected_receiver->vptr,0);
        provider.InvokeEntry<void,Receiver*,void*>(entry,expected_receiver,inner_manifest);
        CHECK(S2NamedCurrentPrecacheManifest()==outer_manifest,
              "nested precache restores outer manifest");
        in_precache=false;
    }
}

int damage_pre_ignore=0,damage_post_ignore=0,chat_ignore=0,chat_supersede=0;
int output_ignore=0,output_supersede=0,usercmd_ignore=0,precache_ignore=0;
void ObserveAction(S2NamedHookSite site,bool post,KHook::Action action) {
    if (site==S2NamedHookSite::Damage && !post && action==KHook::Action::Ignore) ++damage_pre_ignore;
    if (site==S2NamedHookSite::Damage && post && action==KHook::Action::Ignore) ++damage_post_ignore;
    if (site==S2NamedHookSite::Chat && action==KHook::Action::Ignore) ++chat_ignore;
    if (site==S2NamedHookSite::Chat && action==KHook::Action::Supersede) ++chat_supersede;
    if (site==S2NamedHookSite::Output && action==KHook::Action::Ignore) ++output_ignore;
    if (site==S2NamedHookSite::Output && action==KHook::Action::Supersede) ++output_supersede;
    if (site==S2NamedHookSite::Usercmd && action==KHook::Action::Ignore) ++usercmd_ignore;
    if (site==S2NamedHookSite::Precache && action==KHook::Action::Ignore) ++precache_ignore;
}

S2CheckedFunction<void,void*>* terminal_binding=nullptr;
bool terminal_can=false,terminal_removed=false,terminal_complete=false;
void TerminalTarget(void*) {}
KHook::Return<void> TerminalPre(void*) {
    auto observed=terminal_binding->Observe();
    const auto permit=S2Hook_CurrentTerminalPermit();
    terminal_can=permit.IsValid() && S2NamedHooksCanUnloadSync(permit);
    if (terminal_can) terminal_removed=S2NamedHooksUnloadSync(permit);
    terminal_complete=S2NamedHooksRemovalComplete();
    return {KHook::Action::Ignore};
}

void ConfigureAndInvoke() {
    S2NamedHookOps ops;
    ops.damage_pre=&DamagePreOp; ops.damage_post=&DamagePostOp;
    ops.chat=&ChatOp; ops.output=&OutputOp;
    ops.usercmd_slot=&UsercmdSlot; ops.usercmd_dispatch=&UsercmdDispatch;
    ops.usercmd_neutralize=&UsercmdNeutralize; ops.precache=&PrecacheOp;
    ops.observe_action=&ObserveAction;
    S2NamedHooksSetOps(ops);

    const auto damage=S2NamedConfigureDamage(reinterpret_cast<void*>(&DamageTarget));
    const auto chat=S2NamedConfigureChat(reinterpret_cast<void*>(&ChatTarget));
    const auto output=S2NamedConfigureOutput(reinterpret_cast<void*>(&OutputTarget));
    CHECK(damage.Accepted() && chat.Accepted() && output.Accepted(),"three eager named hooks are accepted");
    CHECK(S2NamedHookSnapshot(S2NamedHookSite::Damage).state==S2HookState::Pending,
          "accepted damage receipt begins Pending");

    provider.fail_setup=true;
    S2NamedSetUsercmdTarget(reinterpret_cast<void*>(&UsercmdTarget));
    CHECK(!S2NamedInstallUsercmd().Accepted(),"lazy usercmd reports INVALID_HOOK setup failure");
    provider.fail_setup=false;
    const auto usercmd=S2NamedInstallUsercmd();
    CHECK(usercmd.Accepted(),"lazy usercmd retry is accepted");
    const auto next_after_usercmd=provider.next;
    CHECK(S2NamedInstallUsercmd().id==usercmd.id && provider.next==next_after_usercmd,
          "lazy usercmd installation is idempotent");

    static void* vtable[1]={reinterpret_cast<void*>(&PrecacheTarget)};
    s2resolve::VirtualSlotResolution resolved;
    resolved.target.address=reinterpret_cast<uintptr_t>(&PrecacheTarget);
    resolved.target.recipe="fixture"; resolved.target.validation_receipt="structural";
    resolved.vtable=vtable; resolved.vtable_index=0;
    const auto precache=S2NamedConfigurePrecache(resolved);
    CHECK(precache.Accepted(),"checked precache global binding is accepted");
    Receiver receiver{vtable,1}; expected_receiver=&receiver;

    CHECK(provider.Invoke(&DamageTarget,outer_victim,outer_info,static_cast<void*>(nullptr),static_cast<void*>(nullptr))==INT64_C(0x1122334455667788),
          "damage keeps the full-width original result");
    CHECK(damage_pre_calls==3 && damage_post_calls==3 && damage_originals==3,
          "nested damage runs PRE/original/POST once per invocation");
    CHECK(damage_terminal_refused,"named hook refuses removal from its own active capsule");
    CHECK(S2NamedHookSnapshot(S2NamedHookSite::Damage).state==S2HookState::Active,
          "first observed damage callback promotes Pending to Active");
    CHECK(!S2NamedDamageInfo() && !S2NamedDamageVictim(),"damage pointers expire after callback");

    for (int result : {0,1}) { chat_result=result; provider.Invoke(&ChatTarget,outer_victim,outer_info,true,19,"opaque"); }
    for (int result : {2,3}) { chat_result=result; provider.Invoke(&ChatTarget,outer_victim,outer_info,true,19,"opaque"); }
    CHECK(chat_calls==4 && chat_originals==1,"chat suppresses every nonzero result and otherwise runs original once");

    auto* out=reinterpret_cast<CEntityIOOutput*>(uintptr_t{0x1111});
    auto* act=reinterpret_cast<CEntityInstance*>(uintptr_t{0x2222});
    auto* caller=reinterpret_cast<CEntityInstance*>(uintptr_t{0x3333});
    auto* value=reinterpret_cast<const CVariant*>(uintptr_t{0x4444});
    char marker='x';
    for (int result : {0,1,2,3}) {
        output_result=result;
        provider.Invoke(&OutputTarget,out,act,caller,value,1.25f,outer_victim,&marker);
    }
    CHECK(output_calls==4 && output_originals==2,"output suppresses at >=2 and runs original once below threshold");

    std::array<unsigned char,0x130> commands{}; commands[0x10]=1; commands[0xa0]=2;
    CHECK(provider.Invoke(&UsercmdTarget,outer_victim,static_cast<void*>(commands.data()),0,true,2.5f)==37,
          "empty usercmd batch preserves nonzero original return");
    CHECK(usercmd_dispatches==0,"empty usercmd batch does not dispatch");
    CHECK(provider.Invoke(&UsercmdTarget,outer_victim,static_cast<void*>(commands.data()),2,true,2.5f)==37,
          "multi-command usercmd batch preserves nonzero original return");
    CHECK(usercmd_dispatches==3 && usercmd_neutralized==2,
          "shipped batch loop dispatches every command and neutralizes only >=2");
    CHECK(commands[0x10]==11 && commands[0xa0]==0 && nested_cmd[0x10]==0,
          "usercmd mutation is in-place and neutralization affects the current command");
    CHECK(usercmd_originals==3 && !S2NamedCurrentUsercmd(),
          "usercmd original runs once per outer/nested invocation and pointer expires");

    auto* entry=provider.FindVirtual(vtable,0);
    provider.InvokeEntry<void,Receiver*,void*>(entry,&receiver,outer_manifest);
    CHECK(precache_calls==2 && precache_originals==2,
          "precache nested dispatch and each original run exactly once");
    CHECK(!S2NamedCurrentPrecacheManifest(),"precache manifest expires after callback");
    void* other_vtable[1]={reinterpret_cast<void*>(&PrecacheTarget)};
    Receiver other{other_vtable,2};
    provider.InvokeEntry<void,Receiver*,void*>(entry,&other,outer_manifest);
    CHECK(precache_calls==2 && precache_originals==3,
          "different-vtable receiver is filtered while its original still runs");
    CHECK(damage_pre_ignore==3 && damage_post_ignore==3 && chat_ignore==1 && chat_supersede==3 &&
              output_ignore==2 && output_supersede==2 && usercmd_ignore==3 && precache_ignore==2,
          "production callbacks publish their actual accepted action and damage phase");

    S2CheckedFunction<void,void*> terminal(&TerminalPre,nullptr);
    terminal_binding=&terminal;
    CHECK(terminal.Configure(&TerminalTarget).Accepted(),"terminal fixture binding is accepted");
    S2Hook_SetLifecycle(S2HookLifecycle::Retiring);
    const int pre_before=damage_pre_calls;
    provider.Invoke(&DamageTarget,outer_victim,outer_info,static_cast<void*>(nullptr),static_cast<void*>(nullptr));
    CHECK(damage_pre_calls==pre_before && !S2NamedDamageInfo(),
          "retiring callback rejects dispatch without exposing borrowed pointers");
    provider.Invoke(&TerminalTarget,outer_victim);
    CHECK(terminal_can && terminal_removed && terminal_complete,
          "foreign terminal capsule preflights, removes, and completes named inventory");
    CHECK(provider.FindVirtual(vtable,0)==nullptr,"precache retirement removes the retained vtable hook");
    S2Hook_SetLifecycle(S2HookLifecycle::Ready);
}
}

int main() {
    KHook::__exported__khook=&provider;
    ConfigureAndInvoke();
    if (failures) { std::cerr << "named_hook_invocation_test: " << failures << " failures\n"; return 1; }
    std::cout << "named_hook_invocation_test: all passed\n";
}
