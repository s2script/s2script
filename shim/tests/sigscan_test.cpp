// Host-g++ unit test for the pure pattern scanner (no SDK deps). Run via scripts/test-sigscan.sh.
#include "../src/sigscan.h"
#include <cstdio>
#include <cstring>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg) do { if (!(cond)) { std::printf("FAIL: %s\n", msg); g_fail = 1; } } while (0)

int main() {
    using namespace s2sig;

    // --- ParsePattern ---
    auto p = ParsePattern("4C 8D 35 ?");
    CHECK(p.size() == 4, "ParsePattern size");
    CHECK(p[0] == 0x4C && p[1] == 0x8D && p[2] == 0x35 && p[3] == -1, "ParsePattern tokens");
    CHECK(ParsePattern("").empty(), "ParsePattern empty");
    CHECK(ParsePattern("ZZ").empty(), "ParsePattern malformed");
    CHECK(ParsePattern("4C 8D 35 ??")[3] == -1, "ParsePattern double-? wildcard");

    // --- FindPattern (literal + wildcard + miss) ---
    std::vector<uint8_t> buf(256, 0x90);           // nop-filled
    buf[0x10] = 0x4C; buf[0x11] = 0x8D; buf[0x12] = 0x35; buf[0x13] = 0xAB;
    CHECK(FindPattern(buf.data(), buf.size(), ParsePattern("4C 8D 35")) == 0x10, "FindPattern literal");
    CHECK(FindPattern(buf.data(), buf.size(), ParsePattern("4C 8D 35 ?")) == 0x10, "FindPattern wildcard");
    CHECK(FindPattern(buf.data(), buf.size(), ParsePattern("DE AD BE EF")) == -1, "FindPattern miss");

    // --- ResolveLeaDisp: lea r14,[rip+disp] at 0x80, disp=0x9C -> target = 0x80 + 7 + 0x9C = 0x123 ---
    std::vector<uint8_t> b2(512, 0x90);
    b2[0x80] = 0x4C; b2[0x81] = 0x8D; b2[0x82] = 0x35;
    b2[0x83] = 0x9C; b2[0x84] = 0x00; b2[0x85] = 0x00; b2[0x86] = 0x00;   // disp32 = 0x9C (LE)
    CHECK(ResolveLeaDisp(b2.data(), b2.size(), 0x80, 3, 7) == 0x123, "ResolveLeaDisp");
    CHECK(ResolveLeaDisp(b2.data(), 0x82, 0x80, 3, 7) == kFail, "ResolveLeaDisp oob");

    // --- ResolveCtorXref: ctor at 0x40; lea r14 at 0x80 (target 0x123); E8 call at 0x87 -> 0x40 ---
    std::vector<uint8_t> b3(512, 0x90);
    b3[0x80] = 0x4C; b3[0x81] = 0x8D; b3[0x82] = 0x35;
    b3[0x83] = 0x9C; b3[0x84] = 0x00; b3[0x85] = 0x00; b3[0x86] = 0x00;   // lea disp -> 0x123
    // call rel32 at 0x87: target 0x40 = 0x87 + 5 + rel32 => rel32 = 0x40 - 0x8C = -0x4C = 0xFFFFFFB4
    b3[0x87] = 0xE8; b3[0x88] = 0xB4; b3[0x89] = 0xFF; b3[0x8A] = 0xFF; b3[0x8B] = 0xFF;
    CHECK(ResolveCtorXref(b3.data(), b3.size(), 0x40) == 0x123, "ResolveCtorXref");
    CHECK(ResolveCtorXref(b3.data(), b3.size(), 0x41) == kFail, "ResolveCtorXref no-caller");

    if (g_fail) { std::printf("sigscan_test: FAILURES\n"); return 1; }
    std::printf("sigscan_test: all passed\n");
    return 0;
}
