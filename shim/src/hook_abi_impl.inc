// Included by exactly one selected backend TU. The includer defines:
//   S2_HOOK_ABI        — function ABI annotation (empty on native compilers)
//   S2_HOOK_MICROSOFT  — 0 for SysV, 1 for Microsoft x64

#include "hook_abi.h"

#include <array>
#include <cstddef>
#include <utility>

namespace s2hookabi {
namespace {

void* g_original[S2_HOOK_MAX] = {};
const void* g_activeView = nullptr;

constexpr ParamSlot kParamsThisVoid[] = { { kParamF32, 0 } };
constexpr ParamSlot kParamsThisF32I32I32I32[] = {
    { kParamF32, 0 }, { kParamI32, 0 }, { kParamI32, 1 }, { kParamI32, 2 },
};
constexpr ParamSlot kParamsThisF32I32I64I64[] = {
    { kParamF32, 0 }, { kParamI32, 0 },
};
constexpr ParamSlot kParamsThisI64I32I64[] = {
    { kParamI32, 0 }, { kParamI32, 1 }, { kParamI32, 2 },
};

constexpr uint8_t kAbiThisVoid[] = { 1 };
constexpr uint8_t kAbiThisF32I32I32I32[] = { 1, 0, 0, 0 };
constexpr uint8_t kAbiThisF32I32I64I64[] = { 1, 0, 1, 1 };
constexpr uint8_t kAbiThisI64I32I64[] = { 1, 1, 0, 1 };

constexpr ShapeInfo ShapeInfoFor(int shape) {
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:            return { kParamsThisVoid, 0 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32: return { kParamsThisF32I32I32I32, 4 };
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64: return { kParamsThisF32I32I64I64, 2 };
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:     return { kParamsThisI64I32I64, 3 };
        default:                                 return { nullptr, 0 };
    }
}

constexpr bool ShapeFitsView(int shape) {
    const ShapeInfo si = ShapeInfoFor(shape);
    for (int k = 0; k < si.count; k++) {
        const int limit = si.params[k].cls == kParamF32 ? kViewF32Slots :
                          si.params[k].cls == kParamI64 ? kViewI64Slots : kViewI32Slots;
        if (static_cast<int>(si.params[k].slot) >= limit) return false;
    }
    return true;
}

constexpr bool AllShapesFitView() {
    for (int shape = 0; shape < 256; shape++)
        if (!ShapeFitsView(shape)) return false;
    return true;
}
static_assert(AllShapesFitView(), "a hook shape's params do not fit ArgView");

constexpr s2abi::RegisterMap SysVRegisterMap() {
    s2abi::RegisterMap map{};
    map.slotOfGpr[7] = 0;  // rdi
    map.slotOfGpr[6] = 1;  // rsi
    map.slotOfGpr[2] = 2;  // rdx
    map.slotOfGpr[1] = 3;  // rcx
    map.slotOfGpr[8] = 4;  // r8
    map.slotOfGpr[9] = 5;  // r9
    return map;
}

constexpr s2abi::RegisterMap MicrosoftRegisterMap(int shape) {
    s2abi::RegisterMap map{};
    // Position 0 is `this` for every current shape.
    map.slotOfGpr[1] = 0;  // rcx
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:
            break;
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32:
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64:
            // Position 1 is float -> xmm1; rdx carries no argument. Positions 2/3 use r8/r9.
            map.slotOfGpr[8] = 1;
            map.slotOfGpr[9] = 2;
            break;
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:
            map.slotOfGpr[2] = 1;  // rdx
            map.slotOfGpr[8] = 2;  // r8
            map.slotOfGpr[9] = 3;  // r9
            break;
        default:
            return s2abi::RegisterMap{};
    }
    return map;
}

using ThisVoidFn         = void    (S2_HOOK_ABI *)(void*);
using ThisF32I32I32I32Fn = void    (S2_HOOK_ABI *)(void*, float, int32_t, int32_t, int32_t);
using ThisF32I32I64I64Fn = void    (S2_HOOK_ABI *)(void*, float, int32_t, int64_t, int64_t);
using ThisI64I32I64Fn    = int32_t (S2_HOOK_ABI *)(void*, int64_t, int32_t, int64_t);

template <int Id>
void S2_HOOK_ABI ThunkThisVoid(void* self) {
    const ThisVoidFn orig = reinterpret_cast<ThisVoidFn>(g_original[Id]);
    if (S2Hook_BypassTake(Id)) { if (orig) orig(self); return; }
    ArgView v;
    v.hookId = Id; v.shape = S2_HOOK_SHAPE_THIS_VOID; v.self = self;
    const void* previous = g_activeView;
    g_activeView = &v;
    const int result = S2Hook_Dispatch(Id, &v);
    g_activeView = previous;
    if (S2Hook_Suppresses(result)) return;
    if (orig) orig(self);
}

template <int Id>
void S2_HOOK_ABI ThunkThisF32I32I32I32(
    void* self, float a0, int32_t a1, int32_t a2, int32_t a3) {
    const ThisF32I32I32I32Fn orig = reinterpret_cast<ThisF32I32I32I32Fn>(g_original[Id]);
    if (S2Hook_BypassTake(Id)) { if (orig) orig(self, a0, a1, a2, a3); return; }
    ArgView v;
    v.hookId = Id; v.shape = S2_HOOK_SHAPE_THIS_F32_I32_I32_I32; v.self = self;
    v.f[0] = a0; v.i[0] = a1; v.i[1] = a2; v.i[2] = a3;
    const void* previous = g_activeView;
    g_activeView = &v;
    const int result = S2Hook_Dispatch(Id, &v);
    g_activeView = previous;
    if (S2Hook_Suppresses(result)) return;
    if (orig) orig(self, v.f[0], v.i[0], v.i[1], v.i[2]);
}

template <int Id>
void S2_HOOK_ABI ThunkThisF32I32I64I64(
    void* self, float a0, int32_t a1, int64_t a2, int64_t a3) {
    const ThisF32I32I64I64Fn orig = reinterpret_cast<ThisF32I32I64I64Fn>(g_original[Id]);
    if (S2Hook_BypassTake(Id)) { if (orig) orig(self, a0, a1, a2, a3); return; }
    ArgView v;
    v.hookId = Id; v.shape = S2_HOOK_SHAPE_THIS_F32_I32_I64_I64; v.self = self;
    v.f[0] = a0; v.i[0] = a1; v.q[0] = a2; v.q[1] = a3;
    const void* previous = g_activeView;
    g_activeView = &v;
    const int result = S2Hook_Dispatch(Id, &v);
    g_activeView = previous;
    if (S2Hook_Suppresses(result)) return;
    if (orig) orig(self, v.f[0], v.i[0], v.q[0], v.q[1]);
}

template <int Id>
int32_t S2_HOOK_ABI ThunkThisI64I32I64(
    void* self, int64_t item, int32_t method, int64_t unknown) {
    const ThisI64I32I64Fn orig = reinterpret_cast<ThisI64I32I64Fn>(g_original[Id]);
    if (S2Hook_BypassTake(Id)) return orig ? orig(self, item, method, unknown) : 0;
    ArgView v;
    v.hookId = Id; v.shape = S2_HOOK_SHAPE_THIS_I64_I32_I64; v.self = self;
    v.i[0] = method; v.i[1] = 0; v.i[2] = 0; v.q[0] = item; v.q[1] = unknown;
    const void* previous = g_activeView;
    g_activeView = &v;
    const int result = S2Hook_Dispatch(Id, &v);
    const bool skip = S2Hook_Suppresses(result);
    const int32_t plugin = v.i[1];
    const bool voted = v.i[2] != 0;
    const int32_t out = skip
        ? (voted ? plugin : 1)
        : (voted ? S2Hook_MostRestrictiveAcquire(plugin,
                                                  orig ? orig(self, v.q[0], v.i[0], v.q[1]) : 0)
                 : (orig ? orig(self, v.q[0], v.i[0], v.q[1]) : 0));
    v.i[1] = out;
    S2Hook_DispatchPost(Id, &v, skip ? 1 : 0);
    g_activeView = previous;
    return out;
}

constexpr std::size_t kHookSlots = static_cast<std::size_t>(S2_HOOK_MAX);
template <std::size_t... Is>
constexpr std::array<ThisVoidFn, sizeof...(Is)> MakeThisVoid(std::index_sequence<Is...>) {
    return {{ &ThunkThisVoid<static_cast<int>(Is)>... }};
}
template <std::size_t... Is>
constexpr std::array<ThisF32I32I32I32Fn, sizeof...(Is)>
MakeThisF32I32I32I32(std::index_sequence<Is...>) {
    return {{ &ThunkThisF32I32I32I32<static_cast<int>(Is)>... }};
}
template <std::size_t... Is>
constexpr std::array<ThisF32I32I64I64Fn, sizeof...(Is)>
MakeThisF32I32I64I64(std::index_sequence<Is...>) {
    return {{ &ThunkThisF32I32I64I64<static_cast<int>(Is)>... }};
}
template <std::size_t... Is>
constexpr std::array<ThisI64I32I64Fn, sizeof...(Is)>
MakeThisI64I32I64(std::index_sequence<Is...>) {
    return {{ &ThunkThisI64I32I64<static_cast<int>(Is)>... }};
}

constexpr auto kThisVoid = MakeThisVoid(std::make_index_sequence<kHookSlots>{});
constexpr auto kThisF32I32I32I32 =
    MakeThisF32I32I32I32(std::make_index_sequence<kHookSlots>{});
constexpr auto kThisF32I32I64I64 =
    MakeThisF32I32I64I64(std::make_index_sequence<kHookSlots>{});
constexpr auto kThisI64I32I64 =
    MakeThisI64I32I64(std::make_index_sequence<kHookSlots>{});

}  // namespace

ShapeInfo InfoFor(int shape) { return ShapeInfoFor(shape); }

ValidatorSpec ValidatorFor(int shape) {
    ValidatorSpec spec{};
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:
            spec.wide = kAbiThisVoid; spec.slots = 1; break;
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32:
            spec.wide = kAbiThisF32I32I32I32; spec.slots = 4; break;
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64:
            spec.wide = kAbiThisF32I32I64I64; spec.slots = 4; break;
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:
            spec.wide = kAbiThisI64I32I64; spec.slots = 4; break;
        default:
            return spec;
    }
#if S2_HOOK_MICROSOFT
    spec.registerMap = MicrosoftRegisterMap(shape);
#else
    spec.registerMap = SysVRegisterMap();
#endif
    return spec;
}

void* ThunkFor(int shape, int hookId) {
    if (hookId < 0 || hookId >= S2_HOOK_MAX) return nullptr;
    const std::size_t id = static_cast<std::size_t>(hookId);
    switch (shape) {
        case S2_HOOK_SHAPE_THIS_VOID:
            return reinterpret_cast<void*>(kThisVoid[id]);
        case S2_HOOK_SHAPE_THIS_F32_I32_I32_I32:
            return reinterpret_cast<void*>(kThisF32I32I32I32[id]);
        case S2_HOOK_SHAPE_THIS_F32_I32_I64_I64:
            return reinterpret_cast<void*>(kThisF32I32I64I64[id]);
        case S2_HOOK_SHAPE_THIS_I64_I32_I64:
            return reinterpret_cast<void*>(kThisI64I32I64[id]);
        default:
            return nullptr;
    }
}

void SetOriginal(int hookId, void* original) {
    if (hookId >= 0 && hookId < S2_HOOK_MAX) g_original[hookId] = original;
}

void Reset() {
    for (void*& original : g_original) original = nullptr;
    g_activeView = nullptr;
}

ArgView* LiveViewOf(void* argView) {
    if (!argView || argView != g_activeView) return nullptr;
    return static_cast<ArgView*>(argView);
}

}  // namespace s2hookabi
