#include "call_abi.h"

#include <cstring>

namespace s2callabi {
namespace {

constexpr int kMaxGpArgs = 9;
constexpr int kMaxFpArgs = 8;

// SysV assigns integer and SSE classes from independent sequences. Declaring every available slot
// in those two sequences is therefore sufficient: the callee reads its own prefix and ignores the
// rest. Ten GP slots include the optional receiver; slots past r9 are compiler-placed on the stack.
using FnU64 = uint64_t (*)(uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           float, float, float, float, float, float, float, float);
using FnF32 = float    (*)(uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           float, float, float, float, float, float, float, float);

bool Validate(const InvokeRequest& r) {
    if (!r.fn || r.argCount < 0 || r.gpCount < 0 || r.fpCount < 0) return false;
    if (r.argCount > kMaxGpArgs + kMaxFpArgs ||
        r.gpCount > kMaxGpArgs || r.fpCount > kMaxFpArgs) return false;
    if (r.argCount > 0 && !r.argClasses) return false;
    if (r.gpCount > 0 && !r.gp) return false;
    if (r.fpCount > 0 && !r.fp) return false;
    int gp = 0, fp = 0;
    for (int i = 0; i < r.argCount; i++) {
        if (r.argClasses[i] == kArgGp) gp++;
        else if (r.argClasses[i] == kArgF32) fp++;
        else return false;
    }
    return gp == r.gpCount && fp == r.fpCount;
}

}  // namespace

bool Invoke(const InvokeRequest& r, uint64_t* out) {
    if (out) *out = 0;
    if (!Validate(r)) return false;

    uint64_t g[10] = {};
    float f[kMaxFpArgs] = {};
    int base = 0;
    if (r.hasReceiver) {
        g[0] = r.receiver;
        base = 1;
    }
    for (int i = 0; i < r.gpCount; i++) g[base + i] = r.gp[i];
    for (int i = 0; i < r.fpCount; i++) f[i] = r.fp[i];

    if (r.retKind == kRetF32) {
        const float value = reinterpret_cast<FnF32>(r.fn)(
            g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7], g[8], g[9],
            f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
        if (out) {
            uint32_t bits = 0;
            std::memcpy(&bits, &value, sizeof bits);
            *out = bits;
        }
        return true;
    }
    if (r.retKind != kRetU64) return false;
    const uint64_t value = reinterpret_cast<FnU64>(r.fn)(
        g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7], g[8], g[9],
        f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);
    if (out) *out = value;
    return true;
}

}  // namespace s2callabi
