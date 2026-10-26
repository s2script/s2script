#include "named_hooks.h"

#include "khook_map.h"

#include <array>
#include <cstdint>
#include <atomic>

namespace {

struct PrecacheReceiver {};
struct CvarResponseReceiver {};

KHook::Return<void> ChatPre(void*,void*,bool,int,const char*);
KHook::Return<void> OutputPre(CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                              CPulseArgumentPack*,float,CPulseInputParamMap*,const CVariant*);
KHook::Return<int> UsercmdPre(void*,void*,int,bool,float);
KHook::Return<void> PrecachePre(PrecacheReceiver*,void*);
KHook::Return<bool> InputPre(CEntityIdentity*,void*,CEntityInstance*,CEntityInstance*,
                             const CVariant*,void*,void*);
KHook::Return<bool> CvarResponsePre(CvarResponseReceiver*,const void*);

S2CheckedFunction<void,void*,void*,bool,int,const char*> g_chat(&ChatPre,nullptr);
S2CheckedFunction<void,CEntityIOOutput*,CEntityInstance*,CEntityInstance*,
                  CPulseArgumentPack*,float,CPulseInputParamMap*,const CVariant*> g_output(&OutputPre,nullptr);
S2CheckedFunction<int,void*,void*,int,bool,float> g_usercmd(&UsercmdPre,nullptr);
S2CheckedVirtual<PrecacheReceiver,void,void*> g_precache(&PrecachePre,nullptr);
S2CheckedFunction<bool,CEntityIdentity*,void*,CEntityInstance*,CEntityInstance*,
                  const CVariant*,void*,void*> g_input(&InputPre,nullptr);
S2CheckedVirtual<CvarResponseReceiver,bool,const void*> g_cvar_response(&CvarResponsePre,nullptr);

S2NamedHookOps g_ops;
const void* g_usercmd_target=nullptr;
s2resolve::VirtualSlotResolution g_precache_resolution;
bool g_precache_filter_added=false;
s2resolve::VirtualSlotResolution g_cvar_response_resolution;
bool g_cvar_response_filter_added=false;

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

std::array<S2CheckedBindingOps*,6> Inventory() {
    return {{&g_chat,&g_output,&g_usercmd,&g_precache,&g_input,&g_cvar_response}};
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
                              CEntityInstance* caller,CPulseArgumentPack* arguments,float delay,
                              CPulseInputParamMap* parameters,const CVariant* value) {
    auto observed=g_output.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    const int result=g_ops.output ?
        g_ops.output(output,activator,caller,arguments,delay,parameters,value) : 0;
    const auto action=result>=2 ? S2_Supersede() : S2_Ignore();
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Output,false,action.action);
    return action;
}

// A superseded input reports "not accepted" (false) to the engine, as an entity without a
// handler for that input would.
KHook::Return<bool> InputPre(CEntityIdentity* identity,void* input,CEntityInstance* activator,
                             CEntityInstance* caller,const CVariant* value,void*,void*) {
    auto observed=g_input.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore(false);
    const int result=g_ops.input ? g_ops.input(identity,input,activator,caller,value) : 0;
    const auto action=result>=2 ? S2_Supersede(false) : S2_Ignore(false);
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::Input,false,action.action);
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

// The client's answer to a GetCvarValue query. The receiver is the client object and the
// argument is the engine's CNetMessage wrapper. The op copies what it needs before returning;
// the original (a plain `return true`) always runs.
KHook::Return<bool> CvarResponsePre(CvarResponseReceiver* receiver,const void* message) {
    auto observed=g_cvar_response.Observe(receiver);
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore(true);
    if (g_ops.cvar_response) g_ops.cvar_response(receiver,message);
    const auto action=S2_Ignore(true);
    if (g_ops.observe_action) g_ops.observe_action(S2NamedHookSite::CvarResponse,false,action.action);
    return action;
}

} // namespace

void S2NamedHooksSetOps(const S2NamedHookOps& ops) { g_ops=ops; }
S2HookReceipt S2NamedConfigureChat(const void* target) { return g_chat.Configure(target); }
S2HookReceipt S2NamedConfigureOutput(const void* target) { return g_output.Configure(target); }
S2HookReceipt S2NamedConfigureInput(const void* target) { return g_input.Configure(target); }
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

S2HookReceipt S2NamedConfigureCvarResponse(const s2resolve::VirtualSlotResolution& resolved) {
    if (!resolved.vtable || resolved.vtable_index<0 || resolved.vtable_index>=512 ||
        !resolved.target.address)
        return {KHook::INVALID_HOOK,S2HookState::Failed,"invalid structural virtual-slot resolution"};
    g_cvar_response_resolution=resolved;
    g_cvar_response.Configure(resolved.vtable_index);
    struct Holder { void** vptr; } holder{resolved.vtable};
    const auto receipt=g_cvar_response.AddGlobal(reinterpret_cast<CvarResponseReceiver*>(&holder));
    g_cvar_response_filter_added=receipt.Accepted();
    return receipt;
}

S2HookReceipt S2NamedHookSnapshot(S2NamedHookSite site) {
    switch (site) {
        case S2NamedHookSite::Chat: return g_chat.Snapshot();
        case S2NamedHookSite::Output: return g_output.Snapshot();
        case S2NamedHookSite::Usercmd: return g_usercmd.Snapshot();
        case S2NamedHookSite::Precache: return g_precache.Snapshot();
        case S2NamedHookSite::Input: return g_input.Snapshot();
        case S2NamedHookSite::CvarResponse: return g_cvar_response.Snapshot();
    }
    return {KHook::INVALID_HOOK,S2HookState::Failed,"unknown named hook site"};
}

void* S2NamedCurrentUsercmd() {
    return S2Hook_MayDispatch() && g_usercmd_frame ? g_usercmd_frame->value : nullptr;
}
void* S2NamedCurrentPrecacheManifest() {
    return S2Hook_MayDispatch() && g_manifest_frame ? g_manifest_frame->value : nullptr;
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
    if (g_cvar_response_filter_added && g_cvar_response_resolution.vtable) {
        struct Holder { void** vptr; } holder{g_cvar_response_resolution.vtable};
        g_cvar_response.RemoveGlobal(reinterpret_cast<CvarResponseReceiver*>(&holder));
        g_cvar_response_filter_added=false;
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
