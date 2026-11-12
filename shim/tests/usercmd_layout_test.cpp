// usercmd_layout_test.cpp — the usercmd post snapshot's direct reads and the decision to trust them.
// Engine-free: plain structs stand in for the generated protobuf objects.
#include "usercmd_layout.h"

#include <cstdio>
#include <cstdlib>
#include <string_view>

static int g_fail = 0;
#define CHECK(c) do { if (!(c)) { std::printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); ++g_fail; } } while (0)

// Stand-ins laid out like the build's generated classes (vptr + metadata + has-bits, then fields).
struct FakeButtons { char hdr[0x18]; uint64_t state1, state2, state3; };
struct FakeAngles { char hdr[0x18]; float x, y, z; };
struct FakeBase {
    char hdr[0x38];
    FakeButtons* buttons;   // 0x38
    FakeAngles* angles;     // 0x40
    char pad[0x50 - 0x48];
    int32_t number;         // 0x50
    int32_t clientTick;     // 0x54
    float fwd, left, up;    // 0x58 0x5c 0x60
    int32_t impulse;        // 0x64
};
struct FakeCmd { char hdr[0x30]; FakeBase* base; };

static S2UsercmdLayout Layout() {
    S2UsercmdLayout L;
    L.base = 0x30; L.buttonsPb = 0x38; L.viewAngles = 0x40; L.commandNumber = 0x50;
    L.forwardMove = 0x58; L.leftMove = 0x5c; L.upMove = 0x60; L.impulse = 0x64;
    L.buttonState1 = 0x18; L.angleX = 0x18;
    return L;
}

static void reads_every_field_at_its_offset() {
    FakeButtons b{}; b.state1 = 0x200000004ull;
    FakeAngles a{}; a.x = 1.5f; a.y = -90.0f; a.z = 0.25f;
    FakeBase base{}; base.buttons = &b; base.angles = &a; base.number = 1234;
    base.fwd = 450.f; base.left = 225.f; base.up = -1.f; base.impulse = 101;
    FakeCmd cmd{}; cmd.base = &base;
    S2UsercmdFields f;
    CHECK(S2ReadUsercmdAt(Layout(), &cmd, f));
    CHECK(f.commandNumber == 1234);
    CHECK(f.forwardMove == 450.f && f.leftMove == 225.f && f.upMove == -1.f);
    CHECK(f.impulse == 101 && f.buttons == 0x200000004ull);
    CHECK(f.angles[0] == 1.5f && f.angles[1] == -90.0f && f.angles[2] == 0.25f);
}

static void unset_sub_messages_read_as_defaults() {
    FakeBase base{}; base.number = 9; base.fwd = 3.f;   // no buttons, no angles
    FakeCmd cmd{}; cmd.base = &base;
    S2UsercmdFields f;
    CHECK(S2ReadUsercmdAt(Layout(), &cmd, f));
    CHECK(f.commandNumber == 9 && f.forwardMove == 3.f && f.buttons == 0 && f.angles[1] == 0.f);
    FakeCmd empty{};
    CHECK(S2ReadUsercmdAt(Layout(), &empty, f));
    CHECK(f.commandNumber == 0 && f.forwardMove == 0.f);
}

static void incomplete_layout_or_null_message_refuses() {
    S2UsercmdFields f;
    S2UsercmdLayout L = Layout(); L.impulse = -1;
    FakeCmd cmd{};
    CHECK(!S2ReadUsercmdAt(L, &cmd, f));
    CHECK(!S2ReadUsercmdAt(Layout(), nullptr, f));
}

static S2UsercmdFields Cmd(int32_t n, float fwd = 0.f) { S2UsercmdFields f; f.commandNumber = n; f.forwardMove = fwd; return f; }

static void trusted_only_after_an_unbroken_run_with_moving_numbers() {
    using State = S2UsercmdLayoutCheck::State;
    S2UsercmdLayoutCheck c;
    for (int i = 0; i < S2UsercmdLayoutCheck::kRequired - 1; ++i) CHECK(c.Observe(Cmd(100 + i), Cmd(100 + i)) == State::Checking);
    CHECK(c.Observe(Cmd(200), Cmd(200)) == State::Trusted);
    // Trusted is final: later inputs are not compared.
    CHECK(c.Observe(Cmd(1), Cmd(2)) == State::Trusted);
}

static void idle_zero_reads_never_earn_trust() {
    using State = S2UsercmdLayoutCheck::State;
    S2UsercmdLayoutCheck c;
    for (int i = 0; i < 4 * S2UsercmdLayoutCheck::kRequired; ++i) c.Observe(Cmd(0), Cmd(0));
    CHECK(c.state() == State::Checking);
    S2UsercmdLayoutCheck same;   // a stuck non-zero number is no better a witness
    for (int i = 0; i < 4 * S2UsercmdLayoutCheck::kRequired; ++i) same.Observe(Cmd(7), Cmd(7));
    CHECK(same.state() == State::Checking);
}

static void one_disagreement_fails_for_good_and_names_the_field() {
    using State = S2UsercmdLayoutCheck::State;
    S2UsercmdLayoutCheck c;
    c.Observe(Cmd(1), Cmd(1));
    CHECK(c.Observe(Cmd(2, 1.f), Cmd(2, 2.f)) == State::Failed);
    CHECK(c.failedField() && std::string_view(c.failedField()) == "forwardMove");
    for (int i = 0; i < 2 * S2UsercmdLayoutCheck::kRequired; ++i) c.Observe(Cmd(10 + i), Cmd(10 + i));
    CHECK(c.state() == State::Failed);
}

static void comparison_is_bit_exact() {
    S2UsercmdFields a = Cmd(1), b = Cmd(1);
    a.upMove = 0.0f; b.upMove = -0.0f;
    CHECK(S2UsercmdFieldsDiffer(a, b) != nullptr);
}

int main() {
    reads_every_field_at_its_offset();
    unset_sub_messages_read_as_defaults();
    incomplete_layout_or_null_message_refuses();
    trusted_only_after_an_unbroken_run_with_moving_numbers();
    idle_zero_reads_never_earn_trust();
    one_disagreement_fails_for_good_and_names_the_field();
    comparison_is_bit_exact();
    if (g_fail) { std::printf("usercmd_layout: %d FAILED\n", g_fail); return 1; }
    std::printf("usercmd_layout: all passed\n");
    return 0;
}
