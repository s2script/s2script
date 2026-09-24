#pragma once
#include <dlfcn.h>
#include <cstdio>
#include <cstdlib>
#include "engine_function_member_fixture.h"

// A standalone-test invariant, not a restriction on production hook targets.
// Compare actual mapped images, never function names or adjacent page offsets.
struct S2FnFixtureBoundary {
    const void* fixture;
    const void* provider;
    const void* consumer;

    bool Accepts(const void* target) const {
        Dl_info t{}, f{}, p{}, c{};
        return target && dladdr(target, &t) && dladdr(fixture, &f)
            && dladdr(provider, &p) && dladdr(consumer, &c)
            && t.dli_fbase == f.dli_fbase
            && t.dli_fbase != p.dli_fbase && t.dli_fbase != c.dli_fbase;
    }
    void Require(const char* name, const void* target, bool report = false) const {
        const bool accepted = Accepts(target);
        if (report || !accepted) {
            Dl_info t{}, f{}, p{}, c{};
            dladdr(target, &t); dladdr(fixture, &f);
            dladdr(provider, &p); dladdr(consumer, &c);
            std::fprintf(accepted ? stdout : stderr,
                "fixture-boundary=%s name=%s target=%p target-module=%s target-base=%p "
                "fixture-module=%s provider=%p provider-module=%s provider-base=%p "
                "consumer=%p consumer-module=%s consumer-base=%p\n",
                accepted ? "accepted" : "rejected", name, target,
                t.dli_fname ? t.dli_fname : "<unmapped>", t.dli_fbase,
                f.dli_fname ? f.dli_fname : "<unmapped>", provider,
                p.dli_fname ? p.dli_fname : "<unmapped>", p.dli_fbase, consumer,
                c.dli_fname ? c.dli_fname : "<unmapped>", c.dli_fbase);
            std::fflush(accepted ? stdout : stderr);
        }
        if (!accepted) std::abort(); // Always refuse before Configure, even with NDEBUG.
    }
};

// The complete inventory is shared by ABI, V8 and peer startup. Explicit names
// make a misplaced specialization diagnosable before any provider installation.
inline void S2FnRequireFixtureInventory(const S2FnFixtureTargets& t, const S2FnFixtureBoundary& boundary) {
#define S2FN_REQUIRE_TARGET(field) boundary.Require(#field, reinterpret_cast<const void*>(t.field), true)
    S2FN_REQUIRE_TARGET(identity_bool); S2FN_REQUIRE_TARGET(identity_u8);
    S2FN_REQUIRE_TARGET(identity_i32); S2FN_REQUIRE_TARGET(identity_u32);
    S2FN_REQUIRE_TARGET(identity_i64); S2FN_REQUIRE_TARGET(identity_u64);
    S2FN_REQUIRE_TARGET(identity_f32); S2FN_REQUIRE_TARGET(identity_f64);
    S2FN_REQUIRE_TARGET(identity_ptr); S2FN_REQUIRE_TARGET(void_target);
    S2FN_REQUIRE_TARGET(mixed); S2FN_REQUIRE_TARGET(novel);
    S2FN_REQUIRE_TARGET(noncanonical); S2FN_REQUIRE_TARGET(spill_gp);
    S2FN_REQUIRE_TARGET(spill_sse); S2FN_REQUIRE_TARGET(detached_nested);
    S2FN_REQUIRE_TARGET(member); S2FN_REQUIRE_TARGET(peer_boolean);
    S2FN_REQUIRE_TARGET(peer_mixed);
#undef S2FN_REQUIRE_TARGET
}
