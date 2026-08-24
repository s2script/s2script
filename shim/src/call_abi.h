#pragma once

#include <cstdint>

// Platform calling-convention backend for plugin-declared outbound calls.
//
// engine_calls.cpp owns descriptor resolution, entity liveness and value marshalling. This interface
// owns only the final machine call. The positional `argClasses` sequence is in AUTHOR order and is
// therefore mandatory even though the SysV backend can derive its two independent register streams:
// Microsoft x64 assigns RCX/XMM0, RDX/XMM1, R8/XMM2 and R9/XMM3 by POSITION.
namespace s2callabi {

enum ArgClass : uint8_t {
    kArgGp  = 0,
    kArgF32 = 1,
};

enum ReturnClass : int {
    kRetU64 = 0,
    kRetF32 = 1,
};

struct InvokeRequest {
    void* fn = nullptr;
    bool hasReceiver = false;
    uint64_t receiver = 0;

    const uint8_t* argClasses = nullptr;
    int argCount = 0;

    // Materialized values in per-class author order. A GP value is already a scalar or pointer;
    // floats have already narrowed from the core's f64 wire representation.
    const uint64_t* gp = nullptr;
    int gpCount = 0;
    const float* fp = nullptr;
    int fpCount = 0;

    ReturnClass retKind = kRetU64;
};

// Invoke through this build's native ABI. `out` receives raw RAX bits for kRetU64 or the low
// 32-bit IEEE-754 payload from XMM0 for kRetF32. False means malformed counts/classes; no call ran.
bool Invoke(const InvokeRequest& request, uint64_t* out);

}  // namespace s2callabi
