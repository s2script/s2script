#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <array>

namespace s2khook {

struct DeclarativeVoidObservation {
    bool installed = false;
    int pre = 0;
    int original = 0;
    int original_in_scope = 0;
    int receiver_ok = 0;
    bool expired = false;
    bool Passed() const {
        return installed && pre == 1 && original == 1 && original_in_scope == 1 &&
               receiver_ok == 1 && expired;
    }
    std::string Json() const {
        return std::string("{\"installed\":") + (installed ? "true" : "false") +
            ",\"pre\":" + std::to_string(pre) + ",\"original\":" + std::to_string(original) +
            ",\"original_in_scope\":" + std::to_string(original_in_scope) +
            ",\"receiver_ok\":" + std::to_string(receiver_ok) +
            ",\"expired\":" + (expired ? "true" : "false") + "}";
    }
};

struct DeclarativeMutationObservation {
    int pre = 0, original = 0, original_in_scope = 0;
    float value = 0;
    int32_t integer = 0;
    int64_t opaque_a = 0, opaque_b = 0;
    bool Passed() const {
        return pre == 1 && original == 1 && original_in_scope == 1 && value == 7.25f &&
            integer == -17 && static_cast<uint64_t>(opaque_a) == UINT64_C(0xf123456789abcdef) &&
            static_cast<uint64_t>(opaque_b) == UINT64_C(0x8123456789abcdef);
    }
    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) +
            ",\"original\":" + std::to_string(original) +
            ",\"original_in_scope\":" + std::to_string(original_in_scope) +
            ",\"value\":" + std::to_string(value) + ",\"integer\":" + std::to_string(integer) +
            ",\"opaque_a\":\"" + std::to_string(static_cast<uint64_t>(opaque_a)) +
            "\",\"opaque_b\":\"" + std::to_string(static_cast<uint64_t>(opaque_b)) + "\"}";
    }
};
struct DeclarativeAcquireObservation {
    int pre = 0, post = 0, original = 0, arguments_ok = 0;
    int32_t effective_return = -1, post_result = -1;
    int skipped = -1;
    bool Matches(int32_t expected, bool skip) const {
        return pre == 1 && post == 1 && original == (skip ? 0 : 1) &&
            arguments_ok == (skip ? 0 : 1) && effective_return == expected &&
            post_result == expected && skipped == (skip ? 1 : 0);
    }
    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) + ",\"post\":" + std::to_string(post) +
            ",\"original\":" + std::to_string(original) + ",\"arguments_ok\":" + std::to_string(arguments_ok) +
            ",\"effective_return\":" + std::to_string(effective_return) +
            ",\"post_result\":" + std::to_string(post_result) + ",\"skipped\":" + std::to_string(skipped) + "}";
    }
};
struct DeclarativeNestingObservation {
    int same_pre = 0, same_post = 0, same_original = 0, same_restored = 0;
    int other_pre = 0, other_original = 0, other_restored = 0, stale_rejected = 0;
    int skipped = 0;
    int32_t effective_return = -1;
    std::array<int32_t, 3> post_methods{{-1, -1, -1}};
    bool Passed() const {
        return same_pre == 3 && same_post == 3 && same_original == 3 && same_restored == 2 &&
            other_pre == 2 && other_original == 2 && other_restored == 2 && stale_rejected == 2 &&
            skipped == 0 && effective_return == 40 && post_methods == std::array<int32_t,3>{{42,41,40}};
    }
    std::string Json() const {
        return std::string("{\"same_pre\":") + std::to_string(same_pre) +
            ",\"same_post\":" + std::to_string(same_post) + ",\"same_original\":" + std::to_string(same_original) +
            ",\"same_restored\":" + std::to_string(same_restored) + ",\"other_pre\":" + std::to_string(other_pre) +
            ",\"other_original\":" + std::to_string(other_original) + ",\"other_restored\":" + std::to_string(other_restored) +
            ",\"stale_rejected\":" + std::to_string(stale_rejected) + ",\"skipped\":" + std::to_string(skipped) +
            ",\"effective_return\":" + std::to_string(effective_return) + ",\"post_methods\":[" +
            std::to_string(post_methods[0]) + "," + std::to_string(post_methods[1]) + "," + std::to_string(post_methods[2]) + "]}";
    }
};
struct DeclarativeBypassObservation {
    int pre = 0, post = 0, original = 0;
    int pre_after_bypass = -1, post_after_bypass = -1;
    int removal_refused = 0, reset_preserved_view = 0;
    std::array<int32_t,3> returns{{-1,-1,-1}};
    bool Passed() const {
        return pre == 2 && post == 2 && original == 3 && pre_after_bypass == 0 && post_after_bypass == 0 &&
            removal_refused == 2 && reset_preserved_view == 2 && returns == std::array<int32_t,3>{{6,6,6}};
    }
    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) + ",\"post\":" + std::to_string(post) +
            ",\"original\":" + std::to_string(original) + ",\"pre_after_bypass\":" + std::to_string(pre_after_bypass) +
            ",\"post_after_bypass\":" + std::to_string(post_after_bypass) +
            ",\"removal_refused\":" + std::to_string(removal_refused) +
            ",\"reset_preserved_view\":" + std::to_string(reset_preserved_view) + ",\"returns\":[" +
            std::to_string(returns[0]) + "," + std::to_string(returns[1]) + "," + std::to_string(returns[2]) + "]}";
    }
};
struct DeclarativeSnapshot {
    DeclarativeVoidObservation simple;
    DeclarativeMutationObservation mutation;
    std::array<DeclarativeAcquireObservation, 5> acquire;
    DeclarativeNestingObservation nesting;
    DeclarativeBypassObservation bypass;
    bool Passed() const {
        return simple.Passed() && mutation.Passed() && nesting.Passed() && bypass.Passed() &&
            acquire[0].Matches(6,false) && acquire[1].Matches(6,false) && acquire[2].Matches(2,false) &&
            acquire[3].Matches(1,true) && acquire[4].Matches(0,true);
    }
    std::string Json() const {
        std::string rows;
        for (const auto& row : acquire) { if (!rows.empty()) rows += ','; rows += row.Json(); }
        return "{\"simple\":" + simple.Json() + ",\"mutation\":" + mutation.Json() +
            ",\"acquire\":[" + rows + "],\"nesting\":" + nesting.Json() + ",\"bypass\":" + bypass.Json() + "}";
    }
};

using IntTarget = int (*)(int);

// The volatile function-pointer read is intentional. These functions are patched by KHook at
// runtime, which same-TU optimization cannot see; a direct call lets IPA discard hook side effects.
inline int InvokeOpaque(IntTarget volatile& target, int argument) {
    return target(argument);
}

template <typename Allocate, typename Release>
bool ReplaceOwnedCString(void* pointer_storage, const std::string& value,
                         Allocate allocate, Release release) {
    if (!pointer_storage) return false;
    char* replacement = allocate(value.size() + 1);
    if (!replacement) return false;
    std::memcpy(replacement, value.c_str(), value.size() + 1);
    char* previous = nullptr;
    std::memcpy(&previous, pointer_storage, sizeof(previous));
    std::memcpy(pointer_storage, &replacement, sizeof(replacement));
    if (previous) release(previous);
    return true;
}

class LevelLifetime {
public:
    void OnLevelInit() {
        if (!active_) {
            ++generation_;
        }
        active_ = true;
    }
    void OnLevelShutdown() { active_ = false; }
    bool MayTouchWorld() const { return active_; }
    bool MayTouchOwnedWorld(std::uint64_t owned_generation) const {
        return active_ && owned_generation != 0 && owned_generation == generation_;
    }
    std::uint64_t ClaimCurrentWorld() const { return active_ ? generation_ : 0; }
    std::uint64_t Generation() const { return generation_; }

private:
    bool active_ = false;
    std::uint64_t generation_ = 0;
};

struct PeerActionObservation {
    int pre_a;
    int pre_b;
    int original;
    int ret;
    int pre_order;
};

struct PeerActionsObservation {
    PeerActionObservation ab_io;
    PeerActionObservation ab_oo;
    PeerActionObservation ab_os;
    PeerActionObservation ba_io;
    PeerActionObservation ba_oo;
    PeerActionObservation ba_os;

    bool Passed() const {
        return ab_io.pre_a == 1 && ab_io.pre_b == 1 && ab_io.original == 1 &&
               ab_io.ret == 42 && ab_io.pre_order == 21 &&
               ab_oo.pre_a == 1 && ab_oo.pre_b == 1 && ab_oo.original == 1 &&
               ab_oo.ret == 99 && ab_oo.pre_order == 21 &&
               ab_os.pre_a == 1 && ab_os.pre_b == 1 && ab_os.original == 0 &&
               ab_os.ret == 99 && ab_os.pre_order == 21 &&
               ba_io.pre_a == 1 && ba_io.pre_b == 1 && ba_io.original == 1 &&
               ba_io.ret == 42 && ba_io.pre_order == 12 &&
               ba_oo.pre_a == 1 && ba_oo.pre_b == 1 && ba_oo.original == 1 &&
               ba_oo.ret == 7 && ba_oo.pre_order == 12 &&
               ba_os.pre_a == 1 && ba_os.pre_b == 1 && ba_os.original == 0 &&
               ba_os.ret == 99 && ba_os.pre_order == 12;
    }

    std::string Json() const {
        const auto pair = [](const PeerActionObservation& value) {
            return std::string("{\"pre_a\":") + std::to_string(value.pre_a) +
                   ",\"pre_b\":" + std::to_string(value.pre_b) +
                   ",\"orig\":" + std::to_string(value.original) +
                   ",\"ret\":" + std::to_string(value.ret) +
                   ",\"pre_order\":" + std::to_string(value.pre_order) + "}";
        };
        return std::string("{\"ab_io\":") + pair(ab_io) +
               ",\"ab_oo\":" + pair(ab_oo) +
               ",\"ab_os\":" + pair(ab_os) +
               ",\"ba_io\":" + pair(ba_io) +
               ",\"ba_oo\":" + pair(ba_oo) +
               ",\"ba_os\":" + pair(ba_os) + "}";
    }

    static std::string ExpectedJson() {
        return PeerActionsObservation{
            {1, 1, 1, 42, 21}, {1, 1, 1, 99, 21}, {1, 1, 0, 99, 21},
            {1, 1, 1, 42, 12}, {1, 1, 1, 7, 12}, {1, 1, 0, 99, 12},
        }.Json();
    }
};

struct OnceObservation {
    int pre;
    int post;
    int original;
    int ret;

    bool Passed() const { return pre == 1 && post == 1 && original == 1 && ret == 15; }

    std::string Json() const {
        return std::string("{\"pre\":") + std::to_string(pre) +
               ",\"post\":" + std::to_string(post) +
               ",\"orig\":" + std::to_string(original) +
               ",\"return\":" + std::to_string(ret) + "}";
    }

    static std::string ExpectedJson() { return OnceObservation{1, 1, 1, 15}.Json(); }
};

}  // namespace s2khook
