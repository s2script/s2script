// Unit test for the engine-free half of the inline detour: prologue decoding, relocation, and the
// named refusals. Self-contained — no engine, no CMake SDK paths, no patching of live code.
//
// The point of this file is that a relocator whose only proof is "the server did not crash" is not
// proven at all. Everything here is bytes in, bytes out, at pretend addresses of our choosing, so a
// wrong displacement is a failing assertion rather than a wrong global read on a game server.
#include "../src/detour_reloc.h"
#include "../src/detour.h"
#include "../src/platform/memory.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

using namespace s2detour;

static int g_fail = 0;
#define CHECK(cond, msg)                                                   \
    do {                                                                   \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }  \
        else         { std::cout << "ok:   " << (msg) << "\n"; }            \
    } while (0)

static int32_t Read32(const uint8_t* p) { int32_t v; std::memcpy(&v, p, 4); return v; }

// Pretend addresses. Far enough apart to be a real relocation, close enough to stay in rel32 range.
static const uintptr_t kSrcAddr = 0x0000700000001000ULL;
static const uintptr_t kDstAddr = 0x0000700010002000ULL;

// ---------------------------------------------------------------------------------------------
// 1. The prologue corpus: every function s2script detours, as the byte-signatures declare them.
//    This is the concrete statement of "the near tier cannot regress an existing detour" — for each
//    one, tier 1's steal must be a strict PREFIX of tier 2's.
// ---------------------------------------------------------------------------------------------
struct Prologue { const char* name; std::vector<uint8_t> bytes; int wantNear; int wantFar; };

static std::vector<Prologue> Corpus() {
    return {
        // push rbp; mov rbp,rsp; push r15; push r14; push r13; mov r13,rdi; push r12
        {"DispatchTraceAttack",
         {0x55,0x48,0x89,0xE5,0x41,0x57,0x41,0x56,0x41,0x55,0x49,0x89,0xFD,0x41,0x54,0x49,0x89,0xF4}, 6, 15},
        // push rbp; mov rbp,rsp; push r15; mov r15,rsi; push r14; push r13; mov r13d,edx
        {"HostSay",
         {0x55,0x48,0x89,0xE5,0x41,0x57,0x49,0x89,0xF7,0x41,0x56,0x41,0x55,0x41,0x89,0xD5,0x41,0x54}, 6, 16},
        // push rbp; mov rbp,rsp; push r15; mov r15,rdi; push r14; push r13; push r12
        {"FireOutputInternal",
         {0x55,0x48,0x89,0xE5,0x41,0x57,0x49,0x89,0xFF,0x41,0x56,0x41,0x55,0x41,0x54,0x49,0x89,0xD4}, 6, 15},
        // push rbp; mov rbp,rsp; push r15; push r14; push r13; push r12; push rbx; mov rbx,rdi
        {"ProcessUsercmds",
         {0x55,0x48,0x89,0xE5,0x41,0x57,0x41,0x56,0x41,0x55,0x41,0x54,0x53,0x48,0x89,0xFB,0x48,0x83}, 6, 16},
    };
}

static void test_prologue_corpus_near_is_a_prefix_of_far() {
    for (const auto& p : Corpus()) {
        uint8_t out[kMaxSteal] = {};
        BuildResult n = BuildTrampolineBody(p.bytes.data(), kSrcAddr, kJmpRel32, out, kDstAddr, nullptr);
        CHECK(n.ok && n.steal == p.wantNear,
              std::string(p.name) + ": near tier steals " + std::to_string(p.wantNear) + " bytes");

        uint8_t outFar[kMaxSteal] = {};
        BuildResult f = BuildTrampolineBody(p.bytes.data(), kSrcAddr, kJmpAbs, outFar, kDstAddr, nullptr);
        CHECK(f.ok && f.steal == p.wantFar,
              std::string(p.name) + ": far tier steals " + std::to_string(p.wantFar) + " bytes");

        // The regression statement: shrinking the window can only take FEWER of the same bytes.
        CHECK(n.ok && f.ok && n.steal <= f.steal &&
              std::memcmp(out, outFar, static_cast<size_t>(n.emitted)) == 0,
              std::string(p.name) + ": the near steal is a strict prefix of the far steal");

        // Nothing in these prologues is relative, so relocation must be byte-identical to a copy.
        CHECK(n.ok && std::memcmp(out, p.bytes.data(), static_cast<size_t>(n.emitted)) == 0,
              std::string(p.name) + ": a position-independent prologue copies unchanged");
    }
}

// ---------------------------------------------------------------------------------------------
// 2. The two blocked targets: refused at 14 bytes today, accepted at 5.
// ---------------------------------------------------------------------------------------------
static void test_the_two_blocked_hook_targets() {
    // CCSGameRules::TerminateRound — push rbp; mov rbp,rsp; push r15; mov r15d,esi; push r14;
    //                                lea rsi,[rip+disp32]
    const uint8_t terminate[] = {0x55,0x48,0x89,0xE5,0x41,0x57,0x41,0x89,0xF7,0x41,0x56,
                                 0x48,0x8D,0x35,0x11,0x22,0x33,0x44, 0x41,0x55,0x41,0x54};
    // CCSPlayerController::Respawn — push rbp; mov rbp,rsp; push r12; push rbx; mov rbx,rdi;
    //                                call rel32
    const uint8_t respawn[]   = {0x55,0x48,0x89,0xE5,0x41,0x54,0x53,0x48,0x89,0xFB,
                                 0xE8,0x44,0x33,0x22,0x11, 0x48,0x85,0xC0,0x74,0x10};

    uint8_t out[kMaxSteal] = {};
    BuildResult t5 = BuildTrampolineBody(terminate, kSrcAddr, kJmpRel32, out, kDstAddr, nullptr);
    CHECK(t5.ok && t5.steal == 6, "TerminateRound: the near tier stops before the rip-relative lea");
    BuildResult r5 = BuildTrampolineBody(respawn, kSrcAddr, kJmpRel32, out, kDstAddr, nullptr);
    CHECK(r5.ok && r5.steal == 6, "Respawn: the near tier stops before the call rel32");

    // At 14 bytes both are still installable — but only because we now RELOCATE rather than refuse.
    // Before this slice these two returned false, which is the bug that degraded them on a server.
    BuildResult t14 = BuildTrampolineBody(terminate, kSrcAddr, kJmpAbs, out, kDstAddr, nullptr);
    CHECK(t14.ok && t14.steal == 18, "TerminateRound: the far tier now relocates the lea instead of refusing");
    BuildResult r14 = BuildTrampolineBody(respawn, kSrcAddr, kJmpAbs, out, kDstAddr, nullptr);
    CHECK(r14.ok && r14.steal == 15, "Respawn: the far tier now relocates the call instead of refusing");
}

// ---------------------------------------------------------------------------------------------
// 3. Byte-exact relocation. The arithmetic that, done wrong, writes four bytes into the middle of an
//    instruction that still decodes and still runs.
// ---------------------------------------------------------------------------------------------
static void test_rip_relative_disp32_is_recomputed() {
    // lea rsi,[rip+0x44332211]  (7 bytes, disp32 at offset 3)
    const uint8_t code[] = {0x48,0x8D,0x35,0x11,0x22,0x33,0x44, 0x90,0x90,0x90,0x90,0x90,0x90,0x90};
    uint8_t out[kMaxSteal] = {};
    BuildResult r = BuildTrampolineBody(code, kSrcAddr, 7, out, kDstAddr, nullptr);
    CHECK(r.ok && r.steal == 7 && r.emitted == 7, "rip-relative lea relocates without changing size");

    const int64_t absTarget = static_cast<int64_t>(kSrcAddr) + 7 + 0x44332211;
    const int64_t wantDisp  = absTarget - (static_cast<int64_t>(kDstAddr) + 7);
    CHECK(Read32(out + 3) == static_cast<int32_t>(wantDisp),
          "the new disp32 still points at the SAME absolute address");
    CHECK(out[0] == 0x48 && out[1] == 0x8D && out[2] == 0x35,
          "only the displacement changed — the opcode and ModRM are untouched");
}

static void test_disp32_offset_accounts_for_a_trailing_immediate() {
    // cmpl $0x7F,[rip+0x44332211]  ->  83 3D <disp32> <imm8>   (7 bytes: disp32 at 2, imm8 at 6)
    // THE case that breaks a relocator which assumes the disp32 is the last four bytes. Getting it
    // wrong here rewrites the opcode's immediate and leaves the displacement stale — and the result
    // still decodes, still runs, and compares against the wrong address.
    const uint8_t code[] = {0x83,0x3D,0x11,0x22,0x33,0x44,0x7F, 0x90,0x90,0x90,0x90,0x90,0x90,0x90};
    uint8_t out[kMaxSteal] = {};
    BuildResult r = BuildTrampolineBody(code, kSrcAddr, 7, out, kDstAddr, nullptr);
    CHECK(r.ok && r.steal == 7, "an instruction with BOTH a disp32 and an imm8 decodes");

    const int64_t absTarget = static_cast<int64_t>(kSrcAddr) + 7 + 0x44332211;
    const int64_t wantDisp  = absTarget - (static_cast<int64_t>(kDstAddr) + 7);
    CHECK(Read32(out + 2) == static_cast<int32_t>(wantDisp),
          "the disp32 is rewritten at offset 2, NOT at len-4");
    CHECK(out[6] == 0x7F, "the trailing immediate is left alone");
}

static void test_call_and_jmp_rel32_are_recomputed() {
    struct Case { const char* name; std::vector<uint8_t> bytes; int len; int relOff; };
    const std::vector<Case> cases = {
        {"call rel32", {0xE8,0x11,0x22,0x33,0x44}, 5, 1},
        {"jmp rel32",  {0xE9,0x11,0x22,0x33,0x44}, 5, 1},
        {"jz rel32",   {0x0F,0x84,0x11,0x22,0x33,0x44}, 6, 2},
    };
    for (const auto& c : cases) {
        std::vector<uint8_t> code = c.bytes;
        code.resize(20, 0x90);
        uint8_t out[kMaxSteal] = {};
        BuildResult r = BuildTrampolineBody(code.data(), kSrcAddr, c.len, out, kDstAddr, nullptr);
        CHECK(r.ok && r.steal == c.len, std::string(c.name) + ": decodes at its true length");

        const int64_t absTarget = static_cast<int64_t>(kSrcAddr) + c.len + 0x44332211;
        const int64_t wantRel   = absTarget - (static_cast<int64_t>(kDstAddr) + c.len);
        CHECK(Read32(out + c.relOff) == static_cast<int32_t>(wantRel),
              std::string(c.name) + ": still branches to the SAME absolute address");
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Semantic round-trip: a correct-looking encoding that computes the wrong address still fails
//    here. Relocate real code into a real page, run it, and compare against the original.
// ---------------------------------------------------------------------------------------------
static int64_t g_global = 0;
// Static, not on the stack: this must live within rel32 of g_global for the encoding below to exist
// at all, and .bss neighbours are adjacent where the stack is gigabytes away under ASLR.
static uint8_t g_src[16] = {0x48,0x8B,0x05, 0,0,0,0, 0xC3};

static void test_relocated_code_actually_behaves_the_same() {
    // mov rax,[rip+disp32] ; ret   — reads g_global through a rip-relative operand.
    const uintptr_t srcAddr = reinterpret_cast<uintptr_t>(g_src);
    const int64_t disp = static_cast<int64_t>(reinterpret_cast<uintptr_t>(&g_global)) -
                         static_cast<int64_t>(srcAddr + 7);
    CHECK(disp >= INT32_MIN && disp <= INT32_MAX, "the source encoding is representable");
    if (disp < INT32_MIN || disp > INT32_MAX) return;
    const int32_t d32 = static_cast<int32_t>(disp);
    std::memcpy(g_src + 3, &d32, 4);

    const size_t pageSize = s2platform::PageSize();
    void* page = s2platform::AllocateExecutableNear(
        reinterpret_cast<uintptr_t>(&g_global), pageSize);
    CHECK(page != nullptr, "a test page was mapped within rel32 of the global");
    if (!page) return;
    uint8_t* const src = g_src;

    uint8_t out[kMaxSteal] = {};
    BuildResult r = BuildTrampolineBody(src, srcAddr, 8, out, reinterpret_cast<uintptr_t>(page), nullptr);
    CHECK(r.ok && r.steal == 8, "the rip-relative load + ret relocates");
    std::memcpy(page, out, static_cast<size_t>(r.emitted));

    g_global = 0x0BADC0DE;
    using Fn = int64_t (*)();
    const int64_t got = reinterpret_cast<Fn>(page)();
    CHECK(got == 0x0BADC0DE,
          "the RELOCATED copy reads the same global as the original — a wrong disp32 fails here");

    g_global = 0x1234;
    CHECK(reinterpret_cast<Fn>(page)() == 0x1234, "and it tracks the global, rather than a stale copy");
    s2platform::FreeExecutable(page, pageSize);
}

// ---------------------------------------------------------------------------------------------
// 5. Every refusal fires, asserted on its REASON — not merely on ok == false, which would pass even
//    if the wrong guard tripped.
// ---------------------------------------------------------------------------------------------
static bool ReasonIs(const BuildResult& r, const char* needle) {
    return !r.ok && r.reason && std::strstr(r.reason, needle) != nullptr;
}

static void test_short_branches_are_refused_by_name() {
    const uint8_t jmpRel8[] = {0xEB,0x10, 0x90,0x90,0x90,0x90,0x90,0x90,0x90,0x90};
    const uint8_t jzRel8[]  = {0x74,0x10, 0x90,0x90,0x90,0x90,0x90,0x90,0x90,0x90};
    const uint8_t loopne[]  = {0xE0,0x10, 0x90,0x90,0x90,0x90,0x90,0x90,0x90,0x90};
    uint8_t out[kMaxSteal] = {};
    CHECK(ReasonIs(BuildTrampolineBody(jmpRel8, kSrcAddr, 5, out, kDstAddr, nullptr), "short branch"),
          "jmp rel8 is refused as a short branch, not silently copied");
    CHECK(ReasonIs(BuildTrampolineBody(jzRel8, kSrcAddr, 5, out, kDstAddr, nullptr), "short branch"),
          "jcc rel8 is refused as a short branch");
    CHECK(ReasonIs(BuildTrampolineBody(loopne, kSrcAddr, 5, out, kDstAddr, nullptr), "short branch"),
          "loopne is refused as a short branch");
}

static void test_out_of_reach_displacement_is_refused() {
    // Same rip-relative lea, but relocated 3GB away: the recomputed disp32 cannot represent it.
    const uint8_t code[] = {0x48,0x8D,0x35,0x11,0x22,0x33,0x44, 0x90,0x90,0x90,0x90,0x90,0x90,0x90};
    uint8_t out[kMaxSteal] = {};
    // 6GB away. Note 3GB would NOT be far enough: the encoded displacement is already +0x44332211,
    // so the recomputed value would be about -2.07GB, which still fits in an int32. Getting this
    // wrong makes the test pass for the wrong reason (nothing was out of range at all).
    const uintptr_t farAway = kSrcAddr + 0x180000000ULL;
    CHECK(ReasonIs(BuildTrampolineBody(code, kSrcAddr, 7, out, farAway, nullptr), "out of reach"),
          "a rip-relative target out of rel32 reach of the trampoline is refused, not truncated");

    const uint8_t call[] = {0xE8,0x11,0x22,0x33,0x44, 0x90,0x90,0x90,0x90,0x90};
    CHECK(ReasonIs(BuildTrampolineBody(call, kSrcAddr, 5, out, farAway, nullptr), "out of reach"),
          "a branch target out of rel32 reach of the trampoline is refused");
}

static void test_undecodable_is_refused() {
    const uint8_t junk[] = {0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF};
    uint8_t out[kMaxSteal] = {};
    BuildResult r = BuildTrampolineBody(junk, kSrcAddr, 5, out, kDstAddr, nullptr);
    CHECK(!r.ok, "an undecodable prologue is refused");
}

// ---------------------------------------------------------------------------------------------
// 6. The executability probe is consulted for EVERY instruction, before it is read.
// ---------------------------------------------------------------------------------------------
static int  g_probeCalls   = 0;
static int  g_probeFailAt  = -1;   // fail the Nth distinct probe
static int  g_probeAllowed = 0;

static int CountingProbe(const void*) {
    g_probeCalls++;
    if (g_probeFailAt >= 0 && g_probeCalls > g_probeAllowed) return 0;
    return 1;
}

static void test_probe_gates_every_instruction() {
    const uint8_t code[] = {0x55,0x48,0x89,0xE5,0x41,0x57,0x41,0x56,0x41,0x55,
                            0x41,0x54,0x53,0x48,0x89,0xFB,0x90,0x90};
    uint8_t out[kMaxSteal] = {};

    g_probeCalls = 0; g_probeFailAt = -1; g_probeAllowed = 0;
    BuildResult ok = BuildTrampolineBody(code, kSrcAddr, kJmpAbs, out, kDstAddr, CountingProbe);
    CHECK(ok.ok, "the corpus prologue builds with a permissive probe");
    // 7 instructions cover 16 bytes; each is probed at both ends before being decoded.
    CHECK(g_probeCalls >= 2, "the probe is consulted more than once — not just at the entry address");
    const int probesForWholeWalk = g_probeCalls;

    // Now refuse partway through. The walk must stop THERE, having decoded nothing further.
    g_probeCalls = 0; g_probeFailAt = 1; g_probeAllowed = 2;   // let instruction 1 through, deny 2
    BuildResult denied = BuildTrampolineBody(code, kSrcAddr, kJmpAbs, out, kDstAddr, CountingProbe);
    CHECK(ReasonIs(denied, "executable range"),
          "a probe refusal mid-prologue stops the walk with a named reason");
    CHECK(g_probeCalls < probesForWholeWalk,
          "and it stops EARLY — it does not keep decoding past the refusal");
}

// ---------------------------------------------------------------------------------------------
// 7. The jump encoders.
// ---------------------------------------------------------------------------------------------
static void test_jump_encoders() {
    uint8_t buf[kJmpAbs] = {};
    WriteAbsJmp(buf, 0x1122334455667788ULL);
    uint64_t dest = 0;
    std::memcpy(&dest, buf + 6, 8);
    CHECK(buf[0] == 0xFF && buf[1] == 0x25 && Read32(buf + 2) == 0 && dest == 0x1122334455667788ULL,
          "the absolute jump is FF 25 <disp32=0> followed by the target");

    uint8_t rel[kJmpRel32] = {};
    CHECK(WriteRelJmp(rel, kSrcAddr, kSrcAddr + 0x1000), "a near destination encodes as E9");
    CHECK(rel[0] == 0xE9 && Read32(rel + 1) == 0x1000 - kJmpRel32, "with the rel32 measured from the END of the jump");
    CHECK(!WriteRelJmp(rel, kSrcAddr, kSrcAddr + 0xC0000000ULL),
          "a destination beyond rel32 range is REFUSED, so the caller falls back to the absolute form");
}

// ---------------------------------------------------------------------------------------------
// 8. End-to-end: actually install a detour on a real function in this binary and call it.
//
//    Everything above tests BuildTrampolineBody, which is bytes-to-bytes. This tests Install — the
//    patch itself, the trampoline layout, the jump back, and (for the near tier) the island the E9
//    lands on. Without it the first thing to ever execute a relocated prologue would be a game
//    server, which is a poor place to discover that the island offset is wrong.
//
//    The target is hand-written asm so its prologue is KNOWN rather than whatever the compiler felt
//    like emitting at this optimisation level — a 4-byte compiler prologue would steal into the next
//    function and make the test a coin flip.
extern "C" int64_t s2_detour_test_target(int64_t x);
#if !defined(_MSC_VER)
__asm__(
    ".text\n"
    ".globl s2_detour_test_target\n"
    ".type s2_detour_test_target,@function\n"
    "s2_detour_test_target:\n"
    "  push %rbp\n"            // 55
    "  mov %rsp, %rbp\n"       // 48 89 e5
    "  push %r15\n"            // 41 57      <- 6 bytes: the near tier stops here
    "  mov %rdi, %rax\n"
    "  add $1, %rax\n"
    "  pop %r15\n"
    "  pop %rbp\n"
    "  ret\n"
    ".size s2_detour_test_target,.-s2_detour_test_target\n");
#endif

static int64_t (*g_origTarget)(int64_t) = nullptr;
static int     g_handlerCalls = 0;

extern "C" int64_t s2_detour_test_handler(int64_t x) {
    g_handlerCalls++;
    return g_origTarget(x) * 10;   // reaches the ORIGINAL through the trampoline
}

static void test_install_diverts_and_the_trampoline_still_works() {
    CHECK(s2_detour_test_target(5) == 6, "the un-patched target returns x+1");

    void* orig = nullptr;
    const InstallResult r = Install(reinterpret_cast<void*>(&s2_detour_test_target),
                                    reinterpret_cast<void*>(&s2_detour_test_handler),
                                    &orig, nullptr);
    CHECK(r.ok, r.ok ? "Install patched the target" : (r.reason ? r.reason : "Install failed"));
    if (!r.ok) return;
    CHECK(r.stolen == 6, "it stole the 6-byte push/mov/push prologue");
    g_origTarget = reinterpret_cast<int64_t (*)(int64_t)>(orig);

    g_handlerCalls = 0;
    const int64_t got = s2_detour_test_target(5);
    CHECK(g_handlerCalls == 1, "calling the target reaches OUR handler");
    CHECK(got == 60, "and the handler's call to the trampoline runs the ORIGINAL body (5+1)*10");

    // Twice, because a trampoline that works once and corrupts state would still pass a single call.
    CHECK(s2_detour_test_target(9) == 100 && g_handlerCalls == 2, "and it is re-entrant across calls");

    RemoveAll();
    CHECK(s2_detour_test_target(5) == 6, "RemoveAll restores the original bytes");
    CHECK(r.usedNearJump, "a normally-loaded binary takes the NEAR (5-byte E9) tier");
}

static void test_the_far_tier_also_works_end_to_end() {
    // Forced, because on a normal process the near tier always wins. This is the path that runs when
    // address space is tight — the moment you least want to discover it was never executed.
    SetForceFarTierForTest(true);

    void* orig = nullptr;
    const InstallResult r = Install(reinterpret_cast<void*>(&s2_detour_test_target),
                                    reinterpret_cast<void*>(&s2_detour_test_handler),
                                    &orig, nullptr);
    CHECK(r.ok, r.ok ? "the far tier installs" : (r.reason ? r.reason : "far-tier Install failed"));
    if (r.ok) {
        CHECK(!r.usedNearJump, "and it reports the FAR tier");
        CHECK(r.stolen >= kJmpAbs, "stealing at least the 14 bytes its absolute jump needs");
        g_origTarget = reinterpret_cast<int64_t (*)(int64_t)>(orig);
        g_handlerCalls = 0;
        CHECK(s2_detour_test_target(5) == 60 && g_handlerCalls == 1,
              "the far tier diverts and its trampoline runs the original too");
        RemoveAll();
        CHECK(s2_detour_test_target(5) == 6, "and it restores cleanly");
    }
    SetForceFarTierForTest(false);
}

int main() {
    test_prologue_corpus_near_is_a_prefix_of_far();
    test_the_two_blocked_hook_targets();
    test_rip_relative_disp32_is_recomputed();
    test_disp32_offset_accounts_for_a_trailing_immediate();
    test_call_and_jmp_rel32_are_recomputed();
    test_relocated_code_actually_behaves_the_same();
    test_short_branches_are_refused_by_name();
    test_out_of_reach_displacement_is_refused();
    test_undecodable_is_refused();
    test_probe_gates_every_instruction();
    test_jump_encoders();
    test_install_diverts_and_the_trampoline_still_works();
    test_the_far_tier_also_works_end_to_end();

    if (g_fail) { std::cerr << "\n" << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "\nall detour relocation checks passed\n";
    return 0;
}
