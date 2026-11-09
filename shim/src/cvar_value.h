// cvar_value.h — how many bytes a cvar value really owns in ConVarData::m_Values.
//
// The SDK declares m_Values as sizeof(CVValue_t) * MAX_SPLITSCREEN_CLIENTS (16 * 4) bytes, but the
// engine does not allocate that. tier0's CCvar::RegisterConVar takes each ConVarData from its bump
// arena as offsetof(m_Values) + CVarTypeTraits::m_ByteSize * slots, rounded up to 8 (slots is 1
// unless FCVAR_PER_USER), and then takes the cvar's min, max and default values (m_ByteSize each)
// from the same arena, right behind it. Checked on the CS2 2026-11-05 libtier0. ConVarData::Value()
// strides by m_ByteSize too. So a write of a whole CVValue_t (16 bytes) to an int32 cvar runs 12
// bytes into the next arena block: the cvar's own min, max or default value, or the next cvar.
//
// Every write or copy of a cvar value must therefore use s2cvar::value_size. Engine-free on
// purpose (no SDK include) so tests/cvar_value_test.cpp can run on the host; s2script_mm.cpp
// static_asserts that the numbering below matches EConVarType.
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace s2cvar {

// Same numbering as EConVarType (public/tier0/convar.h).
enum Type : int {
    Bool = 0,
    Int16 = 1,
    UInt16 = 2,
    Int32 = 3,
    UInt32 = 4,
    Int64 = 5,
    UInt64 = 6,
    Float32 = 7,
    Float64 = 8,
};

// CVarTypeTraits::m_ByteSize for the scalar types cvar_set writes; 0 for every other type
// (string, colour, vectors), which cvar_set must not store through this path.
inline size_t value_size(int ty) {
    switch (ty) {
        case Bool:    return sizeof(bool);
        case Int16:
        case UInt16:  return sizeof(int16_t);
        case Int32:
        case UInt32:  return sizeof(int32_t);
        case Float32: return sizeof(float);
        case Int64:
        case UInt64:  return sizeof(int64_t);
        case Float64: return sizeof(double);
        default:      return 0;
    }
}

// Copy one value of type `ty` from `src` into `dst`, touching exactly value_size(ty) bytes.
// Returns false (and copies nothing) for a type value_size does not cover.
inline bool copy_value(void* dst, const void* src, int ty) {
    const size_t n = value_size(ty);
    if (n == 0) return false;
    memcpy(dst, src, n);
    return true;
}

} // namespace s2cvar
