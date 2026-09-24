#pragma once
#include <cstdint>

// Compiler-authored targets in one separate test DSO, like engine code separate
// from Metamod. Only typed/address accessors cross this boundary. No target body
// is defined here: consumers must never instantiate or preempt a native target.
struct S2FnMemberFixture {
    std::int32_t base;
    volatile std::uint64_t* original_calls;
    __attribute__((noinline, visibility("hidden"))) std::int32_t Call(std::int32_t value);
};
extern "C" __attribute__((visibility("default"))) void* s2fn_member_fixture_target();

using S2FnMixedTarget = double (*)(std::int64_t, double, std::int64_t, double,
    std::int64_t, double, std::int64_t, double, std::int64_t, double,
    std::int64_t, double, std::int64_t, double, double, double, double);
template<class T> using S2FnSpillTarget = T (*)(
    T, T, T, T, T, T, T, T, T, T, T, T, T, T, T, T,
    T, T, T, T, T, T, T, T, T, T, T, T, T, T, T, T);
struct S2FnFixtureTargets {
    bool (*identity_bool)(bool);
    std::uint8_t (*identity_u8)(std::uint8_t);
    std::int32_t (*identity_i32)(std::int32_t);
    std::uint32_t (*identity_u32)(std::uint32_t);
    std::int64_t (*identity_i64)(std::int64_t);
    std::uint64_t (*identity_u64)(std::uint64_t);
    float (*identity_f32)(float);
    double (*identity_f64)(double);
    void* (*identity_ptr)(void*);
    void (*void_target)(std::int32_t);
    S2FnMixedTarget mixed;
    float (*novel)(std::uint32_t, float, void*, std::uint64_t);
    std::uint8_t (*noncanonical)(std::uint8_t);
    S2FnSpillTarget<std::int64_t> spill_gp;
    S2FnSpillTarget<double> spill_sse;
    std::int32_t (*detached_nested)(std::int32_t);
    void* member;
    bool (*peer_boolean)(bool);
    S2FnMixedTarget peer_mixed;
};
extern "C" __attribute__((visibility("default"))) S2FnFixtureTargets s2fn_fixture_targets();
extern "C" __attribute__((visibility("default"))) void s2fn_fixture_set_original_calls(volatile std::uint64_t* calls);
extern "C" __attribute__((visibility("default"))) unsigned s2fn_fixture_peer_calls();
