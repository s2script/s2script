#pragma once

#include "abi_registers.h"
#include "hook_dispatch.h"

#include <cstdint>

// Native calling-convention backend for declarative inbound hooks. engine_hooks.cpp owns detour
// installation and named failure policy; this interface owns typed thunks, their block-scoped view,
// and the platform register map used by the arg-width validator.
namespace s2hookabi {

enum ParamClass : uint8_t { kParamF32 = 0, kParamI32 = 1, kParamI64 = 2 };
struct ParamSlot { ParamClass cls; uint8_t slot; };
struct ShapeInfo { const ParamSlot* params; int count; };

constexpr int kViewF32Slots = 1;
constexpr int kViewI32Slots = 3;
constexpr int kViewI64Slots = 2;

struct ArgView {
    int hookId = -1;
    int shape = -1;
    void* self = nullptr;
    float f[kViewF32Slots] = {};
    int32_t i[kViewI32Slots] = {};
    int64_t q[kViewI64Slots] = {};
};

struct ValidatorSpec {
    const uint8_t* wide = nullptr;  // flattened integer slots; 0=i32, 1=i64/pointer
    int slots = 0;
    s2abi::RegisterMap registerMap{};
};

ShapeInfo InfoFor(int shape);
ValidatorSpec ValidatorFor(int shape);

// One compile-time typed thunk per (shape, hook id), compiled under the selected native ABI.
void* ThunkFor(int shape, int hookId);
void SetOriginal(int hookId, void* original);
void Reset();

// The one live stack view during dispatch. Accessors reject every other pointer.
ArgView* LiveViewOf(void* argView);

}  // namespace s2hookabi
