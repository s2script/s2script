#pragma once
#include <cstdint>

// A compiler-authored member target in a separate test DSO, like an engine
// target separate from Metamod. No ABI adapter or provider lives in that DSO.
struct S2FnMemberFixture {
    std::int32_t base;
    volatile std::uint64_t* original_calls;
    __attribute__((noinline)) std::int32_t Call(std::int32_t value);
};
extern "C" __attribute__((visibility("default"))) void* s2fn_member_fixture_target();
