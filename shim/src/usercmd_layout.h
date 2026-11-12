// usercmd_layout.h — read a command's fields at cached byte offsets, and decide whether to trust
// those offsets. Engine-free (no protobuf, no SDK), so the tests drive it with plain structs.
//
// The command is a generated protobuf message. Reflection reads it correctly on any build, but each
// field costs a descriptor walk. The usercmd post phase copies every field once per executed command,
// so it reads them at the generated layout's offsets instead (core gamedata, `offsets`: layout is
// data). Those offsets move when Valve regenerates the protobuf code, so they are never trusted on
// sight: the first commands are read both ways and compared, and only an unbroken run of agreement
// switches the direct path on. One disagreement switches it off for the process, with a named
// reason. Until then, and after a failure, the caller fills from reflection.
#pragma once

#include <cstdint>
#include <cstring>

struct S2UsercmdLayout {
    // Offsets into the outer command message.
    int base = -1;              // -> pointer to the base command sub-message
    // Offsets into the base command.
    int buttonsPb = -1;         // -> pointer to the button-state sub-message
    int viewAngles = -1;        // -> pointer to the view-angles sub-message
    int commandNumber = -1;     // int32
    int forwardMove = -1;       // float
    int leftMove = -1;          // float, +left (sideMove is its negation)
    int upMove = -1;            // float
    int impulse = -1;           // int32
    // Offset into the button-state sub-message.
    int buttonState1 = -1;      // uint64 held buttons
    // Offset of x in the view-angles sub-message; y and z follow as consecutive floats.
    int angleX = -1;

    bool Complete() const {
        return base >= 0 && buttonsPb >= 0 && viewAngles >= 0 && commandNumber >= 0 && forwardMove >= 0 &&
               leftMove >= 0 && upMove >= 0 && impulse >= 0 && buttonState1 >= 0 && angleX >= 0;
    }
};

// Raw field values, as stored (leftMove is NOT negated here, so both paths compare bit for bit).
struct S2UsercmdFields {
    float forwardMove = 0, leftMove = 0, upMove = 0;
    float angles[3] = {0, 0, 0};
    int32_t impulse = 0;
    int32_t commandNumber = 0;
    uint64_t buttons = 0;
};

namespace s2_usercmd_layout_detail {
template <typename T> inline T At(const void* base, int off) {
    T v; std::memcpy(&v, static_cast<const unsigned char*>(base) + off, sizeof v); return v;
}
inline const void* PtrAt(const void* base, int off) { return At<const void*>(base, off); }
}

// Read `message`'s fields at `layout`. An unset sub-message is a null pointer in the generated layout,
// which reads as the protobuf default (zero). False on an incomplete layout or a null message.
inline bool S2ReadUsercmdAt(const S2UsercmdLayout& layout, const void* message, S2UsercmdFields& out) {
    using namespace s2_usercmd_layout_detail;
    if (!message || !layout.Complete()) return false;
    out = S2UsercmdFields{};
    const void* base = PtrAt(message, layout.base);
    if (!base) return true;
    out.commandNumber = At<int32_t>(base, layout.commandNumber);
    out.forwardMove = At<float>(base, layout.forwardMove);
    out.leftMove = At<float>(base, layout.leftMove);
    out.upMove = At<float>(base, layout.upMove);
    out.impulse = At<int32_t>(base, layout.impulse);
    if (const void* btn = PtrAt(base, layout.buttonsPb)) out.buttons = At<uint64_t>(btn, layout.buttonState1);
    if (const void* va = PtrAt(base, layout.viewAngles)) {
        for (int i = 0; i < 3; ++i) out.angles[i] = At<float>(va, layout.angleX + 4 * i);
    }
    return true;
}

// The sub-message addresses reflection reports for the same command (an unset sub-message reports
// its default instance). Used while the layout is still being checked.
struct S2UsercmdPointers {
    const void* base = nullptr;
    const void* buttons = nullptr;
    const void* angles = nullptr;
};

// The checking-phase read: like S2ReadUsercmdAt, but a pointer read at an untrusted offset is only
// followed if it is null or exactly the address reflection reported, so a stale offset can never
// become a wild dereference. Returns the name of the first pointer that disagrees (nothing past it is
// read), or nullptr with `out` filled. Scalar reads stay inside objects reflection vouched for.
inline const char* S2ReadUsercmdChecked(const S2UsercmdLayout& layout, const void* message,
                                        const S2UsercmdPointers& expected, S2UsercmdFields& out) {
    using namespace s2_usercmd_layout_detail;
    if (!message || !layout.Complete()) return "layout offsets incomplete in gamedata";
    out = S2UsercmdFields{};
    const void* base = PtrAt(message, layout.base);
    if (base && base != expected.base) return "base pointer";
    if (!base) return nullptr;
    const void* btn = PtrAt(base, layout.buttonsPb);
    if (btn && btn != expected.buttons) return "buttons pointer";
    const void* va = PtrAt(base, layout.viewAngles);
    if (va && va != expected.angles) return "viewAngles pointer";
    out.commandNumber = At<int32_t>(base, layout.commandNumber);
    out.forwardMove = At<float>(base, layout.forwardMove);
    out.leftMove = At<float>(base, layout.leftMove);
    out.upMove = At<float>(base, layout.upMove);
    out.impulse = At<int32_t>(base, layout.impulse);
    if (btn) out.buttons = At<uint64_t>(btn, layout.buttonState1);
    if (va) for (int i = 0; i < 3; ++i) out.angles[i] = At<float>(va, layout.angleX + 4 * i);
    return nullptr;
}

// Bit-for-bit equality; names the first field that differs (nullptr when they agree).
inline const char* S2UsercmdFieldsDiffer(const S2UsercmdFields& a, const S2UsercmdFields& b) {
    auto same = [](const void* x, const void* y, size_t n) { return std::memcmp(x, y, n) == 0; };
    if (a.commandNumber != b.commandNumber) return "commandNumber";
    if (!same(&a.forwardMove, &b.forwardMove, 4)) return "forwardMove";
    if (!same(&a.leftMove, &b.leftMove, 4)) return "leftMove";
    if (!same(&a.upMove, &b.upMove, 4)) return "upMove";
    if (a.impulse != b.impulse) return "impulse";
    if (a.buttons != b.buttons) return "buttons";
    if (!same(a.angles, b.angles, sizeof a.angles)) return "viewAngles";
    return nullptr;
}

// The trust decision. Checking -> Trusted after `kRequired` agreeing commands that include at least
// two distinct non-zero command numbers (so a zero read at a wrong offset can't pass on idle input);
// any disagreement -> Failed, for good.
class S2UsercmdLayoutCheck {
public:
    enum class State { Checking, Trusted, Failed };
    static constexpr int kRequired = 64;

    State state() const { return state_; }
    const char* failedField() const { return failed_; }

    // Feed one command read both ways. Returns the new state.
    State Observe(const S2UsercmdFields& direct, const S2UsercmdFields& reflected) {
        if (state_ != State::Checking) return state_;
        if (const char* f = S2UsercmdFieldsDiffer(direct, reflected)) {
            failed_ = f;
            state_ = State::Failed;
            return state_;
        }
        const int32_t n = reflected.commandNumber;
        if (n != 0) {
            if (firstNumber_ == 0) firstNumber_ = n;
            else if (n != firstNumber_) sawTwoNumbers_ = true;
        }
        if (++agreed_ >= kRequired && sawTwoNumbers_) state_ = State::Trusted;
        return state_;
    }

    void Fail(const char* why) { failed_ = why; state_ = State::Failed; }

private:
    State state_ = State::Checking;
    const char* failed_ = nullptr;
    int agreed_ = 0;
    int32_t firstNumber_ = 0;
    bool sawTwoNumbers_ = false;
};
