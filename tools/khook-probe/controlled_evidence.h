#pragma once

#include <cstdint>
#include <string>

namespace s2khook {

using IntTarget = int (*)(int);

// The volatile function-pointer read is intentional. These functions are patched by KHook at
// runtime, which same-TU optimization cannot see; a direct call lets IPA discard hook side effects.
inline int InvokeOpaque(IntTarget volatile& target, int argument) {
    return target(argument);
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
};

struct PeerActionsObservation {
    PeerActionObservation ab_io;
    PeerActionObservation ab_oo;
    PeerActionObservation ab_os;
    PeerActionObservation ba_io;
    PeerActionObservation ba_oo;
    PeerActionObservation ba_os;

    bool Passed() const {
        return ab_io.pre_a == 1 && ab_io.pre_b == 1 && ab_io.original == 1 && ab_io.ret == 42 &&
               ab_oo.pre_a == 1 && ab_oo.pre_b == 1 && ab_oo.original == 1 && ab_oo.ret == 7 &&
               ab_os.pre_a == 1 && ab_os.pre_b == 1 && ab_os.original == 0 && ab_os.ret == 99 &&
               ba_io.pre_a == 1 && ba_io.pre_b == 1 && ba_io.original == 1 && ba_io.ret == 42 &&
               ba_oo.pre_a == 1 && ba_oo.pre_b == 1 && ba_oo.original == 1 && ba_oo.ret == 99 &&
               ba_os.pre_a == 1 && ba_os.pre_b == 1 && ba_os.original == 0 && ba_os.ret == 99;
    }

    std::string Json() const {
        const auto pair = [](const PeerActionObservation& value) {
            return std::string("{\"pre_a\":") + std::to_string(value.pre_a) +
                   ",\"pre_b\":" + std::to_string(value.pre_b) +
                   ",\"orig\":" + std::to_string(value.original) +
                   ",\"ret\":" + std::to_string(value.ret) + "}";
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
            {1, 1, 1, 42}, {1, 1, 1, 7}, {1, 1, 0, 99},
            {1, 1, 1, 42}, {1, 1, 1, 99}, {1, 1, 0, 99},
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
