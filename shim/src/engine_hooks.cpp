// Declarative inbound hooks: exact typed KHook callbacks, retained for the native module's
// resident lifetime. Each PRE owns a stack record through synchronous Recall and KHook POST.
// Only the current invocation view is addressable across the C ABI; nested calls restore it.
#include "engine_hooks.h"

#include "khook_map.h"
#include "engine_calls.h"   // S2_EntityHandleFromPtr — the books-first receiver -> CEntityHandle pack
#include "call_validate.h"   // the arg-width check: does the SHAPE match the callee's machine code?
#include "hook_dispatch.h"

#include <array>
#include <cstddef>
#include <cstdio>
#include <utility>
#include <memory>

namespace {

// Capsules are heap-stable and are destroyed only after checked terminal removal.
struct CallbackCapsule {
    virtual ~CallbackCapsule() = default;
    virtual S2CheckedBindingOps& Ops() = 0;
    virtual S2HookReceipt Configure(const void* address) = 0;
};
template <typename Ret, typename... Args>
struct TypedCapsule final : CallbackCapsule {
    using Target = Ret (*)(Args...);
    using Callback = KHook::Return<Ret> (*)(Args...);
    S2CheckedFunction<Ret, Args...> binding;
    Target target = nullptr;
    TypedCapsule(Callback pre, Callback post) : binding(pre, post) {}
    S2CheckedBindingOps& Ops() override { return binding; }
    S2HookReceipt Configure(const void* address) override {
        target = reinterpret_cast<Target>(const_cast<void*>(address));
        return binding.Configure(target);
    }
};
struct Installed {
    int shape = -1;
    int64_t addr = 0;
    std::unique_ptr<CallbackCapsule> capsule;
};
// No process-exit static destructor may call a provider already torn down by Metamod.
// Normal terminal cleanup empties this resident table explicitly.
auto& g_hooks = *new std::array<Installed, S2_HOOK_MAX>;

// ---------------------------------------------------------------------------
// The block-scoped arg view.
//
// A param is addressed by its POSITIONAL index in the descriptor (0 = the first declared param;
// `this` is never index 0 — the receiver has its own accessor). The shape decides both how many
// params exist and what CLASS each one is, so a stale generated binding asking for an i32 where the
// shape has an f32 fails by -1 instead of reinterpreting the bits.
// ---------------------------------------------------------------------------
// kParamI64 is the OPAQUE PASS-THROUGH class: a parameter carried at full register width because we
// do not know what it is. It has no accessor and no `params` entry, so JS can neither read nor write
// it — its only job is to be handed back to the original function unchanged (see hook_dispatch.h on
// why narrowing one of these segfaulted a live server).
// kParamStr is TEXT copied into the view by the thunk (see ArgView::s). Like kParamI64 it is not
// an f32/i32 slot, but unlike it, it IS surfaced — through hook_read_str rather than an accessor.
enum ParamClass : unsigned char { kParamF32 = 0, kParamI32 = 1, kParamI64 = 2, kParamStr = 3 };
struct ParamSlot { ParamClass cls; unsigned char slot; };

// Sized to the WIDEST shape in the vocabulary; each shape's table is static_asserted to fit, so
// adding a shape that needs more storage fails to COMPILE rather than writing past the view.
constexpr int kViewF32Slots = 1;
constexpr int kViewI32Slots = 3;
constexpr int kViewI64Slots = 3; // HUD relays three native pointers.
constexpr int kReadableI64Slots = 2; // Keep the existing C accessor's exposure unchanged.
static_assert(kViewI64Slots >= 3, "HUD requires three opaque native arguments");

// A hook param that is TEXT. The engine hands these as pointers, and a pointer cannot be surfaced
// through the i32/f32 accessors — narrowing one is the documented segfault. So the thunk COPIES the
// bytes into the view while the frame is alive, and JS reads the copy. Fixed capacity, always
// NUL-terminated: a button id is an author-chosen identifier, not user input, and a name longer
// than this is a design error in the layout rather than something to allocate for.
constexpr int kViewStrSlots = 2;
constexpr int kViewStrCap   = 128;

struct ArgView {
    int     hookId = -1;
    int     shape  = -1;
    void*   self   = nullptr;
    float   f[kViewF32Slots] = {};
    int32_t i[kViewI32Slots] = {};
    int64_t q[kViewI64Slots] = {};   // opaque pass-through; never surfaced to JS
    char    s[kViewStrSlots][kViewStrCap] = {};   // copied text params (see above)
};

struct HookInvocation {
    ArgView view;
    HookInvocation* previous = nullptr;
    bool bypass = false;
    bool voted = false;
    int32_t plugin_result = 0;
};
thread_local std::array<HookInvocation*, S2_HOOK_MAX> g_invocations{};
thread_local const void* g_activeView = nullptr;
class InvocationScope {
public:
    InvocationScope(int id, HookInvocation& invocation)
        : id_(id), invocation_(invocation), previous_view_(g_activeView) {
        invocation.previous = g_invocations[id];
        g_invocations[id] = &invocation;
        g_activeView = &invocation.view;
        invocation.view.hookId = id;
        invocation.view.shape = g_hooks[id].shape;
        invocation.bypass = S2Hook_BypassTake(id);
    }
    ~InvocationScope() {
        g_activeView = previous_view_;
        g_invocations[id_] = invocation_.previous;
    }
    InvocationScope(const InvocationScope&) = delete;
    InvocationScope& operator=(const InvocationScope&) = delete;
private:
    int id_;
    HookInvocation& invocation_;
    const void* previous_view_;
};

// S2_HOOK_SHAPE_THIS_VOID has no params at all; the array exists only because a zero-length array is
// not standard C++, and InfoFor() reports count 0 so no index can ever reach it.
constexpr ParamSlot kParamsThisVoid[] = { { kParamF32, 0 } };
// S2_HOOK_SHAPE_THIS_F32_I32_I32_I32 — void(void* self, float, int, int, int).
constexpr ParamSlot kParamsThisF32I32I32I32[] = {
    { kParamF32, 0 },
    { kParamI32, 0 },
    { kParamI32, 1 },
    { kParamI32, 2 },
};

// S2_HOOK_SHAPE_THIS_F32_I32_I64_I64 — void(void* self, float, int, int64_t, int64_t).
// Only the first TWO are addressable params; the trailing pair are opaque pass-through and are
// deliberately absent from this table, so no accessor index can ever reach them.
constexpr ParamSlot kParamsThisF32I32I64I64[] = {
    { kParamF32, 0 },
    { kParamI32, 0 },
};

// S2_HOOK_SHAPE_THIS_I64_I32_I64 — i32(void* self, int64, int32, int64).
// Addressable: method (i32 slot 0), result (i32 slot 1, the RETURN — not an ABI arg),
// and voted (i32 slot 2, core-only: set when a handler's HookResult is a vote).
// The two i64s are opaque pass-through (item view + unknown).
constexpr ParamSlot kParamsThisI64I32I64[] = {
    { kParamI32, 0 },
    { kParamI32, 1 },
    { kParamI32, 2 },
};

// S2_HOOK_SHAPE_THIS_I64_I64_I64 — void(void* self, int64, int64, int64).
//
// Nothing is addressable through the i32/f32 accessors: all three args are pointers. The two that
// matter reach JS by other routes instead —
//   * WHO clicked: the thunk points the view's `self` at the CCSPlayerController argument, so the
//     existing receiver path books-gates it into an EntityRef. No new machinery.
//   * WHICH button: copied into string slot 0 and read back through hook_read_str.
// The layout pointer is carried at full width and never surfaced (kParamI64's whole purpose).
constexpr ParamSlot kParamsThisI64I64I64[] = {
    { kParamStr, 0 },
};

struct ShapeInfo { const ParamSlot* params; int count; };




// An unknown shape yields count 0, so every accessor index fails: an out-of-vocabulary shape can
// never be read as if it were shape 0, whose wrong ABI is exactly what hook_dispatch.h warns about.
// constexpr so the fits-the-view proof below can be structural rather than hand-maintained.
constexpr ShapeInfo InfoFor(int shape) {
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:            return { kParamsThisVoid, 0 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32: return { kParamsThisF32I32I32I32, 4 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64: return { kParamsThisF32I32I64I64, 2 };
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:     return { kParamsThisI64I32I64, 3 };
        case S2_HOOK_SHAPE_THIS_I64_I64_I64:     return { kParamsThisI64I64I64, 1 };
        default:                                 return { nullptr, 0 };
    }
}

// EVERY shape's params fit the view — checked over the whole id space InfoFor can describe, not per
// shape by hand. A new shape is only reachable once it is added to InfoFor, and the moment it is,
// this assert covers it; there is no separate line for its author to forget. That is what lets the
// accessors index v->f[]/v->i[] with the table's slot and no runtime bound: the bound is proven
// here, for all shapes, at compile time.
constexpr bool ShapeFitsView(int shape) {
    const ShapeInfo si = InfoFor(shape);
    for (int k = 0; k < si.count; k++) {
        const int limit = (si.params[k].cls == kParamF32)   ? kViewF32Slots
                          : (si.params[k].cls == kParamI64) ? kViewI64Slots
                          : (si.params[k].cls == kParamStr) ? kViewStrSlots
                                                            : kViewI32Slots;
        if (static_cast<int>(si.params[k].slot) >= limit) return false;
    }
    return true;
}
constexpr bool AllShapesFitView(int upTo) {
    for (int s = 0; s <= upTo; s++)
        if (!ShapeFitsView(s)) return false;
    return true;
}
// 255 is a deliberate over-scan of the shape id space: a shape InfoFor does not know yields count 0
// and passes trivially, so the bound costs nothing and cannot be outgrown by a new enumerator.
static_assert(AllShapesFitView(255),
              "a shape's params do not fit the ArgView — widen kViewF32Slots/kViewI32Slots");

// THE SHAPE'S TRUE INTEGER-ARGUMENT WIDTHS, slot by slot (slot 0 = `this`, always a pointer).
//
// SEPARATE from the ParamSlot tables above, and that separation is the point: those describe what JS
// can ADDRESS, not what the ABI passes. An opaque kParamI64 has NO ParamSlot entry by design, so
// deriving widths from them omitted it entirely — the arg-width validator's `kParamI64` branch was
// dead code, and the shipped shape only checked out because its opaque slots happen to be TRAILING.
// A shape with a non-trailing opaque i64 would shift every later slot, declaring the pointer narrow
// (refusing a correct hook) and pushing the genuinely narrow slot out of range (never checking it).
//
// A float consumes NO integer slot — it rides in xmm0 — which is why this cannot be positional over
// the declared params either. Written out per shape so it says exactly what the ABI does.
struct ShapeAbi { const uint8_t* wide; int slots; };

constexpr uint8_t kAbiThisVoid[]         = { 1 };             // (this)
constexpr uint8_t kAbiThisF32I32I32I32[] = { 1, 0, 0, 0 };    // (this, [f32], i32, i32, i32)
constexpr uint8_t kAbiThisF32I32I64I64[] = { 1, 0, 1, 1 };    // (this, [f32], i32, i64, i64)
constexpr uint8_t kAbiThisI64I32I64[]    = { 1, 1, 0, 1 };    // (this, i64, i32, i64)
constexpr uint8_t kAbiThisI64I64I64[]    = { 1, 1, 1, 1 };    // (this, i64, i64, i64)

constexpr ShapeAbi AbiFor(int shape) {
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:            return { kAbiThisVoid,         1 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32: return { kAbiThisF32I32I32I32, 4 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64: return { kAbiThisF32I32I64I64, 4 };
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:     return { kAbiThisI64I32I64,    4 };
        case S2_HOOK_SHAPE_THIS_I64_I64_I64:     return { kAbiThisI64I64I64,    4 };
        default:                                 return { nullptr, 0 };
    }
}

// A shape's ABI table must cover at least every ADDRESSABLE non-float param plus `this`. It may
// cover MORE (the opaque slots InfoFor deliberately omits) — that asymmetry is the whole reason both
// tables exist, so the assert is one-sided on purpose. A new shape that forgets its ABI row fails to
// COMPILE rather than being silently checked against a shorter array.
constexpr int AddressableIntSlots(int shape) {
    const ShapeInfo si = InfoFor(shape);
    int n = 1;
    for (int k = 0; k < si.count; k++)
        if (si.params[k].cls != kParamF32) n++;
    return n;
}
constexpr bool AbiCoversShape(int shape) {
    return AbiFor(shape).slots == 0 || AbiFor(shape).slots >= AddressableIntSlots(shape);
}
static_assert(AbiCoversShape(S2_HOOK_SHAPE_THIS_VOID),            "this_void: ABI row too short");
static_assert(AbiCoversShape(S2_HOOK_SHAPE_THIS_F32_I32_I32_I32), "narrow 4-arg: ABI row too short");
static_assert(AbiCoversShape(S2_HOOK_SHAPE_THIS_F32_I32_I64_I64), "wide 4-arg: ABI row too short");
static_assert(AbiCoversShape(S2_HOOK_SHAPE_THIS_I64_I32_I64),     "canaquire: ABI row too short");


// Exact positional mapping is deliberately explicit for each of the five native signatures.
using VoidCapsule = TypedCapsule<void, void*>;
using NarrowCapsule = TypedCapsule<void, void*, float, int32_t, int32_t, int32_t>;
using WideCapsule = TypedCapsule<void, void*, float, int32_t, int64_t, int64_t>;
using AcquireCapsule = TypedCapsule<int32_t, void*, int64_t, int32_t, int64_t>;
using HudCapsule = TypedCapsule<void, void*, int64_t, int64_t, int64_t>;

template <typename Capsule, int Id> Capsule& Binding() {
    return static_cast<Capsule&>(*g_hooks[Id].capsule);
}

template <int Id> KHook::Return<void> PreVoid(void* self) {
    HookInvocation invocation;
    InvocationScope scope(Id, invocation);
    auto& capsule = Binding<VoidCapsule, Id>();
    auto observed = capsule.binding.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    invocation.view.self = self;
    const int action = invocation.bypass ? 0 : S2Hook_Dispatch(Id, &invocation.view);
    return KHook::Recall(capsule.target, S2_FromHookResult(action), self);
}

template <int Id>
KHook::Return<void> PreNarrow(void* self, float f, int32_t a, int32_t b, int32_t c) {
    HookInvocation invocation;
    InvocationScope scope(Id, invocation);
    auto& capsule = Binding<NarrowCapsule, Id>();
    auto observed = capsule.binding.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    auto& v = invocation.view;
    v.self = self; v.f[0] = f; v.i[0] = a; v.i[1] = b; v.i[2] = c;
    const int action = invocation.bypass ? 0 : S2Hook_Dispatch(Id, &v);
    return KHook::Recall(capsule.target, S2_FromHookResult(action), self,
                         v.f[0], v.i[0], v.i[1], v.i[2]);
}

template <int Id>
KHook::Return<void> PreWide(void* self, float f, int32_t a, int64_t b, int64_t c) {
    HookInvocation invocation;
    InvocationScope scope(Id, invocation);
    auto& capsule = Binding<WideCapsule, Id>();
    auto observed = capsule.binding.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    auto& v = invocation.view;
    v.self = self; v.f[0] = f; v.i[0] = a; v.q[0] = b; v.q[1] = c;
    const int action = invocation.bypass ? 0 : S2Hook_Dispatch(Id, &v);
    return KHook::Recall(capsule.target, S2_FromHookResult(action), self,
                         v.f[0], v.i[0], v.q[0], v.q[1]);
}

template <int Id>
KHook::Return<void> PreHud(void* self, int64_t controller, int64_t layout, int64_t text_object) {
    HookInvocation invocation;
    InvocationScope scope(Id, invocation);
    auto& capsule = Binding<HudCapsule, Id>();
    auto observed = capsule.binding.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
    auto& v = invocation.view;
    v.self = reinterpret_cast<void*>(controller);
    v.q[0] = controller; v.q[1] = layout; v.q[2] = text_object;
    int action = 0;
    if (!invocation.bypass) {
        // The engine's libstdc++ string has its data pointer in the first word.
        // Keep a bounded copy before plugin dispatch; never narrow any native pointer.
        if (text_object) {
            const char* text = *reinterpret_cast<const char* const*>(text_object);
            if (text) {
                int n = 0;
                while (n < kViewStrCap - 1 && text[n]) { v.s[0][n] = text[n]; ++n; }
                v.s[0][n] = '\0';
            }
        }
        action = S2Hook_Dispatch(Id, &v);
        // Compatibility completion is intentionally BEFORE the engine, not KHook POST.
        S2Hook_DispatchPost(Id, &v, S2Hook_Suppresses(action) ? 1 : 0);
    }
    return KHook::Recall(capsule.target, S2_FromHookResult(action), self,
                         v.q[0], v.q[1], v.q[2]);
}

template <int Id>
KHook::Return<int32_t> PreAcquire(void* self, int64_t item, int32_t method, int64_t unknown) {
    HookInvocation invocation;
    InvocationScope scope(Id, invocation);
    auto& capsule = Binding<AcquireCapsule, Id>();
    auto observed = capsule.binding.Observe();
    auto& v = invocation.view;
    v.self = self; v.q[0] = item; v.i[0] = method; v.q[1] = unknown;
    // Even a rejected acquisition dispatch holds its record through POST. Otherwise a nested
    // same-ID rejected PRE could leave POST looking at an enclosing invocation's saved vote.
    const bool dispatch = S2Hook_EnterDispatch(observed) && !invocation.bypass;
    invocation.bypass = !dispatch;
    const int action = dispatch ? S2Hook_Dispatch(Id, &v) : 0;
    invocation.voted = v.i[2] != 0;
    invocation.plugin_result = v.i[1];
    const int32_t local = invocation.voted ? invocation.plugin_result : 1;
    return KHook::Recall(capsule.target, S2_FromHookResult(action, local), self,
                         v.q[0], v.i[0], v.q[1]);
}

template <int Id>
KHook::Return<int32_t> PostAcquire(void*, int64_t, int32_t, int64_t) {
    auto observed = Binding<AcquireCapsule, Id>().binding.Observe();
    if (!S2Hook_EnterDispatch(observed)) return S2_Ignore(int32_t{0});
    auto* invocation = g_invocations[Id];
    if (!invocation || invocation->bypass) return S2_Ignore(int32_t{0});
    const bool skipped = KHook::WasOriginalFunctionSkipped();
    if (!skipped) {
        const int32_t engine = KHook::GetOriginalReturn<int32_t>();
        const int32_t folded = invocation->voted
            ? S2Hook_MostRestrictiveAcquire(invocation->plugin_result, engine) : engine;
        if (invocation->voted && folded != engine)
            KHook::ManualReturn(KHook::Return<int32_t>{KHook::Action::Override, folded});
    }
    // ManualReturn submits our vote before JS observes it. Equal/higher peer actions can win;
    // later peer POSTs can still alter the final result after this position in the chain.
    invocation->view.i[1] = KHook::GetCurrentReturn<int32_t>();
    S2Hook_DispatchPost(Id, &invocation->view, skipped ? 1 : 0);
    return S2_Ignore(int32_t{0});
}

template <int Id> std::unique_ptr<CallbackCapsule> MakeCapsule(int shape) {
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:
            return std::make_unique<VoidCapsule>(&PreVoid<Id>, nullptr);
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32:
            return std::make_unique<NarrowCapsule>(&PreNarrow<Id>, nullptr);
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64:
            return std::make_unique<WideCapsule>(&PreWide<Id>, nullptr);
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:
            return std::make_unique<AcquireCapsule>(&PreAcquire<Id>, &PostAcquire<Id>);
        case S2_HOOK_SHAPE_THIS_I64_I64_I64:
            return std::make_unique<HudCapsule>(&PreHud<Id>, nullptr);
        default: return nullptr;
    }
}
template <std::size_t... Ids> constexpr auto CapsuleFactories(std::index_sequence<Ids...>) {
    return std::array<std::unique_ptr<CallbackCapsule> (*)(int), sizeof...(Ids)>{
        &MakeCapsule<static_cast<int>(Ids)>...};
}
constexpr auto kCapsuleFactories = CapsuleFactories(std::make_index_sequence<S2_HOOK_MAX>{});

int Fail(char* out, int cap, const char* reason) {
    if (out && cap > 0) std::snprintf(out, static_cast<size_t>(cap), "%s", reason);
    return -1;
}

ArgView* ViewOf(void* argView) { return static_cast<ArgView*>(argView); }

// The gate every accessor shares: a non-null view that IS the live one. Returns null otherwise, so
// a stale or forged pointer fails by -1 and never reaches v->shape.
ArgView* LiveViewOf(void* argView) {
    if (!argView || argView != g_activeView) return nullptr;
    return ViewOf(argView);
}

}  // namespace

// ---------------------------------------------------------------------------
// Install. Idempotent per hook id: core calls it on every subscribe and only the first patches.
// ---------------------------------------------------------------------------
int S2_HookInstall(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap) {
    if (reasonOut && reasonCap > 0) reasonOut[0] = '\0';

    if (hookId < 0 || hookId >= S2_HOOK_MAX)
        return Fail(reasonOut, reasonCap, "hook id out of range (this build installs at most 64 hooks)");

    const char* shapeName = S2Hook_ShapeName(shape);
    if (!shapeName) {
        char buf[96];
        std::snprintf(buf, sizeof buf, "unknown hook shape id %d", shape);
        return Fail(reasonOut, reasonCap, buf);
    }

    if (!addr) return Fail(reasonOut, reasonCap, "hook target address is null");

    // Live executable membership is separate from verified original instruction reads.
    const uintptr_t target = static_cast<uintptr_t>(addr);
    if (!S2_AddressIsExecutable(reinterpret_cast<const void*>(target)))
        return Fail(reasonOut, reasonCap,
                    "hook target is outside any loaded module's executable range (stale gamedata?)");

    // Already installed for this id: the same (shape, address) is a success no-op — that IS the
    // idempotence core relies on. A DIFFERENT target on a live slot is refused, because installing
    // it would orphan the first detour's trampoline with no way to unpatch it before Unload.
    if (g_hooks[hookId].capsule) {
        if (g_hooks[hookId].shape == shape && g_hooks[hookId].addr == addr) return 0;
        return Fail(reasonOut, reasonCap, "hook id is already installed on a different target");
    }

    // S1 keeps local descriptor IDs exclusive; external KHook consumers may share the target.
    for (int i = 0; i < S2_HOOK_MAX; i++) {
        if (g_hooks[i].capsule && g_hooks[i].addr == addr)
            return Fail(reasonOut, reasonCap, "another hook id is already installed at this address");
    }

    s2resolve::Resolution resolved;
    if (!S2_EngineCallResolutionForAddress(reinterpret_cast<const void*>(target), resolved) ||
        !resolved.image || resolved.address != target || !resolved.image->executable(target))
        return Fail(reasonOut, reasonCap, "hook target has no verified original image");
    const auto image = resolved.image; // copied ownership survives call-table growth and Configure
    s2validate::ModuleView mv;
    mv.read_code = [image](uintptr_t pc, void* out, std::size_t n) { return image->read(pc, out, n); };
    mv.executable = [image](uintptr_t pc, std::size_t n) { return image->executable(pc, n); };
    const ShapeAbi abi = AbiFor(shape);
    char argWidthNote[256]{};
    if (s2validate::ArgWidths(abi.wide, abi.slots, mv, reinterpret_cast<const void*>(target),
                              argWidthNote, sizeof argWidthNote) != 0)
        return Fail(reasonOut, reasonCap, argWidthNote);

    auto& installed = g_hooks[hookId];
    installed.capsule = kCapsuleFactories[hookId](shape);
    if (!installed.capsule) return Fail(reasonOut, reasonCap, "no compiled callback for hook shape");
    installed.shape = shape;
    installed.addr = addr;
    const S2HookReceipt receipt = installed.capsule->Configure(reinterpret_cast<const void*>(target));
    if (!receipt.Accepted()) {
        installed = Installed{};
        return Fail(reasonOut, reasonCap, receipt.reason.c_str());
    }
    if (reasonOut && reasonCap > 0)
        std::snprintf(reasonOut, static_cast<size_t>(reasonCap), "KHook id %llu accepted (Pending); %s",
                      static_cast<unsigned long long>(receipt.id), argWidthNote);
    return 0;
}

void S2_HookArmBypass(int hookId) {
    S2Hook_BypassArm(hookId);   // bounds-checked there; an out-of-range id is a silent no-op
}

// Take-and-discard: "clear the latch whether or not the call consumed it". Deliberately expressed as
// the same one-shot take the thunk performs rather than as a second way to write the slot, so there
// is exactly ONE operation that clears a latch.
void S2_HookDisarmBypass(int hookId) {
    (void)S2Hook_BypassTake(hookId);
}

bool S2EngineHooksCanUnloadSync(const S2HookTerminalPermit& permit) {
    if (!permit.IsValid()) return false;
    for (const auto& hook : g_hooks)
        if (hook.capsule && !hook.capsule->Ops().CanBeginRemove(false, &permit)) return false;
    return true;
}
bool S2EngineHooksUnloadSync(const S2HookTerminalPermit& permit) {
    if (!S2EngineHooksCanUnloadSync(permit)) return false;
    for (const auto& hook : g_hooks)
        if (hook.capsule && !hook.capsule->Ops().BeginRemove(false, &permit)) return false;
    return true;
}
bool S2EngineHooksRemovalComplete() {
    for (const auto& hook : g_hooks)
        if (hook.capsule && !hook.capsule->Ops().RemovalComplete()) return false;
    return true;
}
void S2_HookResetAll(void) {
    // This void ABI cannot report Busy. A premature call is a no-op: never delete an active
    // callback capsule or clear an enclosing view. Terminal inventory owns physical removal.
    if (!S2EngineHooksRemovalComplete()) return;
    for (const auto* invocation : g_invocations) if (invocation) return;
    for (auto& hook : g_hooks) hook = Installed{};
    g_activeView = nullptr;
    S2Hook_BypassResetAll();
}

// ---------------------------------------------------------------------------
// The arg-view accessors. Each one is LIVENESS-gated (the pointer must be the view currently being
// dispatched), then bounds- and class-checked against that view's shape.
// ---------------------------------------------------------------------------
int S2_HookReadF32(void* argView, int idx, float* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamF32) return -1;
    *out = v->f[si.params[idx].slot];
    return 0;
}

int S2_HookReadI32(void* argView, int idx, int32_t* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamI32) return -1;
    *out = v->i[si.params[idx].slot];
    return 0;
}

// Read a TEXT param out of the live view. Same liveness + class gating as the scalar readers: a
// view retained past its dispatch, a bad index, or a param that is not text all fail by -1 rather
// than handing back a stale or misinterpreted buffer.
//
// `out` is always NUL-terminated on success, and the copy is bounded by the SMALLER of the caller's
// capacity and the view's — the string in the view is already NUL-terminated by the thunk, so this
// cannot run off the end even if `cap` lies.
int S2_HookReadStr(void* argView, int idx, char* out, int cap) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out || cap <= 0) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamStr) return -1;
    const char* src = v->s[si.params[idx].slot];
    int n = 0;
    while (n < cap - 1 && n < kViewStrCap - 1 && src[n] != '\0') { out[n] = src[n]; n++; }
    out[n] = '\0';
    return 0;
}

int S2_HookWriteF32(void* argView, int idx, float value) {
    ArgView* v = LiveViewOf(argView);
    if (!v) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamF32) return -1;
    v->f[si.params[idx].slot] = value;
    return 0;
}

int S2_HookWriteI32(void* argView, int idx, int32_t value) {
    ArgView* v = LiveViewOf(argView);
    if (!v) return -1;
    const ShapeInfo si = InfoFor(v->shape);
    if (idx < 0 || idx >= si.count) return -1;
    if (si.params[idx].cls != kParamI32) return -1;
    v->i[si.params[idx].slot] = value;
    return 0;
}

// The receiver, as a packed CEntityHandle. -1 when the shape has no receiver, when it is null, or
// when the entity system's own books do not vouch for it — a detour `this` is frequently NOT an
// entity at all (a rules/services singleton), and that is a normal "no receiver to surface", not an
// error. The packing is engine_calls.cpp's books-FIRST walk (membership decided without reading a
// single byte of the pointer), so a non-entity `this` can never be dereferenced here.
int S2_HookReceiverHandle(void* argView, uint32_t* outHandle) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !outHandle) return -1;
    if (!v->self) return -1;
    const uint32_t h = S2_EntityHandleFromPtr(v->self);
    if (h == S2_ENTITY_HANDLE_NONE) return -1;
    *outHandle = h;
    return 0;
}

int S2_HookReadU16AtQ(void* argView, int qslot, int offset, uint16_t* out) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !out) return -1;
    if (qslot < 0 || qslot >= kReadableI64Slots || offset < 0) return -1;
    const int64_t p = v->q[qslot];
    if (p == 0) return -1;
    *out = *reinterpret_cast<const uint16_t*>(static_cast<uintptr_t>(p) + static_cast<uintptr_t>(offset));
    return 0;
}

int S2_HookSelfMatchesField(void* argView, int index, int serial, int offset) {
    const ArgView* v = LiveViewOf(argView);
    if (!v || !v->self || offset < 0) return 0;
    void* ent = S2_ResolveEntity(index, serial);
    if (!ent) return 0;
    void* field = *reinterpret_cast<void**>(static_cast<char*>(ent) + offset);
    return field == v->self ? 1 : 0;
}
