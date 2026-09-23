#include "named_hooks.h"

#include "khook_map.h"

#include <array>
#include <cstdint>
#include <atomic>

namespace {

struct PrecacheReceiver {};

KHook::Return<void> DamagePre(void*,void*,void*);
KHook::Return<void> DamagePost(void*,void*,void*);
KHook::Return<void> ChatPre(void*,void*,bool,int,const char*);
KHook::Return<void> OutputPre(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                              const CVariant*,float,void*,char*);
KHook::Return<int> UsercmdPre(void*,void*,int,bool,float);
KHook::Return<void> PrecachePre(PrecacheReceiver*,void*);

S2CheckedFunction<void,void*,void*,void*> g_damage(&DamagePre,&DamagePost);
S2CheckedFunction<void,void*,void*,bool,int,const char*> g_chat(&ChatPre,nullptr);
S2CheckedFunction<void,CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                  const CVariant*,float,void*,char*> g_output(&OutputPre,nullptr);
S2CheckedFunction<int,void*,void*,int,bool,float> g_usercmd(&UsercmdPre,nullptr);
S2CheckedVirtual<PrecacheReceiver,void,void*> g_precache(&PrecachePre,nullptr);

S2NamedHookOps g_ops;
const void* g_usercmd_target=nullptr;
s2resolve::VirtualSlotResolution g_precache_resolution;
bool g_precache_filter_added=false;

struct DamageFrame {
    void* info;
    void* victim;
    DamageFrame* previous;
};
thread_local DamageFrame* g_damage_frame=nullptr;
class DamageScope {
public:
    DamageScope(void* info,void* victim) : frame_{info,victim,g_damage_frame} { g_damage_frame=&frame_; }
    ~DamageScope() { g_damage_frame=frame_.previous; }
    DamageScope(const DamageScope&)=delete;
    DamageScope& operator=(const DamageScope&)=delete;
private:
    DamageFrame frame_;
};

struct PointerFrame { void* value; PointerFrame* previous; };
thread_local PointerFrame* g_usercmd_frame=nullptr;
thread_local PointerFrame* g_manifest_frame=nullptr;
struct PrecacheFrame {
    S2NamedPrecacheFrameV1 value;
    PrecacheFrame* previous;
};
thread_local PrecacheFrame* g_precache_frame=nullptr;
std::atomic<uint64_t> g_precache_serial{0};
class PrecacheScope {
public:
    PrecacheScope(void* receiver,void* manifest) : frame_{{1,sizeof(S2NamedPrecacheFrameV1),
        ++g_precache_serial,reinterpret_cast<uintptr_t>(receiver),
        reinterpret_cast<uintptr_t>(g_precache_resolution.vtable),reinterpret_cast<uintptr_t>(manifest)},g_precache_frame} {
        g_precache_frame=&frame_;
    }
    ~PrecacheScope() { g_precache_frame=frame_.previous; }
    PrecacheScope(const PrecacheScope&)=delete;
    PrecacheScope& operator=(const PrecacheScope&)=delete;
private:
    PrecacheFrame frame_;
};
class PointerScope {
public:
    PointerScope(PointerFrame*& top,void* value) : top_(top),frame_{value,top} { top_=&frame_; }
    ~PointerScope() { top_=frame_.previous; }
    PointerScope(const PointerScope&)=delete;
    PointerScope& operator=(const PointerScope&)=delete;
private:
    PointerFrame*& top_;
    PointerFrame frame_;
};

std::array<S2CheckedBindingOps*,5> Inventory() {
    return {{&g_damage,&g_chat,&g_output,&g_usercmd,&g_precache}};
}

KHook::Return<void> DamagePre(void* victim,void* info,void* /* optional result */) {
    auto observed=g_damage.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    DamageScope scope(info,victim);
    if (g_ops.damage_pre) g_ops.damage_pre();
    const auto action=S2_Ignore();
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Damage,false,action.action);
    return action;
}

KHook::Return<void> DamagePost(void* victim,void* info,void* /* optional result */) {
    auto observed=g_damage.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    DamageScope scope(info,victim);
    if (g_ops.damage_post) g_ops.damage_post();
    const auto action=S2_Ignore();
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Damage,true,action.action);
    return action;
}

KHook::Return<void> ChatPre(void* controller,void* command,bool team,int number,const char* text) {
    auto observed=g_chat.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    const auto action=g_ops.chat && g_ops.chat(controller,command,team,number,text) ?
        S2_Supersede() : S2_Ignore();
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Chat,false,action.action);
    return action;
}

KHook::Return<void> OutputPre(CEntityIOOutput* output,CEntityInstance* activator,
                              CEntityInstance* caller,const CVariant* value,float delay,
                              void* opaque,char* tail) {
    auto observed=g_output.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    const int result=g_ops.output ?
        g_ops.output(output,activator,caller,value,delay,opaque,tail) : 0;
    const auto action=result>=2 ? S2_Supersede() : S2_Ignore();
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Output,false,action.action);
    return action;
}

KHook::Return<int> UsercmdPre(void* receiver,void* commands,int count,bool,float) {
    auto observed=g_usercmd.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore(0);
    const int slot=g_ops.usercmd_slot ? g_ops.usercmd_slot(receiver) : -1;
    if (commands && count>0) {
        constexpr size_t kStride=0x90;
        constexpr size_t kMessageOffset=0x10;
        for (int i=0;i<count;++i) {
            auto* command=static_cast<unsigned char*>(commands)+size_t(i)*kStride+kMessageOffset;
            PointerScope scope(g_usercmd_frame,command);
            const int result=g_ops.usercmd_dispatch ? g_ops.usercmd_dispatch(slot) : 0;
            if (result>=2 && g_ops.usercmd_neutralize) g_ops.usercmd_neutralize();
        }
    }
    const auto action=S2_Ignore(0);
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Usercmd,false,action.action);
    return action;
}

KHook::Return<void> PrecachePre(PrecacheReceiver* receiver,void* manifest) {
    auto observed=g_precache.Observe(receiver);
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    PointerScope scope(g_manifest_frame,manifest);
    PrecacheScope native_frame(receiver,manifest);
    if (g_ops.precache) g_ops.precache();
    const auto action=S2_Ignore();
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Precache,false,action.action);
    return action;
}

} // namespace

void S2NamedHooksSetOps(const S2NamedHookOps& ops) { g_ops=ops; }
S2HookReceipt S2NamedConfigureDamage(const void* target) { return g_damage.Configure(target); }
S2HookReceipt S2NamedConfigureChat(const void* target) { return g_chat.Configure(target); }
S2HookReceipt S2NamedConfigureOutput(const void* target) { return g_output.Configure(target); }
void S2NamedSetUsercmdTarget(const void* target) { g_usercmd_target=target; }

S2HookReceipt S2NamedInstallUsercmd() {
    if (!g_usercmd_target)
        return {KHook::INVALID_HOOK,S2HookState::Failed,"usercmd target unavailable"};
    const auto current=g_usercmd.Snapshot();
    if (current.Accepted()) return current;
    return g_usercmd.Configure(g_usercmd_target);
}

S2HookReceipt S2NamedConfigurePrecache(const s2resolve::VirtualSlotResolution& resolved) {
    if (!resolved.vtable || resolved.vtable_index<0 || resolved.vtable_index>=512 ||
        !resolved.target.address)
        return {KHook::INVALID_HOOK,S2HookState::Failed,"invalid structural virtual-slot resolution"};
    g_precache_resolution=resolved;
    g_precache.Configure(resolved.vtable_index);
    struct Holder { void** vptr; } holder{resolved.vtable};
    const auto receipt=g_precache.AddGlobal(reinterpret_cast<PrecacheReceiver*>(&holder));
    g_precache_filter_added=receipt.Accepted();
    return receipt;
}

S2HookReceipt S2NamedHookSnapshot(S2NamedHookSite site) {
    switch (site) {
        case S2NamedHookSite::Damage: return g_damage.Snapshot();
        case S2NamedHookSite::Chat: return g_chat.Snapshot();
        case S2NamedHookSite::Output: return g_output.Snapshot();
        case S2NamedHookSite::Usercmd: return g_usercmd.Snapshot();
        case S2NamedHookSite::Precache: return g_precache.Snapshot();
    }
    return {KHook::INVALID_HOOK,S2HookState::Failed,"unknown named hook site"};
}

void* S2NamedDamageInfo() {
    return S2Hook_MayDispatch() && g_damage_frame ? g_damage_frame->info : nullptr;
}
void* S2NamedDamageVictim() {
    return S2Hook_MayDispatch() && g_damage_frame ? g_damage_frame->victim : nullptr;
}
void* S2NamedCurrentUsercmd() {
    return S2Hook_MayDispatch() && g_usercmd_frame ? g_usercmd_frame->value : nullptr;
}
void* S2NamedCurrentPrecacheManifest() {
    return S2Hook_MayDispatch() && g_manifest_frame ? g_manifest_frame->value : nullptr;
}

bool S2NamedDispatchSyntheticDamage(void* victim,void* info) {
    if (!g_damage.Snapshot().Accepted()) return false;
    S2HookDispatchGuard guard;
    if (!guard) return false;
    {
        DamageScope scope(info,victim);
        if (g_ops.damage_pre) g_ops.damage_pre();
    }
    {
        DamageScope scope(info,victim);
        if (g_ops.damage_post) g_ops.damage_post();
    }
    return true;
}

bool S2NamedHooksCanUnloadSync(const S2HookTerminalPermit& permit) {
    return S2HookInventoryCanRemoveSync(Inventory(),permit);
}

bool S2NamedHooksUnloadSync(const S2HookTerminalPermit& permit) {
    if (!S2NamedHooksCanUnloadSync(permit)) return false;
    if (g_precache_filter_added && g_precache_resolution.vtable) {
        struct Holder { void** vptr; } holder{g_precache_resolution.vtable};
        g_precache.RemoveGlobal(reinterpret_cast<PrecacheReceiver*>(&holder));
        g_precache_filter_added=false;
    }
    return S2HookInventoryBeginRemoveSync(Inventory(),permit);
}

bool S2NamedHooksRemovalComplete() {
    return S2HookInventoryRemovalComplete(Inventory());
}

extern "C" bool S2NamedReadPrecacheFrameV1(S2NamedPrecacheFrameV1* out,size_t size) {
    if (!out || size!=sizeof(*out)) return false;
    *out={};
    if (!g_precache_frame) return false;
    *out=g_precache_frame->value;
    return true;
}
