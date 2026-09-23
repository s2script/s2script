#include "declarative_fixture.h"
#include "engine_hooks.h"
#include "engine_calls.h"
#include "hook_dispatch.h"
#include "khook_binding.h"
#include <dlfcn.h>
#include <map>

namespace {
std::map<const void*, s2resolve::Resolution> resolutions;
s2khook::DeclarativeSnapshot observation;
enum class Mode { Idle, Void, Mutation, Acquire, Nesting, Bypass };
Mode mode = Mode::Idle;
int receiver;
void* last_view = nullptr;
bool installed = false;
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
        reinterpret_cast<void*>(&S2ProbeDeclarativeWideTarget), reinterpret_cast<void*>(&S2ProbeDeclarativeAcquireTarget)};
    const int shapes[] = {S2_HOOK_SHAPE_THIS_VOID, S2_HOOK_SHAPE_THIS_F32_I32_I64_I64, S2_HOOK_SHAPE_THIS_I64_I32_I64};
    S2Hook_SetOps({&Dispatch, &Post});
    for (int id = 0; id < 3; ++id) {
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
