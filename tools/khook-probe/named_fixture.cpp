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
enum class Mode { Idle,Damage,Chat,Output,OutputVector,Usercmd,Precache };
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

KHook::Return<void> EarlyChatPost(void*,void*,bool,int,const char*);
KHook::Return<void> EarlyVirtualPost(ProbePrecache*,void*);
KHook::Return<void> ChatPeerBefore(void*,void*,bool,int,const char*);
KHook::Return<void> ChatPeerAfter(void*,void*,bool,int,const char*);
KHook::Return<void> ChatPeerPost(void*,void*,bool,int,const char*);
KHook::Return<int64_t> DamageObservePre(void*,void*,void*,void*);
KHook::Return<int64_t> DamageObservePost(void*,void*,void*,void*);
KHook::Return<void> OutputObservePre(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*);
KHook::Return<void> OutputObservePost(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                                      const CVariant*,float,void*,char*);
KHook::Return<int> UsercmdObservePre(void*,void*,int,bool,float);
KHook::Return<int> UsercmdObservePost(void*,void*,int,bool,float);
KHook::Return<void> VirtualPeerBefore(ProbePrecache*,void*);
KHook::Return<void> VirtualPeerAfter(ProbePrecache*,void*);
KHook::Return<void> VirtualPeerPost(ProbePrecache*,void*);
S2CheckedFunction<int64_t,void*,void*,void*,void*> peer_damage_observer(&DamageObservePre,&DamageObservePost);
S2CheckedFunction<void,void*,void*,bool,int,const char*> peer_chat_before(&ChatPeerBefore,&EarlyChatPost);
S2CheckedFunction<void,void*,void*,bool,int,const char*> peer_chat_after(&ChatPeerAfter,&ChatPeerPost);
S2CheckedFunction<void,CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*>
    peer_output_observer(&OutputObservePre,&OutputObservePost);
S2CheckedFunction<int,void*,void*,int,bool,float> peer_usercmd_observer(&UsercmdObservePre,&UsercmdObservePost);
S2CheckedVirtual<ProbePrecache,void,void*> peer_virtual_before(&VirtualPeerBefore,&EarlyVirtualPost);
S2CheckedVirtual<ProbePrecache,void,void*> peer_virtual_after(&VirtualPeerAfter,&VirtualPeerPost);
bool peer_virtual_before_filter=false,peer_virtual_after_filter=false;
s2khook::NamedOrderSnapshot order_snapshot;
s2khook::NamedOrderObservation* order_active=nullptr;
bool order_late=false;
int order_phase=0;
std::array<const void*,4> order_targets{};
bool OrderMain() {
    if (!order_active) return false;
    if (order_active->site=="precache" && !order_late) order_snapshot.active_refused=!peer_virtual_before.CanBeginRemove(false);
    ++order_active->callbacks; order_active->trace+='M'; return true;
}
void OrderPeer(bool post,bool late) {
    if (!order_active) return;
    if (late!=order_late) ++order_snapshot.retired_callbacks;
    if (post) { ++order_active->peer_post; order_active->trace+='Q'; }
    else { ++order_active->peer_pre; order_active->trace+='P'; }
}
void OrderOriginal() { if (order_active) { ++order_active->original; order_active->trace+='O'; } }


KHook::Return<void> ChatPeerBefore(void*,void*,bool,int,const char*) {
    auto observed=peer_chat_before.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    OrderPeer(false,false);
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
    OrderPeer(true,false);
    if (order_active) { order_active->effective=KHook::GetCurrentReturn<int64_t>(); order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; }
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
    OrderPeer(true,false);
    if (mode==Mode::OutputVector) observation.output_vector_skipped[output_verdict]=KHook::WasOriginalFunctionSkipped() ? 1 : 0;
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
    OrderPeer(true,false);
    if (order_active) { order_active->effective=KHook::GetCurrentReturn<int>(); order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; }
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
    OrderPeer(false,false);
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

KHook::Return<void> EarlyChatPost(void*,void*,bool,int,const char*) {
    auto observed=peer_chat_before.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,false); if (order_active) order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; }
    return S2_Ignore();
}
KHook::Return<void> EarlyVirtualPost(ProbePrecache* receiver,void*) {
    auto observed=peer_virtual_before.Observe(receiver);
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,false); if (order_active) order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; }
    return S2_Ignore();
}
KHook::Return<int64_t> DamageObservePre(void*,void*,void*,void*) {
    auto observed=peer_damage_observer.Observe();
    if (S2Hook_EnterDispatch(observed)) OrderPeer(false,false);
    return S2_Ignore(int64_t{0});
}
KHook::Return<void> OutputObservePre(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    auto observed=peer_output_observer.Observe();
    if (S2Hook_EnterDispatch(observed)) OrderPeer(false,false);
    return S2_Ignore();
}
KHook::Return<int> UsercmdObservePre(void*,void*,int,bool,float) {
    auto observed=peer_usercmd_observer.Observe();
    if (S2Hook_EnterDispatch(observed)) OrderPeer(false,false);
    return S2_Ignore(0);
}
KHook::Return<int64_t> LateDamagePre(void*,void*,void*,void*);
KHook::Return<int64_t> LateDamagePost(void*,void*,void*,void*);
S2CheckedFunction<int64_t,void*,void*,void*,void*> late_damage(&LateDamagePre,&LateDamagePost);
KHook::Return<int64_t> LateDamagePre(void* a,void* b,void* c,void* d) {
    auto observed=late_damage.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(false,true); }
    return S2_Ignore(int64_t{0});
}
KHook::Return<int64_t> LateDamagePost(void* a,void* b,void* c,void* d) {
    auto observed=late_damage.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,true); if (order_active) { order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; order_active->effective=KHook::GetCurrentReturn<int64_t>(); } }
    return S2_Ignore(int64_t{0});
}
KHook::Return<void> LateChatPre(void*,void*,bool,int,const char*);
KHook::Return<void> LateChatPost(void*,void*,bool,int,const char*);
S2CheckedFunction<void,void*,void*,bool,int,const char*> late_chat(&LateChatPre,&LateChatPost);
KHook::Return<void> LateChatPre(void* a,void* b,bool c,int d,const char* e) {
    auto observed=late_chat.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(false,true); }
    return S2_Ignore();
}
KHook::Return<void> LateChatPost(void* a,void* b,bool c,int d,const char* e) {
    auto observed=late_chat.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,true); if (order_active) { order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; } }
    return S2_Ignore();
}
KHook::Return<void> LateOutputPre(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*);
KHook::Return<void> LateOutputPost(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*);
S2CheckedFunction<void,CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*> late_output(&LateOutputPre,&LateOutputPost);
KHook::Return<void> LateOutputPre(CEntityIOOutput* a,CEntityInstance* b,CEntityInstance* c,const CVariant* d,float e,void* f,char* g) {
    auto observed=late_output.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(false,true); }
    return S2_Ignore();
}
KHook::Return<void> LateOutputPost(CEntityIOOutput* a,CEntityInstance* b,CEntityInstance* c,const CVariant* d,float e,void* f,char* g) {
    auto observed=late_output.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,true); if (order_active) { order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; } }
    return S2_Ignore();
}
KHook::Return<int> LateUsercmdPre(void*,void*,int,bool,float);
KHook::Return<int> LateUsercmdPost(void*,void*,int,bool,float);
S2CheckedFunction<int,void*,void*,int,bool,float> late_usercmd(&LateUsercmdPre,&LateUsercmdPost);
KHook::Return<int> LateUsercmdPre(void* a,void* b,int c,bool d,float e) {
    auto observed=late_usercmd.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(false,true); }
    return S2_Ignore(0);
}
KHook::Return<int> LateUsercmdPost(void* a,void* b,int c,bool d,float e) {
    auto observed=late_usercmd.Observe();
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,true); if (order_active) { order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; order_active->effective=KHook::GetCurrentReturn<int>(); } }
    return S2_Ignore(0);
}
KHook::Return<void> LatePrecachePre(ProbePrecache*,void*);
KHook::Return<void> LatePrecachePost(ProbePrecache*,void*);
S2CheckedVirtual<ProbePrecache,void,void*> late_precache(&LatePrecachePre,&LatePrecachePost);
KHook::Return<void> LatePrecachePre(ProbePrecache* a,void* b) {
    auto observed=late_precache.Observe(a);
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(false,true); }
    return S2_Ignore();
}
KHook::Return<void> LatePrecachePost(ProbePrecache* a,void* b) {
    auto observed=late_precache.Observe(a);
    if (S2Hook_EnterDispatch(observed)) { OrderPeer(true,true); if (order_active) { order_active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0; } }
    return S2_Ignore();
}
std::array<S2CheckedBindingOps*,5> LateInventory() { return {{&late_damage,&late_chat,&late_output,&late_usercmd,&late_precache}}; }
std::array<S2CheckedBindingOps*,7> PeerInventory() {
    return {{&peer_damage_observer,&peer_chat_before,&peer_chat_after,&peer_output_observer,
             &peer_usercmd_observer,&peer_virtual_before,&peer_virtual_after}};
}

void DamagePreOp() {
    if (OrderMain()) return;
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
    if (order_active) return;
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
    if (OrderMain()) return 0;
    if (mode==Mode::Chat) {
        observation.chat_peer_order=observation.chat_peer_order*10+2;
        ++observation.chat_dispatch;
    }
    return chat_verdict;
}
int OutputOp(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    if (OrderMain()) return 0;
    if (mode==Mode::OutputVector) ++observation.output_vector_dispatch[output_verdict];
    if (mode==Mode::Output) ++observation.output_dispatch;
    return output_verdict;
}
int UsercmdSlot(void*) { return 7; }
std::array<unsigned char,0xa0> nested_command{};
int UsercmdDispatch(int) {
    if (OrderMain()) return 0;
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
    if (OrderMain()) return;
    ++observation.precache_dispatch;
    S2NamedPrecacheFrameV1 frame{};
    if (S2NamedReadPrecacheFrameV1(&frame,sizeof frame)) {
        if (frame.receiver==reinterpret_cast<uintptr_t>(&precache_object)) ++observation.precache_frame_receiver_matches;
        if (frame.vtable==reinterpret_cast<uintptr_t>(*reinterpret_cast<void***>(&precache_object))) ++observation.precache_frame_vtable_matches;
    }
    observation.precache_manifest_trace.push_back(S2NamedCurrentPrecacheManifest()==outer_manifest ? "outer" :
        S2NamedCurrentPrecacheManifest()==inner_manifest ? "inner" : "unexpected");
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
        observation.precache_manifest_trace.push_back(S2NamedCurrentPrecacheManifest()==outer_manifest ? "outer" : "unexpected");
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
    OrderOriginal();
    if (mode==Mode::Damage) ++observation.damage_original;
    return INT64_C(0x1122334455667788);
}
extern "C" __attribute__((noinline)) void S2ProbeNamedChatBody(void*,void*,bool,int,const char*) {
    OrderOriginal();
    if (mode==Mode::Chat) { ++observation.chat_original; ++observation.chat_original_each[chat_verdict ? 1 : 0]; }
}
extern "C" __attribute__((noinline)) void S2ProbeNamedOutputBody(
    CEntityIOOutput*,CEntityInstance*,CEntityInstance*,const CVariant*,float,void*,char*) {
    OrderOriginal();
    if (mode==Mode::OutputVector) ++observation.output_vector_original[output_verdict];
    if (mode==Mode::Output) ++observation.output_original;
}
extern "C" __attribute__((noinline)) int S2ProbeNamedUsercmdBody(void* self,void* commands,int count,bool paused,float margin) {
    OrderOriginal();
    if (mode==Mode::Usercmd) {
        ++observation.usercmd_original;
        if (count>=0 && count<=2) ++observation.usercmd_original_by_batch[count];
        if (self==outer_victim && commands && count>=0 && count<=2 && paused && margin==2.5f) ++observation.usercmd_argument_matches;
        if (count==2) {
            observation.usercmd_batch_first=static_cast<unsigned char*>(commands)[0x10];
            observation.usercmd_batch_second=static_cast<unsigned char*>(commands)[0xa0];
        }
    }
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
    OrderOriginal();
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

    if (!peer_damage_observer.Configure(targets[0]).Accepted()) { reason="damage observer rejected"; return false; }
    if (!S2NamedConfigureDamage(targets[0]).Accepted()) { reason="damage setup rejected"; return false; }
    if (!peer_chat_before.Configure(targets[1]).Accepted()) { reason="before chat peer rejected"; return false; }
    if (!S2NamedConfigureChat(targets[1]).Accepted()) { reason="chat setup rejected"; return false; }
    if (!peer_chat_after.Configure(targets[1]).Accepted()) { reason="after chat peer rejected"; return false; }
    if (!peer_output_observer.Configure(targets[2]).Accepted()) { reason="output observer rejected"; return false; }
    if (!S2NamedConfigureOutput(targets[2]).Accepted()) { reason="output setup rejected"; return false; }
    S2NamedSetUsercmdTarget(targets[3]);
    if (!peer_usercmd_observer.Configure(targets[3]).Accepted()) { reason="usercmd observer rejected"; return false; }
    if (!S2NamedInstallUsercmd().Accepted()) { reason="usercmd setup rejected"; return false; }

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
    order_targets=targets;
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
    const int before_filtered=observation.precache_dispatch;
    S2ProbeNamedInvokeVirtual(&other_object,KHook::GetVtableIndex(&OtherPrecache::Run),outer_manifest);
    observation.precache_filtered_dispatch=observation.precache_dispatch-before_filtered;
    observation.precache_expired=S2NamedCurrentPrecacheManifest()==nullptr;
    mode=Mode::OutputVector;
    for (output_verdict=0;output_verdict<4;++output_verdict)
        call_output(reinterpret_cast<CEntityIOOutput*>(1),reinterpret_cast<CEntityInstance*>(2),reinterpret_cast<CEntityInstance*>(3),
            reinterpret_cast<const CVariant*>(4),1.25f,outer_info,nullptr);
    mode=Mode::Idle;
}
s2khook::NamedSnapshot S2ProbeNamedCollect() { return observation; }

bool S2ProbeNamedCanUnloadSync(const S2HookTerminalPermit& permit) {
    const bool result=permit.IsValid() && S2HookInventoryCanRemoveSync(PeerInventory(),permit) && S2HookInventoryCanRemoveSync(LateInventory(),permit) &&
        S2NamedHooksCanUnloadSync(permit);
    observation.removal.terminal_preflight=result ? 1 : 0;
    return result;
}
bool S2ProbeNamedUnloadSync(const S2HookTerminalPermit& permit) {
    if (!S2ProbeNamedCanUnloadSync(permit)) { observation.removal.terminal_remove=0; return false; }
    if (peer_virtual_before_filter) { peer_virtual_before.RemoveGlobal(&precache_object); peer_virtual_before_filter=false; }
    if (peer_virtual_after_filter) { peer_virtual_after.RemoveGlobal(&precache_object); peer_virtual_after_filter=false; }
    const bool result=S2HookInventoryBeginRemoveSync(PeerInventory(),permit) && S2HookInventoryBeginRemoveSync(LateInventory(),permit) && S2NamedHooksUnloadSync(permit);
    observation.removal.terminal_remove=result ? 1 : 0;
    return result;
}
bool S2ProbeNamedRemovalComplete() {
    const bool result=S2HookInventoryRemovalComplete(PeerInventory()) && S2HookInventoryRemovalComplete(LateInventory()) && S2NamedHooksRemovalComplete();
    observation.removal.terminal_complete=result ? 1 : 0;
    return result;
}

namespace {
void InvokeOrderPhase(bool late) {
    order_late=late;
    for (const char* site:{"damage","chat","output","usercmd","precache"}) {
        s2khook::NamedOrderObservation row; row.site=site; row.order=late ? "s2script-first" : "peer-first";
        order_active=&row;
        if (row.site=="damage") call_damage(outer_victim,outer_info,nullptr,nullptr);
        else if (row.site=="chat") call_chat(outer_victim,outer_info,true,19,"order");
        else if (row.site=="output") call_output(nullptr,nullptr,nullptr,nullptr,0,nullptr,nullptr);
        else if (row.site=="usercmd") { std::array<unsigned char,0xa0> commands{}; call_usercmd(outer_victim,commands.data(),1,true,2.5f); }
        else S2ProbeNamedInvokeVirtual(&precache_object,precache_index,outer_manifest);
        order_active=nullptr; order_snapshot.rows.push_back(row);
    }
}
}
void S2ProbeNamedStartOrders(const std::string& run) {
    if (!installed || order_snapshot.requested) return;
    order_snapshot.run=run; order_snapshot.requested=true; order_phase=1;
    // Drop legacy after-peers first. Only one independent peer remains for the
    // measured peer-first phase, never the old simultaneous before+after trace.
    peer_chat_after.BeginRemove(true); peer_virtual_after.BeginRemove(true);
}
void S2ProbeNamedAdvanceOrders() {
    if (!order_snapshot.requested || order_snapshot.completion) return;
    if (order_phase==1) {
        if (!peer_chat_after.RemovalComplete() || !peer_virtual_after.RemovalComplete()) return;
        InvokeOrderPhase(false); order_phase=2;
        for (auto* peer:PeerInventory()) peer->BeginRemove(true);
        return; // completion is observed on a later frame, never spun here.
    }
    if (order_phase!=2 || !S2HookInventoryRemovalComplete(PeerInventory())) return;
    order_snapshot.completion=true;
    late_precache.Configure(precache_index);
    order_snapshot.late_installed=late_damage.Configure(order_targets[0]).Accepted() &&
        late_chat.Configure(order_targets[1]).Accepted() && late_output.Configure(order_targets[2]).Accepted() &&
        late_usercmd.Configure(order_targets[3]).Accepted() && late_precache.AddGlobal(&precache_object).Accepted();
    if (order_snapshot.late_installed) InvokeOrderPhase(true);
}
s2khook::NamedOrderSnapshot S2ProbeNamedCollectOrders() { return order_snapshot; }
