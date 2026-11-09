// Drives shim/src/cvar_value.h, the copy Server.setCvar uses to store a cvar value. Run by
// scripts/test-cvar-value.sh from scripts/ci-native.sh.
//
// Regression: setCvar used to memcpy a whole CVValue_t (16 bytes) into the value slot. The engine
// only allocates m_ByteSize per slot and puts the next arena block (the cvar's min/max/default value)
// right behind it, so an int32 write leaked 12 stack bytes past the slot. sv_infinite_ammo ended up
// as -229000626 that way. These tests lay out the engine's arena and check that nothing past the
// slot moves.
#include "cvar_value.h"

#include <cstdio>
#include <cstdint>
#include <cstring>
#include <initializer_list>

static int g_failures = 0;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); ++g_failures; } } while (0)

// offsetof(ConVarData, m_Values) in the pinned SDK and in CS2 2026-11-05 libtier0 (Value(): +0x58).
static const size_t kValuesOffset = 0x58;
static const unsigned char kNext = 0xA5;

static size_t round8(size_t n) { return (n + 7) & ~size_t(7); }

// The scalar types have the engine's m_ByteSize widths.
static void sizes_match_engine_widths() {
    CHECK(s2cvar::value_size(s2cvar::Bool) == 1);
    CHECK(s2cvar::value_size(s2cvar::Int16) == 2);
    CHECK(s2cvar::value_size(s2cvar::UInt16) == 2);
    CHECK(s2cvar::value_size(s2cvar::Int32) == 4);
    CHECK(s2cvar::value_size(s2cvar::UInt32) == 4);
    CHECK(s2cvar::value_size(s2cvar::Float32) == 4);
    CHECK(s2cvar::value_size(s2cvar::Int64) == 8);
    CHECK(s2cvar::value_size(s2cvar::UInt64) == 8);
    CHECK(s2cvar::value_size(s2cvar::Float64) == 8);
    // string (9), colour, vectors and invalid are not stored through this path.
    for (int ty : {-1, 9, 10, 11, 12, 13, 14, 15, 16}) CHECK(s2cvar::value_size(ty) == 0);
}

// One ConVarData block of type `ty` (non-per-user: 1 slot) followed by the next arena block, the
// way RegisterConVar lays them out. A store with a dirty 16-byte source must change only the slot.
static void store_stays_in_slot(int ty) {
    const size_t n = s2cvar::value_size(ty);
    const size_t block = round8(kValuesOffset + n);
    unsigned char arena[0x58 + 8 + 16 + 16];
    memset(arena, kNext, sizeof(arena));
    memset(arena, 0, block); // this cvar's block
    unsigned char* slot = arena + kValuesOffset;

    // A CVValue_t-sized source whose tail is "stack garbage", as an uninitialised CVValue_t has.
    unsigned char src[16];
    memset(src, 0xEE, sizeof(src));
    const int32_t one = 1;
    memcpy(src, &one, n < sizeof(one) ? n : sizeof(one));

    CHECK(s2cvar::copy_value(slot, src, ty));
    CHECK(memcmp(slot, src, n) == 0);
    for (size_t i = block; i < sizeof(arena); ++i) {
        if (arena[i] != kNext) {
            std::fprintf(stderr, "FAIL type %d: byte +0x%zx past the slot changed\n", ty, i);
            ++g_failures;
            break;
        }
    }
    // The slot's own padding inside the 8-byte rounding is untouched as well.
    for (size_t i = kValuesOffset + n; i < block; ++i) CHECK(arena[i] == 0);
}

// The sv_infinite_ammo case: an int32 with its default value in the next arena block. Setting 1, 2
// and back to 0 leaves the default at 0 and the value at exactly what was set.
static void int32_default_survives_set_cycle() {
    unsigned char arena[0x60 + 8];
    memset(arena, 0, sizeof(arena));
    int32_t* value = reinterpret_cast<int32_t*>(arena + kValuesOffset);
    int32_t* deflt = reinterpret_cast<int32_t*>(arena + 0x60);
    for (int32_t v : {1, 2, 0}) {
        unsigned char src[16];
        memset(src, 0x4E, sizeof(src)); // 0x4E4E4E4E if it leaked
        memcpy(src, &v, sizeof(v));
        CHECK(s2cvar::copy_value(value, src, s2cvar::Int32));
        CHECK(*value == v);
        CHECK(*deflt == 0);
    }
}

static void unsupported_type_copies_nothing() {
    unsigned char dst[16];
    unsigned char src[16];
    memset(dst, 0x11, sizeof(dst));
    memset(src, 0x22, sizeof(src));
    CHECK(!s2cvar::copy_value(dst, src, 9)); // string
    for (unsigned char b : dst) CHECK(b == 0x11);
}

int main() {
    sizes_match_engine_widths();
    for (int ty = s2cvar::Bool; ty <= s2cvar::Float64; ++ty) store_stays_in_slot(ty);
    int32_default_survives_set_cycle();
    unsupported_type_copies_nothing();
    if (g_failures) {
        std::fprintf(stderr, "cvar_value_test: %d failure(s)\n", g_failures);
        return 1;
    }
    std::printf("cvar_value_test: ok\n");
    return 0;
}
