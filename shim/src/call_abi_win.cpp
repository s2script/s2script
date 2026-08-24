#include "call_abi.h"

#include <cstring>
#if !defined(_WIN32)
#include <type_traits>
#endif

#if !defined(_WIN32) && !defined(S2_ABI_PROBE_MICROSOFT)
#error "call_abi_win.cpp is production Windows code; Linux may compile it only as an explicit ABI probe"
#endif

#if defined(S2_ABI_PROBE_MICROSOFT) && !defined(_WIN32)
#define S2_MS_ABI __attribute__((ms_abi))
#endif

namespace s2callabi {
namespace {

constexpr int kMaxGpArgs = 9;
constexpr int kMaxFpArgs = 8;
constexpr int kMaxPositions = kMaxGpArgs + kMaxFpArgs + 1;  // optional receiver

struct Slot {
    uint64_t bits = 0;
    ArgClass cls = kArgGp;
};

uint64_t FloatBits(float value) {
    uint32_t bits = 0;
    std::memcpy(&bits, &value, sizeof bits);
    return bits;
}

bool BuildSlots(const InvokeRequest& r, Slot (&slots)[kMaxPositions]) {
    if (!r.fn || r.argCount < 0 || r.gpCount < 0 || r.fpCount < 0) return false;
    if (r.argCount > kMaxGpArgs + kMaxFpArgs ||
        r.gpCount > kMaxGpArgs || r.fpCount > kMaxFpArgs) return false;
    if (r.argCount > 0 && !r.argClasses) return false;
    if (r.gpCount > 0 && !r.gp) return false;
    if (r.fpCount > 0 && !r.fp) return false;

    int pos = 0, gp = 0, fp = 0;
    if (r.hasReceiver) slots[pos++] = Slot{ r.receiver, kArgGp };
    for (int i = 0; i < r.argCount; i++, pos++) {
        if (r.argClasses[i] == kArgGp) {
            if (gp >= r.gpCount) return false;
            slots[pos] = Slot{ r.gp[gp++], kArgGp };
        } else if (r.argClasses[i] == kArgF32) {
            if (fp >= r.fpCount) return false;
            slots[pos] = Slot{ FloatBits(r.fp[fp++]), kArgF32 };
        } else {
            return false;
        }
    }
    return gp == r.gpCount && fp == r.fpCount;
}

#if !defined(_WIN32)
// Linux-only behavioural scaffolding. GCC's explicit ms_abi support lets the gate exercise
// Microsoft register/shadow/stack placement without pretending this mismatched prototype is a
// production-safe invocation mechanism. The real Windows build always uses the MASM helper below.
template <typename T>
T SlotValue(const Slot& slot) {
    if constexpr (std::is_same_v<T, float>) {
        const uint32_t bits = static_cast<uint32_t>(slot.bits);
        float value = 0.0f;
        std::memcpy(&value, &bits, sizeof value);
        return value;
    } else {
        return slot.bits;
    }
}

// Microsoft x64's first four positions select paired GP/XMM registers by STATIC parameter type.
// Generate all sixteen first-four type patterns so MSVC performs that assignment and reserves the
// mandatory 32-byte shadow space. Positions 4+ are always stack slots of eight bytes; carrying their
// raw low-bit payload as uint64_t preserves both GP values and 32-bit float values while leaving all
// stack placement to the compiler.
template <typename R, typename A0, typename A1, typename A2, typename A3>
R InvokeTyped(void* raw, const Slot (&s)[kMaxPositions]) {
    using Fn = R (S2_MS_ABI *)(A0, A1, A2, A3,
                               uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                               uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                               uint64_t, uint64_t);
    const Fn fn = reinterpret_cast<Fn>(raw);
    return fn(SlotValue<A0>(s[0]), SlotValue<A1>(s[1]),
              SlotValue<A2>(s[2]), SlotValue<A3>(s[3]),
              s[4].bits, s[5].bits, s[6].bits, s[7].bits, s[8].bits, s[9].bits,
              s[10].bits, s[11].bits, s[12].bits, s[13].bits, s[14].bits, s[15].bits,
              s[16].bits, s[17].bits);
}

template <typename R>
R Dispatch(void* fn, const Slot (&s)[kMaxPositions]) {
    const unsigned key =
        (s[0].cls == kArgF32 ? 1u : 0u) |
        (s[1].cls == kArgF32 ? 2u : 0u) |
        (s[2].cls == kArgF32 ? 4u : 0u) |
        (s[3].cls == kArgF32 ? 8u : 0u);
#define S2_CASE(KEY, A0, A1, A2, A3) \
    case KEY: return InvokeTyped<R, A0, A1, A2, A3>(fn, s)
    switch (key) {
        S2_CASE(0x0, uint64_t, uint64_t, uint64_t, uint64_t);
        S2_CASE(0x1, float,    uint64_t, uint64_t, uint64_t);
        S2_CASE(0x2, uint64_t, float,    uint64_t, uint64_t);
        S2_CASE(0x3, float,    float,    uint64_t, uint64_t);
        S2_CASE(0x4, uint64_t, uint64_t, float,    uint64_t);
        S2_CASE(0x5, float,    uint64_t, float,    uint64_t);
        S2_CASE(0x6, uint64_t, float,    float,    uint64_t);
        S2_CASE(0x7, float,    float,    float,    uint64_t);
        S2_CASE(0x8, uint64_t, uint64_t, uint64_t, float);
        S2_CASE(0x9, float,    uint64_t, uint64_t, float);
        S2_CASE(0xA, uint64_t, float,    uint64_t, float);
        S2_CASE(0xB, float,    float,    uint64_t, float);
        S2_CASE(0xC, uint64_t, uint64_t, float,    float);
        S2_CASE(0xD, float,    uint64_t, float,    float);
        S2_CASE(0xE, uint64_t, float,    float,    float);
        S2_CASE(0xF, float,    float,    float,    float);
    }
#undef S2_CASE
    return R{};
}
#endif

}  // namespace

#if defined(_WIN32)
// Implemented in call_abi_win_msvc.asm. It consumes positional raw slots/classes, owns the exact
// Microsoft x64 register and stack layout, and returns RAX/XMM0 bits without ever casting the target
// to a C++ function-pointer type that does not match its declaration.
extern "C" int S2_InvokeMicrosoftX64(void* fn, const uint64_t* slots,
                                     const uint8_t* classes, int count,
                                     int retKind, uint64_t* out);
#endif

bool Invoke(const InvokeRequest& r, uint64_t* out) {
    if (out) *out = 0;
    Slot slots[kMaxPositions] = {};
    if (!BuildSlots(r, slots)) return false;
    if (r.retKind != kRetU64 && r.retKind != kRetF32) return false;

#if defined(_WIN32)
    uint64_t raw[kMaxPositions] = {};
    uint8_t classes[kMaxPositions] = {};
    const int count = r.argCount + (r.hasReceiver ? 1 : 0);
    for (int i = 0; i < count; i++) {
        raw[i] = slots[i].bits;
        classes[i] = static_cast<uint8_t>(slots[i].cls);
    }
    return S2_InvokeMicrosoftX64(r.fn, raw, classes, count,
                                 static_cast<int>(r.retKind), out) != 0;
#else
    if (r.retKind == kRetF32) {
        const float value = Dispatch<float>(r.fn, slots);
        if (out) *out = FloatBits(value);
        return true;
    }
    if (r.retKind != kRetU64) return false;
    const uint64_t value = Dispatch<uint64_t>(r.fn, slots);
    if (out) *out = value;
    return true;
#endif
}

}  // namespace s2callabi
