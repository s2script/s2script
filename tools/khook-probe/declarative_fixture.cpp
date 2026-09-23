#include "declarative_fixture.h"
#include "engine_hooks.h"
#include "engine_calls.h"
#include "hook_dispatch.h"
#include "khook_binding.h"
#include "khook_map.h"
#include <dlfcn.h>
#include <map>
#include <sys/stat.h>

namespace {
std::map<const void*, s2resolve::Resolution> resolutions;
s2khook::DeclarativeSnapshot observation;
enum class Mode { Idle, Void, Mutation, Acquire, Nesting, Bypass, Hud };
Mode mode = Mode::Idle;
int receiver;
void* last_view = nullptr;
bool installed = false;
int hud_case=0;
int acquire_case = 0, engine_result = 0, depth = 0;
void* nested_views[3]{};
constexpr int64_t kHighA = static_cast<int64_t>(UINT64_C(0xf123456789abcdef));
constexpr int64_t kHighB = static_cast<int64_t>(UINT64_C(0x8123456789abcdef));
// Reading these pointers through volatile prevents IPA from assuming the entry is unpatched.
void (*volatile call_void)(void*) = &S2ProbeDeclarativeVoidTarget;
void (*volatile call_wide)(void*,float,int32_t,int64_t,int64_t) = &S2ProbeDeclarativeWideTarget;
int32_t (*volatile call_acquire)(void*,int64_t,int32_t,int64_t) = &S2ProbeDeclarativeAcquireTarget;

int Dispatch(int id, void* view) {
    last_view = view;
    if (id==3 && mode==Mode::Hud) {
        auto& h=observation.hud[hud_case]; ++h.pre; h.trace=h.trace*10+1;
        return hud_case==1 ? 2 : 0;
    }
    if (id == 0) {
        if (mode == Mode::Void) {
            ++observation.simple.pre;
            uint32_t handle = 0;
            if (S2_HookReceiverHandle(view, &handle) == 0 && handle == 42)
                ++observation.simple.receiver_ok;
        } else if (mode == Mode::Nesting) ++observation.nesting.other_pre;
    } else if (id == 1 && mode == Mode::Mutation) {
        ++observation.mutation.pre;
        S2_HookWriteF32(view, 0, 7.25f);
        S2_HookWriteI32(view, 1, -17);
        return 1;
    } else if (id == 2 && mode == Mode::Acquire) {
        ++observation.acquire[acquire_case].pre;
        // Cases: Continue, Changed Allowed, Changed denial, Handled implicit, Handled Allowed.
        if (acquire_case == 1 || acquire_case == 2 || acquire_case == 4) {
            S2_HookWriteI32(view, 1, acquire_case == 2 ? 2 : 0);
            S2_HookWriteI32(view, 2, 1);
        }
        return acquire_case == 0 ? 0 : acquire_case < 3 ? 1 : 2;
    } else if (id == 2 && mode == Mode::Bypass) {
        auto& b = observation.bypass;
        ++b.pre;
        if (!S2EngineHooksCanUnloadSync(S2Hook_CurrentTerminalPermit())) ++b.removal_refused;
        S2_HookResetAll();
        int32_t method = -1;
        if (S2_HookReadI32(view, 0, &method) == 0 && method == 12) ++b.reset_preserved_view;
    } else if (id == 2 && mode == Mode::Nesting) {
        auto& n = observation.nesting;
        ++n.same_pre;
        nested_views[depth] = view;
        int32_t method = -1;
        S2_HookReadI32(view, 0, &method);
        if (depth < 2) {
            ++depth;
            const int32_t inner = call_acquire(&receiver, kHighA, method + 1, kHighB);
            int32_t stale = 0;
            if (S2_HookReadI32(nested_views[depth], 0, &stale) == -1) ++n.stale_rejected;
            --depth;
            int32_t restored = 0;
            if (inner == method + 1 && S2_HookReadI32(view, 0, &restored) == 0 && restored == method)
                ++n.same_restored;
            call_void(&receiver);
            if (S2_HookReadI32(view, 0, &restored) == 0 && restored == method) ++n.other_restored;
        }
    }
    return 0;
}
int Post(int id, void* view, int skipped) {
    if (id==3 && mode==Mode::Hud) {
        auto& h=observation.hud[hud_case]; ++h.completion; h.trace=h.trace*10+2; h.skipped=skipped;
        return 0;
    }
    if (id != 2) return 0;
    if (mode == Mode::Acquire) {
        auto& a = observation.acquire[acquire_case];
        ++a.post;
        S2_HookReadI32(view, 1, &a.post_result);
        a.skipped = skipped;
    } else if (mode == Mode::Bypass) {
        ++observation.bypass.post;
    } else if (mode == Mode::Nesting) {
        auto& n = observation.nesting;
        int32_t method = -1;
        S2_HookReadI32(view, 0, &method);
        if (n.same_post < 3) n.post_methods[n.same_post] = method;
        ++n.same_post;
        if (skipped) ++n.skipped;
    }
    return 0;
}
} // namespace

extern "C" __attribute__((noinline)) void S2ProbeDeclarativeVoidBody(void* self) {
    if (mode == Mode::Void) {
        ++observation.simple.original;
        if (self == &receiver && S2Hook_ActiveCount() > 0) ++observation.simple.original_in_scope;
    } else if (mode == Mode::Nesting) ++observation.nesting.other_original;
}
extern "C" __attribute__((noinline)) void S2ProbeDeclarativeWideBody(
    void*, float value, int32_t integer, int64_t a, int64_t b) {
    if (mode != Mode::Mutation) return;
    auto& m = observation.mutation;
    ++m.original;
    m.value = value; m.integer = integer; m.opaque_a = a; m.opaque_b = b;
    if (S2Hook_ActiveCount() > 0) ++m.original_in_scope;
}
extern "C" __attribute__((noinline)) int32_t S2ProbeDeclarativeAcquireBody(
    void*, int64_t a, int32_t method, int64_t b) {
    if (mode == Mode::Acquire) {
        auto& row = observation.acquire[acquire_case];
        ++row.original;
        if (a == kHighA && b == kHighB && method == 12) ++row.arguments_ok;
        return engine_result;
    }
    if (mode == Mode::Nesting) { ++observation.nesting.same_original; return method; }
    if (mode == Mode::Bypass) { ++observation.bypass.original; return 6; }
    return 0;
}

extern "C" __attribute__((noinline)) void S2ProbeDeclarativeHudBody(void*,int64_t,int64_t,int64_t) {
    if (mode==Mode::Hud) { auto& h=observation.hud[hud_case]; ++h.original; h.trace=h.trace*10+3; }
}
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeDeclarativeHudTarget(void*,int64_t,int64_t,int64_t) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x48,0x34\n\tjmp S2ProbeDeclarativeHudBody");
}
#else
extern "C" void S2ProbeDeclarativeHudTarget(void* s,int64_t a,int64_t b,int64_t c) { S2ProbeDeclarativeHudBody(s,a,b,c); }
#endif

#if defined(__linux__) && defined(__x86_64__)
// Unique safe eight-byte NOPs are the actual entry prologues. A signature scan can identify these
// hidden controlled targets without an exported symbol or a compiler-specific function prefix.
// The tail jump preserves the exact native ABI; the stock provider owns its relocation.
extern "C" __attribute__((naked,noinline)) void S2ProbeDeclarativeVoidTarget(void*) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x56,0x34\n\tjmp S2ProbeDeclarativeVoidBody");
}
extern "C" __attribute__((naked,noinline)) void S2ProbeDeclarativeWideTarget(void*,float,int32_t,int64_t,int64_t) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x57,0x34\n\tjmp S2ProbeDeclarativeWideBody");
}
extern "C" __attribute__((naked,noinline)) int32_t S2ProbeDeclarativeAcquireTarget(void*,int64_t,int32_t,int64_t) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x32,0x41,0x34\n\tjmp S2ProbeDeclarativeAcquireBody");
}
#else
// Host syntax checks can compile the fixture, but only Linux x86_64 is native acceptance.
extern "C" void S2ProbeDeclarativeVoidTarget(void* s) { S2ProbeDeclarativeVoidBody(s); }
extern "C" void S2ProbeDeclarativeWideTarget(void* s,float f,int32_t i,int64_t a,int64_t b) { S2ProbeDeclarativeWideBody(s,f,i,a,b); }
extern "C" int32_t S2ProbeDeclarativeAcquireTarget(void* s,int64_t a,int32_t i,int64_t b) { return S2ProbeDeclarativeAcquireBody(s,a,i,b); }
#endif

bool S2ProbeDeclarativeInstall(std::string& reason) {
    if (installed) return true;
    Dl_info loaded{};
    if (!dladdr(reinterpret_cast<void*>(&S2ProbeDeclarativeVoidTarget), &loaded) || !loaded.dli_fname) {
        reason = "controlled declarative target module not found";
        return false;
    }
    auto image = s2original::OpenLoadedModule(loaded.dli_fname, reason);
    if (!image) return false;
    const void* targets[] = {reinterpret_cast<void*>(&S2ProbeDeclarativeVoidTarget),
        reinterpret_cast<void*>(&S2ProbeDeclarativeWideTarget), reinterpret_cast<void*>(&S2ProbeDeclarativeAcquireTarget), reinterpret_cast<void*>(&S2ProbeDeclarativeHudTarget)};
    const int shapes[] = {S2_HOOK_SHAPE_THIS_VOID, S2_HOOK_SHAPE_THIS_F32_I32_I64_I64, S2_HOOK_SHAPE_THIS_I64_I32_I64,S2_HOOK_SHAPE_THIS_I64_I64_I64};
    S2Hook_SetOps({&Dispatch, &Post});
    for (int id = 0; id < 4; ++id) {
        if (!image->executable(reinterpret_cast<uintptr_t>(targets[id]))) return false;
        resolutions[targets[id]] = {reinterpret_cast<uintptr_t>(targets[id]), image, "controlled native target", "verified ELF"};
        char why[256]{};
        if (S2_HookInstall(id, shapes[id], reinterpret_cast<int64_t>(targets[id]), why, sizeof why) != 0) {
            reason = why;
            return false;
        }
    }
    installed = true;
    reason = "three checked production shapes accepted; observation pending";
    return true;
}

void S2ProbeDeclarativeReset() {
    observation = {};
    observation.simple.installed = installed;
    last_view = nullptr;
    mode = Mode::Idle;
    depth = 0;
    for (auto& view : nested_views) view = nullptr;
}
void S2ProbeDeclarativeInvoke() {
    if (!installed) return;
    mode = Mode::Void;
    call_void(&receiver);
    uint32_t handle = 0;
    observation.simple.expired = last_view && S2_HookReceiverHandle(last_view, &handle) == -1;
    mode = Mode::Mutation;
    call_wide(&receiver, 1.5f, 3, kHighA, kHighB);
    mode = Mode::Acquire;
    for (acquire_case = 0; acquire_case < 5; ++acquire_case) {
        engine_result = acquire_case < 2 ? 6 : 0;
        observation.acquire[acquire_case].effective_return = call_acquire(&receiver, kHighA, 12, kHighB);
    }
    mode = Mode::Nesting;
    observation.nesting.effective_return = call_acquire(&receiver, kHighA, 40, kHighB);
    mode = Mode::Bypass;
    S2_HookArmBypass(2);
    observation.bypass.returns[0] = call_acquire(&receiver, kHighA, 12, kHighB);
    observation.bypass.pre_after_bypass = observation.bypass.pre;
    observation.bypass.post_after_bypass = observation.bypass.post;
    observation.bypass.returns[1] = call_acquire(&receiver, kHighA, 12, kHighB);
    S2_HookArmBypass(2);
    S2_HookDisarmBypass(2);
    observation.bypass.returns[2] = call_acquire(&receiver, kHighA, 12, kHighB);
    mode=Mode::Hud;
    const char* text="s2-khook-hud";
    void (*volatile call_hud)(void*,int64_t,int64_t,int64_t)=&S2ProbeDeclarativeHudTarget;
    for (hud_case=0;hud_case<2;++hud_case) call_hud(&receiver,reinterpret_cast<int64_t>(&receiver),reinterpret_cast<int64_t>(&receiver),reinterpret_cast<int64_t>(&text));
    mode = Mode::Idle;
}
s2khook::DeclarativeSnapshot S2ProbeDeclarativeCollect() { return observation; }

// Test-plugin engine contacts only. The production shim links its real engine_calls.cpp.
// The fixture selects these targets itself; all instruction validation still uses the verified
// backing ELF through the exact copied-resolution handoff used by production.
bool S2_EngineCallResolutionForAddress(const void* address, s2resolve::Resolution& out) {
    const auto it = resolutions.find(address);
    out = it == resolutions.end() ? s2resolve::Resolution{} : it->second;
    return it != resolutions.end();
}
int S2_AddressIsExecutable(const void* address) {
    for (const auto& item : resolutions)
        if (item.second.image->executable(reinterpret_cast<uintptr_t>(address))) return 1;
    return 0;
}
uint32_t S2_EntityHandleFromPtr(void* pointer) { return pointer == &receiver ? 42 : S2_ENTITY_HANDLE_NONE; }
void* S2_ResolveEntity(int, int) { return nullptr; }

// The following targets are deliberately disjoint from the private production-TU
// fixture above. Only the actual main runtime installs its five shape callbacks.
extern "C" void S2ProbeBridgeVoid0(void* self);
extern "C" void S2ProbeBridgeNarrow0(void* self,float value,int32_t a,int32_t b,int32_t c);
extern "C" void S2ProbeBridgeWide0(void* self,float value,int32_t a,int64_t b,int64_t c);
extern "C" int32_t S2ProbeBridgeAcquire0(void* self,int64_t a,int32_t method,int64_t b);
extern "C" void S2ProbeBridgeHud0(void* self,int64_t controller,int64_t layout,int64_t text);
extern "C" void S2ProbeBridgeVoid1(void* self);
extern "C" void S2ProbeBridgeNarrow1(void* self,float value,int32_t a,int32_t b,int32_t c);
extern "C" void S2ProbeBridgeWide1(void* self,float value,int32_t a,int64_t b,int64_t c);
extern "C" int32_t S2ProbeBridgeAcquire1(void* self,int64_t a,int32_t method,int64_t b);
extern "C" void S2ProbeBridgeHud1(void* self,int64_t controller,int64_t layout,int64_t text);

namespace main_bridge {
std::string run,suite,artifact;
uint64_t map_generation=0,tick=0,window_tick=0;
std::vector<s2khook::MainBridgeObservation> rows;
s2khook::MainBridgeObservation* active=nullptr;
s2khook::MainBridgeObservation window;
void* expected_receiver=nullptr;
using FrameRead=bool (*)(S2NamedPrecacheFrameV1*,size_t);
FrameRead read_main_frame=nullptr;
void* main_handle=nullptr;
s2khook::PrecacheTokens tokens;

template<int O> KHook::Return<void> PeerVoid(void*);
template<int O> KHook::Return<void> PostVoid(void*);
template<int O> KHook::Return<void> PeerNarrow(void*,float,int32_t,int32_t,int32_t);
template<int O> KHook::Return<void> PostNarrow(void*,float,int32_t,int32_t,int32_t);
template<int O> KHook::Return<void> PeerWide(void*,float,int32_t,int64_t,int64_t);
template<int O> KHook::Return<void> PostWide(void*,float,int32_t,int64_t,int64_t);
template<int O> KHook::Return<int32_t> PeerAcquire(void*,int64_t,int32_t,int64_t);
template<int O> KHook::Return<int32_t> PostAcquire(void*,int64_t,int32_t,int64_t);
template<int O> KHook::Return<int32_t> WitnessAcquire(void*,int64_t,int32_t,int64_t);
template<int O> KHook::Return<void> PeerHud(void*,int64_t,int64_t,int64_t);
template<int O> KHook::Return<void> PostHud(void*,int64_t,int64_t,int64_t);
S2CheckedFunction<void,void*> peer_void[2]={{&PeerVoid<0>,&PostVoid<0>},{&PeerVoid<1>,&PostVoid<1>}};
S2CheckedFunction<void,void*,float,int32_t,int32_t,int32_t> peer_narrow[2]={{&PeerNarrow<0>,&PostNarrow<0>},{&PeerNarrow<1>,&PostNarrow<1>}};
S2CheckedFunction<void,void*,float,int32_t,int64_t,int64_t> peer_wide[2]={{&PeerWide<0>,&PostWide<0>},{&PeerWide<1>,&PostWide<1>}};
S2CheckedFunction<int32_t,void*,int64_t,int32_t,int64_t> peer_acquire[2]={{&PeerAcquire<0>,&PostAcquire<0>},{&PeerAcquire<1>,&PostAcquire<1>}};
S2CheckedFunction<int32_t,void*,int64_t,int32_t,int64_t> witness_acquire[2]={{nullptr,&WitnessAcquire<0>},{nullptr,&WitnessAcquire<1>}};
S2CheckedFunction<void,void*,int64_t,int64_t,int64_t> peer_hud[2]={{&PeerHud<0>,&PostHud<0>},{&PeerHud<1>,&PostHud<1>}};

template<int O,typename Binding> void ObservePeer(Binding& binding,bool post) {
    auto observed=binding.Observe();
    if (!S2Hook_EnterDispatch(observed) || !active || active->order!=O) return;
    if (post) { ++active->peer_post; active->trace+='Q'; }
    else { ++active->peer_pre; active->trace+='P'; }
}
template<int O> KHook::Return<void> PeerVoid(void* self) {
    ObservePeer<O>(peer_void[O],false); return S2_Ignore();
}
template<int O> KHook::Return<void> PostVoid(void* self) {
    ObservePeer<O>(peer_void[O],true); return S2_Ignore();
}
template<int O> KHook::Return<void> PeerNarrow(void* self,float value,int32_t a,int32_t b,int32_t c) {
    ObservePeer<O>(peer_narrow[O],false); return S2_Ignore();
}
template<int O> KHook::Return<void> PostNarrow(void* self,float value,int32_t a,int32_t b,int32_t c) {
    ObservePeer<O>(peer_narrow[O],true); return S2_Ignore();
}
template<int O> KHook::Return<void> PeerWide(void* self,float value,int32_t a,int64_t b,int64_t c) {
    ObservePeer<O>(peer_wide[O],false); return S2_Ignore();
}
template<int O> KHook::Return<void> PostWide(void* self,float value,int32_t a,int64_t b,int64_t c) {
    ObservePeer<O>(peer_wide[O],true); return S2_Ignore();
}
template<int O> KHook::Return<int32_t> PeerAcquire(void* self,int64_t a,int32_t method,int64_t b) {
    ObservePeer<O>(peer_acquire[O],false); return S2_Ignore(int32_t{0});
}
template<int O> KHook::Return<int32_t> PostAcquire(void* self,int64_t a,int32_t method,int64_t b) {
    ObservePeer<O>(peer_acquire[O],true); return S2_Ignore(int32_t{0});
}
template<int O> KHook::Return<void> PeerHud(void* self,int64_t controller,int64_t layout,int64_t text) {
    ObservePeer<O>(peer_hud[O],false); return S2_Ignore();
}
template<int O> KHook::Return<void> PostHud(void* self,int64_t controller,int64_t layout,int64_t text) {
    ObservePeer<O>(peer_hud[O],true); return S2_Ignore();
}
template<int O> KHook::Return<int32_t> WitnessAcquire(void*,int64_t,int32_t,int64_t) {
    auto observed=witness_acquire[O].Observe();
    if (S2Hook_EnterDispatch(observed) && active && active->order==O) {
        active->effective=KHook::GetCurrentReturn<int32_t>();
        active->skipped=KHook::WasOriginalFunctionSkipped() ? 1 : 0;
        active->trace+='W';
    }
    return S2_Ignore(int32_t{0});
}
void Original(int order) {
    if (active && active->order==order) { ++active->original; active->trace+='O'; }
}
void VoidBody(int order,void*) { Original(order); }
void NarrowBody(int order,void*,float value,int32_t a,int32_t b,int32_t c) {
    Original(order); if (!active) return;
    active->value=value; active->a=a; active->b=b; active->c=c;
}
void WideBody(int order,void*,float value,int32_t a,int64_t b,int64_t c) {
    Original(order); if (!active) return;
    active->value=value; active->a=a;
    active->opaque_a=reinterpret_cast<void*>(b)==expected_receiver;
    active->opaque_b=reinterpret_cast<void*>(c)==expected_receiver;
}
int32_t AcquireBody(int order,void*,int64_t a,int32_t method,int64_t b) {
    Original(order); if (!active) return 0;
    active->a=method; active->opaque_a=reinterpret_cast<void*>(a)==expected_receiver;
    active->opaque_b=reinterpret_cast<void*>(b)==expected_receiver;
    return active->scenario==6 ? 6 : 0;
}
void HudBody(int order,void* self,int64_t controller,int64_t layout,int64_t text) {
    Original(order); if (!active) return;
    active->hud_self=self==expected_receiver;
    active->hud_controller=reinterpret_cast<void*>(controller)==expected_receiver;
    active->hud_layout=reinterpret_cast<void*>(layout)==expected_receiver;
    // This is a borrowed first-word text holder (utlstring), NEVER std::string.
    const char* value=nullptr;
    if (text) std::memcpy(&value,reinterpret_cast<const void*>(text),sizeof value);
    if (value) active->text.assign(value,strnlen(value,256));
}
std::array<S2CheckedBindingOps*,12> Inventory() {
    return {{&peer_void[0],&peer_void[1],&peer_narrow[0],&peer_narrow[1],&peer_wide[0],&peer_wide[1],
             &peer_acquire[0],&peer_acquire[1],&peer_hud[0],&peer_hud[1],&witness_acquire[0],&witness_acquire[1]}};
}
bool Configure(int order) {
    if (order==0) return peer_void[0].Configure(&S2ProbeBridgeVoid0).Accepted() &&
        peer_narrow[0].Configure(&S2ProbeBridgeNarrow0).Accepted() && peer_wide[0].Configure(&S2ProbeBridgeWide0).Accepted() &&
        peer_acquire[0].Configure(&S2ProbeBridgeAcquire0).Accepted() && peer_hud[0].Configure(&S2ProbeBridgeHud0).Accepted();
    return peer_void[1].Configure(&S2ProbeBridgeVoid1).Accepted() &&
        peer_narrow[1].Configure(&S2ProbeBridgeNarrow1).Accepted() && peer_wide[1].Configure(&S2ProbeBridgeWide1).Accepted() &&
        peer_acquire[1].Configure(&S2ProbeBridgeAcquire1).Accepted() && peer_hud[1].Configure(&S2ProbeBridgeHud1).Accepted();
}
bool Frame(S2NamedPrecacheFrameV1& out) {
    out={}; return read_main_frame && read_main_frame(&out,sizeof out) && out.version==1 && out.size==sizeof out;
}
} // namespace main_bridge
extern "C" __attribute__((noinline)) void S2ProbeBridgeVoid0Body(void* self) { return main_bridge::VoidBody(0,self); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeVoid0(void* self) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x10,0x42\n\tjmp S2ProbeBridgeVoid0Body");
}
#else
extern "C" void S2ProbeBridgeVoid0(void* self) { return S2ProbeBridgeVoid0Body(self); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeNarrow0Body(void* self,float value,int32_t a,int32_t b,int32_t c) { return main_bridge::NarrowBody(0,self,value,a,b,c); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeNarrow0(void* self,float value,int32_t a,int32_t b,int32_t c) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x11,0x42\n\tjmp S2ProbeBridgeNarrow0Body");
}
#else
extern "C" void S2ProbeBridgeNarrow0(void* self,float value,int32_t a,int32_t b,int32_t c) { return S2ProbeBridgeNarrow0Body(self,value,a,b,c); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeWide0Body(void* self,float value,int32_t a,int64_t b,int64_t c) { return main_bridge::WideBody(0,self,value,a,b,c); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeWide0(void* self,float value,int32_t a,int64_t b,int64_t c) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x12,0x42\n\tjmp S2ProbeBridgeWide0Body");
}
#else
extern "C" void S2ProbeBridgeWide0(void* self,float value,int32_t a,int64_t b,int64_t c) { return S2ProbeBridgeWide0Body(self,value,a,b,c); }
#endif
extern "C" __attribute__((noinline)) int32_t S2ProbeBridgeAcquire0Body(void* self,int64_t a,int32_t method,int64_t b) { return main_bridge::AcquireBody(0,self,a,method,b); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int32_t S2ProbeBridgeAcquire0(void* self,int64_t a,int32_t method,int64_t b) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x13,0x42\n\tjmp S2ProbeBridgeAcquire0Body");
}
#else
extern "C" int32_t S2ProbeBridgeAcquire0(void* self,int64_t a,int32_t method,int64_t b) { return S2ProbeBridgeAcquire0Body(self,a,method,b); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeHud0Body(void* self,int64_t controller,int64_t layout,int64_t text) { return main_bridge::HudBody(0,self,controller,layout,text); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeHud0(void* self,int64_t controller,int64_t layout,int64_t text) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x14,0x42\n\tjmp S2ProbeBridgeHud0Body");
}
#else
extern "C" void S2ProbeBridgeHud0(void* self,int64_t controller,int64_t layout,int64_t text) { return S2ProbeBridgeHud0Body(self,controller,layout,text); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeVoid1Body(void* self) { return main_bridge::VoidBody(1,self); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeVoid1(void* self) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x18,0x42\n\tjmp S2ProbeBridgeVoid1Body");
}
#else
extern "C" void S2ProbeBridgeVoid1(void* self) { return S2ProbeBridgeVoid1Body(self); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeNarrow1Body(void* self,float value,int32_t a,int32_t b,int32_t c) { return main_bridge::NarrowBody(1,self,value,a,b,c); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeNarrow1(void* self,float value,int32_t a,int32_t b,int32_t c) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x19,0x42\n\tjmp S2ProbeBridgeNarrow1Body");
}
#else
extern "C" void S2ProbeBridgeNarrow1(void* self,float value,int32_t a,int32_t b,int32_t c) { return S2ProbeBridgeNarrow1Body(self,value,a,b,c); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeWide1Body(void* self,float value,int32_t a,int64_t b,int64_t c) { return main_bridge::WideBody(1,self,value,a,b,c); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeWide1(void* self,float value,int32_t a,int64_t b,int64_t c) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x1a,0x42\n\tjmp S2ProbeBridgeWide1Body");
}
#else
extern "C" void S2ProbeBridgeWide1(void* self,float value,int32_t a,int64_t b,int64_t c) { return S2ProbeBridgeWide1Body(self,value,a,b,c); }
#endif
extern "C" __attribute__((noinline)) int32_t S2ProbeBridgeAcquire1Body(void* self,int64_t a,int32_t method,int64_t b) { return main_bridge::AcquireBody(1,self,a,method,b); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int32_t S2ProbeBridgeAcquire1(void* self,int64_t a,int32_t method,int64_t b) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x1b,0x42\n\tjmp S2ProbeBridgeAcquire1Body");
}
#else
extern "C" int32_t S2ProbeBridgeAcquire1(void* self,int64_t a,int32_t method,int64_t b) { return S2ProbeBridgeAcquire1Body(self,a,method,b); }
#endif
extern "C" __attribute__((noinline)) void S2ProbeBridgeHud1Body(void* self,int64_t controller,int64_t layout,int64_t text) { return main_bridge::HudBody(1,self,controller,layout,text); }
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) void S2ProbeBridgeHud1(void* self,int64_t controller,int64_t layout,int64_t text) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x1c,0x42\n\tjmp S2ProbeBridgeHud1Body");
}
#else
extern "C" void S2ProbeBridgeHud1(void* self,int64_t controller,int64_t layout,int64_t text) { return S2ProbeBridgeHud1Body(self,controller,layout,text); }
#endif

extern "C" __attribute__((noinline)) int32_t S2ProbeBridgeDriveBody(void* self,int32_t encoded,int32_t sequence,int32_t generation,const char* run) {
    using namespace main_bridge;
    const int order=encoded/100,scenario=encoded%100;
    if (!run || main_bridge::run!=run || artifact.empty() || suite!="B" || active || !self || sequence<=0 || generation<=0 || order<0 || order>1 || scenario<1 || scenario>13) return -1;
    for (const auto& row:rows) if (row.sequence==sequence && row.generation==generation) return -1;
    s2khook::MainBridgeObservation observation;
    observation.scenario=scenario; observation.sequence=sequence; observation.generation=generation; observation.order=order;
    active=&observation; expected_receiver=self;
    const int64_t ptr=reinterpret_cast<int64_t>(self);
    if (scenario<=2 || scenario==10 || scenario==11) {
        void (*volatile fn)(void*)=order ? &S2ProbeBridgeVoid1 : &S2ProbeBridgeVoid0; fn(self);
    } else if (scenario==3) {
        void (*volatile fn)(void*,float,int32_t,int32_t,int32_t)=order ? &S2ProbeBridgeNarrow1 : &S2ProbeBridgeNarrow0; fn(self,1.5f,3,4,5);
    } else if (scenario==4) {
        void (*volatile fn)(void*,float,int32_t,int64_t,int64_t)=order ? &S2ProbeBridgeWide1 : &S2ProbeBridgeWide0; fn(self,1.5f,3,ptr,ptr);
    } else if (scenario<=7) {
        int32_t (*volatile fn)(void*,int64_t,int32_t,int64_t)=order ? &S2ProbeBridgeAcquire1 : &S2ProbeBridgeAcquire0;
        observation.result=fn(self,ptr,12,ptr);
    } else if (scenario<=9) {
        const char* text="s2-khook-hud";
        void (*volatile fn)(void*,int64_t,int64_t,int64_t)=order ? &S2ProbeBridgeHud1 : &S2ProbeBridgeHud0;
        fn(self,ptr,ptr,reinterpret_cast<int64_t>(&text));
    }
    rows.push_back(observation); active=nullptr; expected_receiver=nullptr;
    return observation.result;
}
extern "C" __attribute__((noinline)) int32_t S2ProbeBridgeMarkBody(const char* run,int32_t scenario,int32_t sequence,int32_t generation,int32_t phase,int32_t order) {
    using namespace main_bridge;
    if (!run || main_bridge::run!=run || !active || artifact.empty() || suite!="B" || phase!=1 ||
        active->scenario!=scenario || active->sequence!=sequence || active->generation!=generation || active->order!=order ||
        (active==&window && tick!=window_tick)) return 0;
    ++active->callbacks; active->trace+='J'; return sequence;
}
extern "C" __attribute__((noinline)) bool S2ProbeBridgeWindowBody(const char* run,int32_t encoded,int32_t sequence,int32_t generation,bool open) {
    using namespace main_bridge;
    if (!run || main_bridge::run!=run || artifact.empty() || suite!="B" || sequence<=0 || generation<=0) return false;
    if (open) {
        if (active || encoded/100<0 || encoded/100>1 || (encoded%100!=12 && encoded%100!=13)) return false;
        for (const auto& row:rows) if (row.sequence==sequence && row.generation==generation) return false;
        window={}; window.scenario=encoded%100; window.order=encoded/100; window.sequence=sequence; window.generation=generation;
        window_tick=tick; active=&window; return true;
    }
    if (active!=&window || window.sequence!=sequence || window.generation!=generation || window.scenario!=encoded%100 || window.order!=encoded/100 || tick!=window_tick) return false;
    rows.push_back(window); active=nullptr; return true;
}
extern "C" __attribute__((noinline)) int32_t S2ProbePrecacheBeginBody(const char* run,int32_t generation,int32_t) {
    using namespace main_bridge;
    S2NamedPrecacheFrameV1 frame;
    if (!run || suite!="C" || artifact.empty() || !Frame(frame)) return 0;
    return tokens.Begin(run,generation,static_cast<int>(map_generation),frame);
}
extern "C" __attribute__((noinline)) bool S2ProbePrecacheFinishBody(int32_t token,const char* resource,bool added,int32_t generation) {
    using namespace main_bridge;
    S2NamedPrecacheFrameV1 frame;
    if (suite!="C" || artifact.empty() || !resource || !Frame(frame)) return false;
    return tokens.Finish(run,token,generation,frame,resource,added);
}
extern "C" __attribute__((noinline)) int32_t S2ProbePrecacheReadBody(int32_t token,int32_t field) {
    using namespace main_bridge;
    S2NamedPrecacheFrameV1 frame;
    return Frame(frame) ? tokens.Read(token,field,frame) : 0;
}
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int32_t S2ProbeBridgeDrive(void* self,int32_t scenario,int32_t sequence,int32_t generation,const char* run) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x01,0x42\n\tjmp S2ProbeBridgeDriveBody");
}
#else
extern "C" int32_t S2ProbeBridgeDrive(void* self,int32_t scenario,int32_t sequence,int32_t generation,const char* run) { return S2ProbeBridgeDriveBody(self,scenario,sequence,generation,run); }
#endif
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int32_t S2ProbeBridgeMark(const char* run,int32_t scenario,int32_t sequence,int32_t generation,int32_t phase,int32_t order) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x02,0x42\n\tjmp S2ProbeBridgeMarkBody");
}
#else
extern "C" int32_t S2ProbeBridgeMark(const char* run,int32_t scenario,int32_t sequence,int32_t generation,int32_t phase,int32_t order) { return S2ProbeBridgeMarkBody(run,scenario,sequence,generation,phase,order); }
#endif
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) bool S2ProbeBridgeWindow(const char* run,int32_t scenario,int32_t sequence,int32_t generation,bool open) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x03,0x42\n\tjmp S2ProbeBridgeWindowBody");
}
#else
extern "C" bool S2ProbeBridgeWindow(const char* run,int32_t scenario,int32_t sequence,int32_t generation,bool open) { return S2ProbeBridgeWindowBody(run,scenario,sequence,generation,open); }
#endif
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int32_t S2ProbePrecacheBegin(const char* run,int32_t generation,int32_t map) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x04,0x42\n\tjmp S2ProbePrecacheBeginBody");
}
#else
extern "C" int32_t S2ProbePrecacheBegin(const char* run,int32_t generation,int32_t map) { return S2ProbePrecacheBeginBody(run,generation,map); }
#endif
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) bool S2ProbePrecacheFinish(int32_t token,const char* resource,bool added,int32_t generation) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x05,0x42\n\tjmp S2ProbePrecacheFinishBody");
}
#else
extern "C" bool S2ProbePrecacheFinish(int32_t token,const char* resource,bool added,int32_t generation) { return S2ProbePrecacheFinishBody(token,resource,added,generation); }
#endif
#if defined(__linux__) && defined(__x86_64__)
extern "C" __attribute__((naked,noinline)) int32_t S2ProbePrecacheRead(int32_t token,int32_t field) {
    asm volatile(".byte 0x0f,0x1f,0x84,0x00,0x53,0x36,0x06,0x42\n\tjmp S2ProbePrecacheReadBody");
}
#else
extern "C" int32_t S2ProbePrecacheRead(int32_t token,int32_t field) { return S2ProbePrecacheReadBody(token,field); }
#endif

bool S2ProbeBridgeInstallEarly(std::string& reason) {
    const bool ok=main_bridge::Configure(0);
    reason=ok ? "early bridge peers accepted; actual callback order pending" : "early bridge peer rejected";
    return ok;
}
bool S2ProbeBridgePrepare(const std::string& run,const std::string& suite,const std::string& artifact,
                          const std::string& measured_main_path,uint64_t map_generation) {
    main_bridge::run=run; main_bridge::suite=suite; main_bridge::artifact=artifact;
    main_bridge::rows.clear(); main_bridge::active=nullptr; main_bridge::tokens.Reset(run);
    main_bridge::map_generation=map_generation; main_bridge::read_main_frame=nullptr;
    if (suite=="B") return main_bridge::Configure(1) &&
        main_bridge::witness_acquire[0].Configure(&S2ProbeBridgeAcquire0).Accepted() &&
        main_bridge::witness_acquire[1].Configure(&S2ProbeBridgeAcquire1).Accepted();
    if (suite!="C" || measured_main_path.empty()) return false;
    // The caller supplies the measured unique main-module path. Never RTLD_DEFAULT:
    // this DSO also contains a private production named_hooks.cpp for mechanics.
    void* handle=dlopen(measured_main_path.c_str(),RTLD_NOW|RTLD_NOLOAD);
    if (!handle) return false;
    auto* symbol=dlsym(handle,"S2NamedReadPrecacheFrameV1");
    Dl_info info{};
    struct stat wanted{},found{};
    if (!symbol || !dladdr(symbol,&info) || !info.dli_fname ||
        stat(measured_main_path.c_str(),&wanted) || stat(info.dli_fname,&found) ||
        wanted.st_dev!=found.st_dev || wanted.st_ino!=found.st_ino) { dlclose(handle); return false; }
    Dl_info probe{};
    if (!dladdr(reinterpret_cast<void*>(&S2ProbeBridgePrepare),&probe) || info.dli_fbase==probe.dli_fbase) { dlclose(handle); return false; }
    if (main_bridge::main_handle) dlclose(main_bridge::main_handle);
    main_bridge::main_handle=handle;
    main_bridge::read_main_frame=reinterpret_cast<main_bridge::FrameRead>(symbol);
    return true;
}
void S2ProbeBridgeWorld(uint64_t map_generation,uint64_t tick) {
    main_bridge::map_generation=map_generation; main_bridge::tick=tick;
    if (main_bridge::active==&main_bridge::window && main_bridge::window_tick!=tick) main_bridge::active=nullptr;
}
const std::vector<s2khook::MainBridgeObservation>& S2ProbeBridgeCollect() { return main_bridge::rows; }
const std::vector<s2khook::PrecacheTokenObservation>& S2ProbePrecacheCollect() { return main_bridge::tokens.Rows(); }
bool S2ProbeBridgeCanUnloadSync(const S2HookTerminalPermit& permit) {
    auto inventory=main_bridge::Inventory();
    return !main_bridge::active && S2HookInventoryCanRemoveSync(inventory,permit);
}
bool S2ProbeBridgeUnloadSync(const S2HookTerminalPermit& permit) {
    auto inventory=main_bridge::Inventory();
    return S2ProbeBridgeCanUnloadSync(permit) && S2HookInventoryBeginRemoveSync(inventory,permit);
}

bool S2ProbeBridgeBind(const std::string& run,const std::string& suite,const std::string& artifact) {
    if (main_bridge::run!=run || main_bridge::suite!=suite || artifact.empty() ||
        (!main_bridge::artifact.empty() && main_bridge::artifact!=artifact)) return false;
    main_bridge::artifact=artifact; return true;
}
