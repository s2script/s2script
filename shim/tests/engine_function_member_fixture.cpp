#include "engine_function_member_fixture.h"
#include <khook.hpp>

std::int32_t S2FnMemberFixture::Call(std::int32_t value) {
    ++*original_calls;
    return base + value;
}
extern "C" void* s2fn_member_fixture_target() {
    return KHook::ExtractMFP(&S2FnMemberFixture::Call);
}
