#include "named_fixture.h"
#include "named_hooks.h"
#include "original_module.h"
#include "khook_map.h"

#include <array>
#include <cstring>
#include <dlfcn.h>

extern "C" void S2ProbeNamedInvokeVirtual(void*,int,void*);
extern "C" int64_t S2ProbeNamedDamageTarget(void*,void*,void*,void*);
extern "C" void S2ProbeNamedChatTarget(void*,void*,bool,int,const char*);
extern "C" void S2ProbeNamedOutputTarget(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                                          const CVariant*,float,void*,char*);
extern "C" int S2ProbeNamedUsercmdTarget(void*,void*,int,bool,float);

namespace {
enum class Mode { Idle,Damage,Chat,Output,Usercmd,Precache };
Mode mode=Mode::Idle;
s2khook::NamedSnapshot observation;
bool installed=false;
bool damage_pre_nested=false,damage_post_nested=false,usercmd_nested=false,precache_nested=false;
bool removal_refusal_checked=false;
int chat_verdict=0,output_verdict=0,precache_index=-1;
size_t chat_action_index=0,chat_post_index=0,output_action_index=0,output_post_index=0;
uint64_t invocation_map_generation=0;
int receiver_token=0;
void* outer_victim=&receiver_token;
void* outer_info=reinterpret_cast<void*>(uintptr_t{0x5555666677778888});
void* inner_victim=reinterpret_cast<void*>(uintptr_t{0x9999aaaabbbbcccc});
void* inner_info=reinterpret_cast<void*>(uintptr_t{0xddddeeeeffff1111});
void* outer_manifest=reinterpret_cast<void*>(uintptr_t{0x1010101010101010});
void* inner_manifest=reinterpret_cast<void*>(uintptr_t{0x2020202020202020});

class ProbePrecache {
public:
    virtual void Run(void* manifest);
    int tag=17;
};
class OtherPrecache {
public:
    virtual void Run(void*) { if (mode==Mode::Precache) ++observation.precache_filtered_original; }
};
ProbePrecache precache_object;
OtherPrecache other_object;

using DamageFn=int64_t (*)(void*,void*,void*,void*);
using ChatFn=void (*)(void*,void*,bool,int,const char*);
using OutputFn=void (*)(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*);
using UsercmdFn=int (*)(void*,void*,int,bool,float);
DamageFn volatile call_damage=reinterpret_cast<DamageFn>(&S2ProbeNamedDamageTarget);
ChatFn volatile call_chat=&S2ProbeNamedChatTarget;
OutputFn volatile call_output=&S2ProbeNamedOutputTarget;
UsercmdFn volatile call_usercmd=&S2ProbeNamedUsercmdTarget;

KHook::Return<void> ChatPeerBefore(void*,void*,bool,int,const char*);
KHook::Return<void> ChatPeerAfter(void*,void*,bool,int,const char*);
KHook::Return<void> ChatPeerPost(void*,void*,bool,int,const char*);
KHook::Return<int64_t> DamageObservePost(void*,void*,void*,void*);
KHook::Return<void> OutputObservePost(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                                      const CVariant*,float,void*,char*);
KHook::Return<int> UsercmdObservePost(void*,void*,int,bool,float);
KHook::Return<void> VirtualPeerBefore(ProbePrecache*,void*);
KHook::Return<void> VirtualPeerAfter(ProbePrecache*,void*);
KHook::Return<void> VirtualPeerPost(ProbePrecache*,void*);
S2CheckedFunction<int64_t,void*,void*,void*,void*> peer_damage_observer(nullptr,&DamageObservePost);
S2CheckedFunction<void,void*,void*,bool,int,const char*> peer_chat_before(&ChatPeerBefore,nullptr);
S2CheckedFunction<void,void*,void*,bool,int,const char*> peer_chat_after(&ChatPeerAfter,&ChatPeerPost);
S2CheckedFunction<void,CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*>
    peer_output_observer(nullptr,&OutputObservePost);
S2CheckedFunction<int,void*,void*,int,bool,float> peer_usercmd_observer(nullptr,&UsercmdObservePost);
S2CheckedVirtual<ProbePrecache,void,void*> peer_virtual_before(&VirtualPeerBefore,nullptr);
S2CheckedVirtual<ProbePrecache,void,void*> peer_virtual_after(&VirtualPeerAfter,&VirtualPeerPost);
bool peer_virtual_before_filter=false,peer_virtual_after_filter=false;

KHook::Return<void> ChatPeerBefore(void*,void*,bool,int,const char*) {
    auto observed=peer_chat_before.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Chat) {
        ++observation.chat_peer_before;
        observation.chat_peer_order=observation.chat_peer_order*10+1;
    }
    return S2_Ignore();
}
KHook::Return<void> ChatPeerAfter(void*,void*,bool,int,const char*) {
    auto observed=peer_chat_after.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Chat) {
        ++observation.chat_peer_after;
        observation.chat_peer_order=observation.chat_peer_order*10+3;
    }
    return S2_Ignore();
}
KHook::Return<void> ChatPeerPost(void*,void*,bool,int,const char*) {
    auto observed=peer_chat_after.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Chat) {
        const int skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0;
        if (chat_post_index<observation.chat_skipped.size()) observation.chat_skipped[chat_post_index]=skipped;
        ++chat_post_index;
        ++observation.chat_post_observed;
    }
    return S2_Ignore();
}
KHook::Return<int64_t> DamageObservePost(void*,void*,void*,void*) {
    auto observed=peer_damage_observer.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore(int64_t{0});
    if (mode==Mode::Damage) {
        const auto current=KHook::GetCurrentReturn<int64_t>();
        observation.damage_current_return=current;
        ++observation.damage_post_observed;
        if (current==INT64_C(0x1122334455667788)) ++observation.damage_current_return_matches;
        if (KHook::WasOriginalFunctionSkipped()) ++observation.damage_skipped;
    }
    return S2_Ignore(int64_t{0});
}
KHook::Return<void> OutputObservePost(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                                      const CVariant*,float,void*,char*) {
    auto observed=peer_output_observer.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Output) {
        const int skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0;
        if (output_post_index<observation.output_skipped.size()) observation.output_skipped[output_post_index]=skipped;
        ++output_post_index;
        ++observation.output_post_observed;
    }
    return S2_Ignore();
}
KHook::Return<int> UsercmdObservePost(void*,void*,int,bool,float) {
    auto observed=peer_usercmd_observer.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore(0);
    if (mode==Mode::Usercmd) {
        const int current=KHook::GetCurrentReturn<int>();
        observation.usercmd_current_return=current;
        ++observation.usercmd_post_observed;
        if (current==37) ++observation.usercmd_current_return_matches;
        if (KHook::WasOriginalFunctionSkipped()) ++observation.usercmd_skipped;
    }
    return S2_Ignore(0);
}
KHook::Return<void> VirtualPeerBefore(ProbePrecache* receiver,void*) {
    auto observed=peer_virtual_before.Observe(receiver);
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Precache) {
        ++observation.precache_peer_before;
        observation.precache_peer_order=observation.precache_peer_order*10+1;
    }
    return S2_Ignore();
}
KHook::Return<void> VirtualPeerAfter(ProbePrecache* receiver,void*) {
    auto observed=peer_virtual_after.Observe(receiver);
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Precache) {
        ++observation.precache_peer_after;
        observation.precache_peer_order=observation.precache_peer_order*10+3;
    }
    return S2_Ignore();
}
KHook::Return<void> VirtualPeerPost(ProbePrecache* receiver,void*) {
    auto observed=peer_virtual_after.Observe(receiver);
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    if (mode==Mode::Precache) {
        ++observation.precache_post_observed;
        if (KHook::WasOriginalFunctionSkipped()) ++observation.precache_skipped;
    }
    return S2_Ignore();
}

std::array<S2CheckedBindingOps*,7> PeerInventory() {
    return {{&peer_damage_observer,&peer_chat_before,&peer_chat_after,&peer_output_observer,
             &peer_usercmd_observer,&peer_virtual_before,&peer_virtual_after}};
}

void DamagePreOp() {
    ++observation.damage_pre;
    if (!removal_refusal_checked && S2NamedDamageVictim()==outer_victim) {
        removal_refusal_checked=true;
        const auto permit=S2Hook_CurrentTerminalPermit();
        const bool allowed=permit.IsValid() &&
            S2HookInventoryCanRemoveSync(PeerInventory(),permit) && S2NamedHooksCanUnloadSync(permit);
        observation.removal.active_refused=permit.IsValid() && !allowed ? 1 : 0;
    }
    if (!damage_pre_nested && S2NamedDamageVictim()==outer_victim) {
        damage_pre_nested=true;
        call_damage(inner_victim,inner_info,nullptr,nullptr);
        if (S2NamedDamageVictim()==outer_victim && S2NamedDamageInfo()==outer_info)
            ++observation.damage_nested_restored;
        damage_pre_nested=false;
    }
}
void DamagePostOp() {
    ++observation.damage_post;
    if (!damage_post_nested && S2NamedDamageVictim()==outer_victim) {
        damage_post_nested=true;
        call_damage(inner_victim,inner_info,nullptr,nullptr);
        if (S2NamedDamageVictim()==outer_victim && S2NamedDamageInfo()==outer_info)
            ++observation.damage_nested_restored;
        damage_post_nested=false;
    }
}
int ChatOp(void*,void*,bool,int,const char*) {
    if (mode==Mode::Chat) {
        observation.chat_peer_order=observation.chat_peer_order*10+2;
        ++observation.chat_dispatch;
    }
    return chat_verdict;
}
int OutputOp(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    if (mode==Mode::Output) ++observation.output_dispatch;
    return output_verdict;
}
int UsercmdSlot(void*) { return 7; }
std::array<unsigned char,0xa0> nested_command{};
int UsercmdDispatch(int) {
    ++observation.usercmd_dispatch;
    auto* command=static_cast<unsigned char*>(S2NamedCurrentUsercmd());
    const int result=command ? *command : 0;
    if (command) *command=static_cast<unsigned char>(*command+10);
    if (!usercmd_nested && result==1) {
        usercmd_nested=true;
        nested_command[0x10]=3;
        call_usercmd(outer_victim,nested_command.data(),1,true,2.5f);
        if (S2NamedCurrentUsercmd()==command) ++observation.usercmd_nested_restored;
        usercmd_nested=false;
    }
    return result;
}
void UsercmdNeutralize() {
    ++observation.usercmd_neutralized;
    auto* command=static_cast<unsigned char*>(S2NamedCurrentUsercmd());
    if (command) *command=0;
}
void PrecacheOp() {
    ++observation.precache_dispatch;
    observation.precache_peer_order=observation.precache_peer_order*10+2;
    if (observation.precache_observed_map_generation==0)
        observation.precache_observed_map_generation=invocation_map_generation;
    if (observation.precache_observed_map_generation==invocation_map_generation)
        ++observation.precache_generation_observations;
    if (!precache_nested && S2NamedCurrentPrecacheManifest()==outer_manifest) {
        precache_nested=true;
        S2ProbeNamedInvokeVirtual(&precache_object,precache_index,inner_manifest);
        if (S2NamedCurrentPrecacheManifest()==outer_manifest)
            ++observation.precache_nested_restored;
        precache_nested=false;
    }
}

void ObserveAction(S2NamedHookSite site,bool post,KHook::Action action) {
    const int value=static_cast<int>(action);
    if (mode==Mode::Damage && site==S2NamedHookSite::Damage && action==KHook::Action::Ignore) {
        if (post) ++observation.damage_post_ignore; else ++observation.damage_pre_ignore;
    } else if (mode==Mode::Chat && site==S2NamedHookSite::Chat) {
        if (chat_action_index<observation.chat_actions.size()) observation.chat_actions[chat_action_index]=value;
        ++chat_action_index;
    } else if (mode==Mode::Output && site==S2NamedHookSite::Output) {
        if (output_action_index<observation.output_actions.size()) observation.output_actions[output_action_index]=value;
        ++output_action_index;
    } else if (mode==Mode::Usercmd && site==S2NamedHookSite::Usercmd && action==KHook::Action::Ignore) {
        ++observation.usercmd_ignore;
    } else if (mode==Mode::Precache && site==S2NamedHookSite::Precache && action==KHook::Action::Ignore) {
        ++observation.precache_ignore;
    }
}

bool VerifyEntry(const s2original::Image& image,const void* target,const std::array<uint8_t,8>& marker) {
    std::array<uint8_t,8> bytes{};
    return image.executable(reinterpret_cast<uintptr_t>(target),bytes.size()) &&
        image.read(reinterpret_cast<uintptr_t>(target),bytes.data(),bytes.size()) && bytes==marker;
}
}

extern "C" __attribute__((noinline)) int64_t S2ProbeNamedDamageBody(void*,void*,void*,void*) {
    if (mode==Mode::Damage) ++observation.damage_original;
    return INT64_C(0x1122334455667788);
}
extern "C" __attribute__((noinline)) void S2ProbeNamedChatBody(void*,void*,bool,int,const char*) {
    if (mode==Mode::Chat) ++observation.chat_original;
}
extern "C" __attribute__((noinline)) void S2ProbeNamedOutputBody(
    CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    if (mode==Mode::Output) ++observation.output_original;
}
extern "C" __attribute__((noinline)) int S2ProbeNamedUsercmdBody(void*,void*,int,bool,float) {
    if (mode==Mode::Usercmd) ++observation.usercmd_original;
    return 37;
}

#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int64_t S2ProbeNamedDamageTarget(void*,void*,void*,void*) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x44,0x35\n\tjmp S2ProbeNamedDamageBody");
}
extern "C" __attribute__((naked,noinline)) void S2ProbeNamedChatTarget(void*,void*,bool,int,const char*) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x43,0x35\n\tjmp S2ProbeNamedChatBody");
}
extern "C" __attribute__((naked,noinline)) void S2ProbeNamedOutputTarget(
    CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x4f,0x35\n\tjmp S2ProbeNamedOutputBody");
}
extern "C" __attribute__((naked,noinline)) int S2ProbeNamedUsercmdTarget(void*,void*,int,bool,float) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x55,0x35\n\tjmp S2ProbeNamedUsercmdBody");
}
#else
extern "C" int64_t S2ProbeNamedDamageTarget(void* a,void* b,void* c,void* d) { return S2ProbeNamedDamageBody(a,b,c,d); }
extern "C" void S2ProbeNamedChatTarget(void* a,void* b,bool c,int d,const char* e) { S2ProbeNamedChatBody(a,b,c,d,e); }
extern "C" void S2ProbeNamedOutputTarget(CEntityIOOutput* a,CEntityInstance* b,CEntityInstance* c,const CVariant* d,float e,void* f,char* g) { S2ProbeNamedOutputBody(a,b,c,d,e,f,g); }
extern "C" int S2ProbeNamedUsercmdTarget(void* a,void* b,int c,bool d,float e) { return S2ProbeNamedUsercmdBody(a,b,c,d,e); }
#endif

void ProbePrecache::Run(void*) {
    if (mode==Mode::Precache) {
        ++observation.precache_original;
        if (this==&precache_object) ++observation.precache_receiver_ok;
    }
}

bool S2ProbeNamedInstall(std::string& reason) {
    if (installed) return true;
    Dl_info loaded{};
    if (!dladdr(reinterpret_cast<void*>(&S2ProbeNamedDamageTarget),&loaded) || !loaded.dli_fname) {
        reason="controlled named target module not found"; return false;
    }
    auto image=s2original::OpenLoadedModule(loaded.dli_fname,reason);
    if (!image) return false;
    const std::array<const void*,4> targets{{reinterpret_cast<void*>(&S2ProbeNamedDamageTarget),
        reinterpret_cast<void*>(&S2ProbeNamedChatTarget),reinterpret_cast<void*>(&S2ProbeNamedOutputTarget),
        reinterpret_cast<void*>(&S2ProbeNamedUsercmdTarget)}};
    const std::array<std::array<uint8_t,8>,4> markers{{
        {{0x0f,0x1f,0x84,0x00,0x53,0x32,0x44,0x35}},
        {{0x0f,0x1f,0x84,0x00,0x53,0x32,0x43,0x35}},
        {{0x0f,0x1f,0x84,0x00,0x53,0x32,0x4f,0x35}},
        {{0x0f,0x1f,0x84,0x00,0x53,0x32,0x55,0x35}}}};
    for (size_t i=0;i<targets.size();++i) if (!VerifyEntry(*image,targets[i],markers[i])) {
        reason="controlled named target marker not present in verified image"; return false;
    }
    S2NamedHookOps ops;
    ops.damage_pre=&DamagePreOp; ops.damage_post=&DamagePostOp; ops.chat=&ChatOp; ops.output=&OutputOp;
    ops.usercmd_slot=&UsercmdSlot; ops.usercmd_dispatch=&UsercmdDispatch;
    ops.usercmd_neutralize=&UsercmdNeutralize; ops.precache=&PrecacheOp;
    ops.observe_action=&ObserveAction;
    S2NamedHooksSetOps(ops);

    if (!S2NamedConfigureDamage(targets[0]).Accepted()) { reason="damage setup rejected"; return false; }
    if (!peer_damage_observer.Configure(targets[0]).Accepted()) { reason="damage observer rejected"; return false; }
    if (!peer_chat_before.Configure(targets[1]).Accepted()) { reason="before chat peer rejected"; return false; }
    if (!S2NamedConfigureChat(targets[1]).Accepted()) { reason="chat setup rejected"; return false; }
    if (!peer_chat_after.Configure(targets[1]).Accepted()) { reason="after chat peer rejected"; return false; }
    if (!S2NamedConfigureOutput(targets[2]).Accepted()) { reason="output setup rejected"; return false; }
    if (!peer_output_observer.Configure(targets[2]).Accepted()) { reason="output observer rejected"; return false; }
    S2NamedSetUsercmdTarget(targets[3]);
    if (!S2NamedInstallUsercmd().Accepted()) { reason="usercmd setup rejected"; return false; }
    if (!peer_usercmd_observer.Configure(targets[3]).Accepted()) { reason="usercmd observer rejected"; return false; }

    precache_index=KHook::GetVtableIndex(&ProbePrecache::Run);
    void** vtable=*reinterpret_cast<void***>(&precache_object);
    if (precache_index<0 || !vtable) { reason="precache vtable unavailable"; return false; }
    peer_virtual_before.Configure(precache_index);
    if (!peer_virtual_before.AddGlobal(&precache_object).Accepted()) { reason="before virtual peer rejected"; return false; }
    peer_virtual_before_filter=true;
    void* original=KHook::FindOriginalVirtual(vtable,precache_index);
    if (!original || !image->executable(reinterpret_cast<uintptr_t>(original))) {
        reason="precache original outside verified image"; return false;
    }
    s2resolve::VirtualSlotResolution resolved;
    resolved.target={reinterpret_cast<uintptr_t>(original),image,"controlled structural virtual slot","verified ELF/provider original"};
    resolved.vtable=vtable; resolved.vtable_index=precache_index;
    if (!S2NamedConfigurePrecache(resolved).Accepted()) { reason="precache setup rejected"; return false; }
    peer_virtual_after.Configure(precache_index);
    if (!peer_virtual_after.AddGlobal(&precache_object).Accepted()) { reason="after virtual peer rejected"; return false; }
    peer_virtual_after_filter=true;
    installed=true;
    reason="five production named bindings and before/after peers accepted; observation pending";
    return true;
}

void S2ProbeNamedReset(std::uint64_t map_generation) {
    observation={};
    observation.installed=installed;
    observation.chat_current_return={s2khook::FacetApplicability::Inapplicable,-1};
    observation.output_current_return={s2khook::FacetApplicability::Inapplicable,-1};
    observation.precache_current_return={s2khook::FacetApplicability::Inapplicable,-1};
    observation.bypass={s2khook::FacetApplicability::Inapplicable,-1};
    observation.removal.applicability=s2khook::FacetApplicability::Applicable;
    invocation_map_generation=map_generation;
    observation.precache_generation_source=s2khook::MapGenerationSource::LevelLifetime;
    observation.precache_map_generation=map_generation;
    chat_action_index=chat_post_index=output_action_index=output_post_index=0;
    removal_refusal_checked=false;
    mode=Mode::Idle;
}
void S2ProbeNamedInvoke() {
    if (!installed) return;
    mode=Mode::Damage; call_damage(outer_victim,outer_info,nullptr,nullptr);
    observation.damage_expired=!S2NamedDamageInfo() && !S2NamedDamageVictim();
    mode=Mode::Chat; chat_verdict=0; call_chat(outer_victim,outer_info,true,19,"opaque");
    chat_verdict=1; call_chat(outer_victim,outer_info,true,19,"opaque");
    mode=Mode::Output; output_verdict=1;
    call_output(reinterpret_cast<CEntityIOOutput*>(1),reinterpret_cast<CEntityInstance*>(2),
        reinterpret_cast<CEntityInstance*>(3),reinterpret_cast<const CVariant*>(4),1.25f,outer_info,nullptr);
    output_verdict=2;
    call_output(reinterpret_cast<CEntityIOOutput*>(1),reinterpret_cast<CEntityInstance*>(2),
        reinterpret_cast<CEntityInstance*>(3),reinterpret_cast<const CVariant*>(4),1.25f,outer_info,nullptr);
    mode=Mode::Usercmd;
    std::array<unsigned char,0x130> commands{}; commands[0x10]=1; commands[0xa0]=2;
    call_usercmd(outer_victim,commands.data(),0,true,2.5f);
    observation.usercmd_return=call_usercmd(outer_victim,commands.data(),2,true,2.5f);
    observation.usercmd_expired=S2NamedCurrentUsercmd()==nullptr;
    mode=Mode::Precache;
    S2ProbeNamedInvokeVirtual(&precache_object,precache_index,outer_manifest);
    S2ProbeNamedInvokeVirtual(&other_object,KHook::GetVtableIndex(&OtherPrecache::Run),outer_manifest);
    observation.precache_expired=S2NamedCurrentPrecacheManifest()==nullptr;
    mode=Mode::Idle;
}
s2khook::NamedSnapshot S2ProbeNamedCollect() { return observation; }

bool S2ProbeNamedCanUnloadSync(const S2HookTerminalPermit& permit) {
    const bool result=permit.IsValid() && S2HookInventoryCanRemoveSync(PeerInventory(),permit) &&
        S2NamedHooksCanUnloadSync(permit);
    observation.removal.terminal_preflight=result ? 1 : 0;
    return result;
}
bool S2ProbeNamedUnloadSync(const S2HookTerminalPermit& permit) {
    if (!S2ProbeNamedCanUnloadSync(permit)) { observation.removal.terminal_remove=0; return false; }
    if (peer_virtual_before_filter) { peer_virtual_before.RemoveGlobal(&precache_object); peer_virtual_before_filter=false; }
    if (peer_virtual_after_filter) { peer_virtual_after.RemoveGlobal(&precache_object); peer_virtual_after_filter=false; }
    const bool result=S2HookInventoryBeginRemoveSync(PeerInventory(),permit) && S2NamedHooksUnloadSync(permit);
    observation.removal.terminal_remove=result ? 1 : 0;
    return result;
}
bool S2ProbeNamedRemovalComplete() {
    const bool result=S2HookInventoryRemovalComplete(PeerInventory()) && S2NamedHooksRemovalComplete();
    observation.removal.terminal_complete=result ? 1 : 0;
    return result;
}
