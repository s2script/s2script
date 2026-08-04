// Unit test for the REAL descriptor-validator vocabulary (shim/src/call_validate.cpp) — the two
// semantic gates A5b adds (`vtable-member`, `string-xref`), the existing `prologue`, and the
// closure of the vocabulary itself.
//
// WHY THIS EXISTS. A validator that cannot fail in a test is decoration. Both gates exist for the
// SAME failure: a borrowed signature that matches UNIQUELY at the WRONG in-range function, which
// every cheaper check (uniqueness, .text range) passes. If that rejection is never exercised, a
// refactor can quietly turn it into a no-op and nothing goes red until a live server calls the
// wrong engine function.
//
// It drives the SHIPPED code, not a mock: the same parser, the same walk, the same reason strings.
// The engine is replaced by a SYNTHETIC MODULE IMAGE — one buffer laid out the way a real ELF
// mapping is (a .rodata region BELOW the .text region, so a rip-relative string target computes to
// a legitimately NEGATIVE offset, which is the case a .text-only range guard would wrongly reject)
// — and RTTI resolution is replaced by an injected fake that hands back a hand-built vtable.
//
// Self-contained: no SDK, no engine, no isolate, no V8, no libserver.so. Compiled + run by
// scripts/test-call-validate.sh out of scripts/ci-native.sh.
//
// ENGINE-GENERIC: every name below is a FIXTURE name. No CS2 class, function or string appears
// here — the validators never interpret one either.
#include "../src/call_validate.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }      \
        else         { std::cout << "ok:   " << (msg) << "\n"; }                \
    } while (0)

namespace {

// --- the synthetic module image ---------------------------------------------------------------
// Layout, mirroring a real mapping:
//
//   [0x0000, 0x1000)  "rodata"  — literals live here, BELOW the text base
//   [0x1000, 0x8000)  "text"    — functions live here
//
// mv.lo/mv.hi span the whole thing (the full mapped LOAD extent); mv.text points at 0x1000.
constexpr std::size_t kImageSize = 0x8000;
constexpr std::size_t kTextBase  = 0x1000;
constexpr std::size_t kTextSize  = kImageSize - kTextBase;

// Where the fixture function lives, as an offset from the TEXT base.
constexpr int64_t kFnOff = 0x2000;
// Offsets of the fixture literals, from the IMAGE base (i.e. inside the "rodata" region).
constexpr std::size_t kGoodStrOff  = 0x100;   // "ScopeAlpha"
constexpr std::size_t kLongerStr   = 0x200;   // "ScopeAlphaTail" — the NUL-compare trap

std::vector<uint8_t> g_image;

// The xref instruction's shape inside the fixture function:
//   fn+0 .. fn+10   eleven filler/prologue bytes
//   fn+11           48 8D 35 <disp32>      (lea rsi,[rip+disp32]; 7 bytes total)
constexpr int64_t kXrefAt       = 11;
constexpr int64_t kXrefDispOff  = 3;
constexpr int64_t kXrefInstrLen = 7;

void WriteI32(std::size_t imageOff, int32_t v) {
    for (int i = 0; i < 4; i++) g_image[imageOff + i] = static_cast<uint8_t>((v >> (8 * i)) & 0xFF);
}

void WriteStr(std::size_t imageOff, const char* s) {
    std::memcpy(&g_image[imageOff], s, std::strlen(s) + 1);   // including the NUL
}

// Point the fixture function's lea at `imageOff`. disp is relative to the instruction's END, and
// both sides are expressed as offsets from the TEXT base — so a rodata target yields a NEGATIVE
// displacement, exactly as in a real module.
void PointXrefAt(std::size_t imageOff) {
    const int64_t targetOff = static_cast<int64_t>(imageOff) - static_cast<int64_t>(kTextBase);
    const int64_t ripBase   = kFnOff + kXrefAt + kXrefInstrLen;
    WriteI32(kTextBase + kFnOff + kXrefAt + kXrefDispOff, static_cast<int32_t>(targetOff - ripBase));
}

void BuildImage() {
    g_image.assign(kImageSize, 0xCC);
    WriteStr(kGoodStrOff, "ScopeAlpha");
    WriteStr(kLongerStr,  "ScopeAlphaTail");

    // The fixture function's first bytes (a plausible prologue), then the lea.
    static const uint8_t kFnBytes[] = { 0x55, 0x48, 0x89, 0xE5, 0x41, 0x57, 0x41, 0x89, 0xF7, 0x41,
                                        0x56, 0x48, 0x8D, 0x35 };
    std::memcpy(&g_image[kTextBase + kFnOff], kFnBytes, sizeof(kFnBytes));
    PointXrefAt(kGoodStrOff);
}

const uint8_t* Image()    { return g_image.data(); }
const uint8_t* TextBase() { return g_image.data() + kTextBase; }
const void*    Fn()       { return TextBase() + kFnOff; }

s2validate::ModuleView View() {
    s2validate::ModuleView mv;
    mv.text     = TextBase();
    mv.textSize = kTextSize;
    mv.lo       = Image();
    mv.hi       = Image() + kImageSize;
    return mv;
}

// --- the fake RTTI resolver -------------------------------------------------------------------
// One hand-built "primary vtable": three function slots inside .text, then a value OUTSIDE .text
// standing in for the next sub-vtable's offset-to-top header (which is what terminates the real
// walk), then one more in-text slot BEYOND that terminator. That last entry is the fail-closed
// probe: a truncated walk must NOT reach it.
// Sized to the walk's own cap (512) ON PURPOSE: if the walk's fail-closed termination regresses to
// "keep going", the test must fail on the ASSERTION below, not on an out-of-bounds read that only a
// sanitizer-equipped toolchain would notice. The gate must be red on every toolchain.
constexpr int kVtableSlots = 512;
void* g_vtable[kVtableSlots];
std::string g_lastModuleAsked;      // the module string the validator forwarded
std::string g_lastClassAsked;
int         g_resolverCalls = 0;

constexpr int64_t kOtherFnOff   = 0x3000;   // in .text, in the vtable
constexpr int64_t kStrangerOff  = 0x5000;   // in .text, NOT in the vtable
constexpr int64_t kBeyondEndOff = 0x4000;   // in .text, in the vtable but PAST the terminator

void** FakeVtableByName(const char* module, const char* className) {
    g_resolverCalls++;
    g_lastModuleAsked = module ? module : "";
    g_lastClassAsked  = className ? className : "";
    if (std::strcmp(className, "CFixtureClass") != 0) return nullptr;   // not on this build
    g_vtable[0] = const_cast<void*>(reinterpret_cast<const void*>(TextBase() + kOtherFnOff));
    g_vtable[1] = const_cast<void*>(Fn());                              // the descriptor's target
    g_vtable[2] = reinterpret_cast<void*>(static_cast<uintptr_t>(0x18)); // offset-to-top: TERMINATOR
    g_vtable[3] = const_cast<void*>(reinterpret_cast<const void*>(TextBase() + kBeyondEndOff));
    for (int i = 4; i < kVtableSlots; i++) g_vtable[i] = nullptr;
    return g_vtable;
}

s2validate::Ops RealOps() {
    s2validate::Ops ops;
    ops.vtable_by_name = &FakeVtableByName;
    return ops;
}

// --- the harness -------------------------------------------------------------------------------
struct Result {
    bool        ok;
    std::string reason;
};

Result RunOn(const char* validateJson, const void* fn, const s2validate::Ops& ops) {
    char reason[256];
    std::memset(reason, 0x7F, sizeof(reason));   // poison: a passing run must still NUL it
    bool ok = s2validate::Run(validateJson, View(), "libfixture.so", fn, ops, reason, sizeof(reason));
    return Result{ ok, std::string(reason) };
}

Result RunOn(const char* validateJson) { return RunOn(validateJson, Fn(), RealOps()); }

bool Mentions(const Result& r, const char* needle) {
    return r.reason.find(needle) != std::string::npos;
}

// A reason must be BOTH present and named — an empty string would technically "fail" while telling
// an operator nothing, which is the degrade this project treats as a bug.
bool Named(const Result& r) { return !r.ok && !r.reason.empty(); }

// -----------------------------------------------------------------------------------------------
// vtable-member
// -----------------------------------------------------------------------------------------------
void test_vtable_member_accepts_a_real_member() {
    Result r = RunOn(R"({"vtable-member":"CFixtureClass"})");
    CHECK(r.ok, "vtable-member: an address IN the class's primary vtable passes");
    CHECK(r.reason.empty(), "vtable-member: a pass leaves the reason buffer empty");
    CHECK(g_lastClassAsked == "CFixtureClass", "vtable-member: the class name reaches the resolver");
    CHECK(g_lastModuleAsked == "libfixture.so",
          "vtable-member: the DESCRIPTOR's module is forwarded (not a hardcoded soname)");
}

// THE GATE. This is the unique-but-WRONG case: an address that is real, in .text, and would sail
// through every cheaper check — but is not a member of the named class's vtable.
void test_vtable_member_rejects_an_in_text_non_member() {
    Result r = RunOn(R"({"vtable-member":"CFixtureClass"})", TextBase() + kStrangerOff, RealOps());
    CHECK(Named(r), "vtable-member: an in-.text address that is NOT in the vtable is REJECTED");
    CHECK(Mentions(r, "CFixtureClass") && Mentions(r, "NOT a member"),
          "vtable-member: and the reason NAMES the class and the failure");
}

// Fail-closed: the walk stops at the first slot outside .text (a sub-vtable header). A member found
// only PAST that terminator must not be accepted — a truncated walk never passes wrongly.
void test_vtable_member_walk_termination_is_fail_closed() {
    Result r = RunOn(R"({"vtable-member":"CFixtureClass"})", TextBase() + kBeyondEndOff, RealOps());
    CHECK(Named(r), "vtable-member: a slot PAST the walk terminator is rejected (fail-closed)");
}

void test_vtable_member_unresolvable_class_degrades_by_name() {
    Result r = RunOn(R"({"vtable-member":"CAbsentClass"})");
    CHECK(Named(r) && Mentions(r, "CAbsentClass") && Mentions(r, "not found"),
          "vtable-member: a class with no RTTI on this build degrades, naming the class");
}

void test_vtable_member_without_a_resolver_degrades_by_name() {
    s2validate::Ops none;   // vtable_by_name == nullptr
    Result r = RunOn(R"({"vtable-member":"CFixtureClass"})", Fn(), none);
    CHECK(Named(r) && Mentions(r, "unavailable"),
          "vtable-member: no RTTI resolver available degrades by name (never a null call)");
}

void test_vtable_member_malformed_value_degrades_by_name() {
    CHECK(Named(RunOn(R"({"vtable-member":274})")),
          "vtable-member: a non-string class (a slot index!) is rejected");
    CHECK(Named(RunOn(R"({"vtable-member":""})")), "vtable-member: an empty class name is rejected");
}

// -----------------------------------------------------------------------------------------------
// string-xref
// -----------------------------------------------------------------------------------------------
const char* kGoodXref =
    R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":"ScopeAlpha"}})";

void test_string_xref_accepts_the_right_literal() {
    PointXrefAt(kGoodStrOff);
    Result r = RunOn(kGoodXref);
    CHECK(r.ok, "string-xref: the xref target matching `expect` passes");
    CHECK(r.reason.empty(), "string-xref: a pass leaves the reason buffer empty");
}

// THE GATE. Same function, same in-range unique match — a DIFFERENT expected string.
void test_string_xref_rejects_a_wrong_expectation() {
    PointXrefAt(kGoodStrOff);
    Result r = RunOn(R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":"ScopeOmega"}})");
    CHECK(Named(r), "string-xref: a wrong expected string is REJECTED");
    CHECK(Mentions(r, "ScopeOmega") && Mentions(r, "borrowed-sig trap"),
          "string-xref: and the reason NAMES the string the treadmill must re-xref");
}

// The compare includes the NUL, so a prefix is NOT a match. Without that, "Round" would be
// satisfied by "RoundEnd" — precisely the near-miss a borrowed signature lands on.
void test_string_xref_compare_includes_the_nul() {
    PointXrefAt(kLongerStr);   // the target is now "ScopeAlphaTail"
    Result r = RunOn(kGoodXref);
    CHECK(Named(r), "string-xref: `expect` being a strict PREFIX of the target is rejected (NUL-compared)");
    PointXrefAt(kGoodStrOff);
}

void test_string_xref_rejects_a_target_outside_the_module() {
    // A displacement far past the end of the mapping: a stale `at`/pattern computing off the map.
    WriteI32(kTextBase + kFnOff + kXrefAt + kXrefDispOff, 0x7000000);
    Result r = RunOn(kGoodXref);
    CHECK(Named(r) && Mentions(r, "outside the module"),
          "string-xref: a target outside the module's mapped extent is rejected");
    PointXrefAt(kGoodStrOff);
}

void test_string_xref_rejects_an_out_of_bounds_displacement_read() {
    // `at` past the end of the text buffer — the four displacement bytes cannot even be read.
    Result r = RunOn(R"({"string-xref":{"at":2000000000,"dispOff":3,"instrLen":7,"expect":"ScopeAlpha"}})");
    CHECK(Named(r) && Mentions(r, "out of bounds"),
          "string-xref: a displacement read outside the text buffer is rejected, not read");
}

void test_string_xref_rejects_a_self_contradictory_descriptor() {
    // dispOff + 4 > instrLen: the displacement would lie past the end of its own instruction.
    Result r = RunOn(R"({"string-xref":{"at":11,"dispOff":3,"instrLen":5,"expect":"ScopeAlpha"}})");
    CHECK(Named(r) && Mentions(r, "outside the instruction"),
          "string-xref: a displacement outside its own instruction is caught without the binary");
}

void test_string_xref_malformed_shapes_degrade_by_name() {
    CHECK(Named(RunOn(R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7}})")),
          "string-xref: a missing `expect` is rejected");
    CHECK(Named(RunOn(R"({"string-xref":{"dispOff":3,"instrLen":7,"expect":"ScopeAlpha"}})")),
          "string-xref: a missing `at` is rejected");
    CHECK(Named(RunOn(R"({"string-xref":"ScopeAlpha"})")),
          "string-xref: a bare string instead of the parameter object is rejected");
    CHECK(Named(RunOn(R"({"string-xref":{"at":-1,"dispOff":3,"instrLen":7,"expect":"ScopeAlpha"}})")),
          "string-xref: a negative `at` is rejected");
    CHECK(Named(RunOn(R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":""}})")),
          "string-xref: an empty `expect` is rejected");
    std::string huge = R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":")" +
                       std::string(300, 'x') + R"("}})";
    CHECK(Named(RunOn(huge.c_str())), "string-xref: an oversized `expect` is rejected");
}

// -----------------------------------------------------------------------------------------------
// prologue (unchanged behaviour, now evaluated through the same shared pass)
// -----------------------------------------------------------------------------------------------
void test_prologue_still_matches_and_still_rejects() {
    CHECK(RunOn(R"({"prologue":"55 48 89 E5 ? 57"})").ok,
          "prologue: a masked pattern matching at the address passes");
    Result bad = RunOn(R"({"prologue":"90 90 90"})");
    CHECK(Named(bad) && Mentions(bad, "prologue mismatch"), "prologue: a mismatch is rejected by name");
    CHECK(Named(RunOn(R"({"prologue":"zz"})")), "prologue: a malformed pattern is rejected by name");
    CHECK(s2validate::DeclaresPrologue(R"({"prologue":"55 48"})"),
          "prologue: DeclaresPrologue sees a declared one (the vtable target's mandatory rule)");
    CHECK(!s2validate::DeclaresPrologue(R"({"vtable-member":"CFixtureClass"})"),
          "prologue: DeclaresPrologue is false when only another validator is declared");
    CHECK(!s2validate::DeclaresPrologue(""), "prologue: DeclaresPrologue is false for no validators");
}

// -----------------------------------------------------------------------------------------------
// the vocabulary is CLOSED
// -----------------------------------------------------------------------------------------------
void test_an_unknown_validator_is_never_ignored() {
    // The exact typo class this exists for: the descriptor still carries its gate, spelled wrong.
    Result r = RunOn(R"({"vtable_member":"CFixtureClass"})");
    CHECK(Named(r) && Mentions(r, "unknown validator") && Mentions(r, "vtable_member"),
          "closure: an unknown validator key FAILS the descriptor by name, never silently passes");
    CHECK(Mentions(r, "prologue") && Mentions(r, "string-xref") && Mentions(r, "vtable-member"),
          "closure: and the reason names the whole valid set");
}

void test_an_unknown_key_is_not_masked_by_a_sibling() {
    // A known sibling that would ALSO fail must not steal the reason: the typo is the real defect.
    Result r = RunOn(R"({"stringxref":{},"prologue":"90 90 90"})");
    CHECK(Named(r) && Mentions(r, "unknown validator"),
          "closure: an unknown key wins the reason over a failing known sibling");
}

void test_no_validators_declared_passes() {
    CHECK(RunOn("").ok,        "closure: an empty validate string means no validators (passes)");
    CHECK(RunOn("{}").ok,      "closure: an empty object means no validators (passes)");
    CHECK(RunOn("null").ok,    "closure: a JSON null means no validators (passes)");
    CHECK(RunOn(nullptr, Fn(), RealOps()).ok, "closure: a null pointer means no validators (passes)");
}

void test_malformed_validate_object_degrades_by_name() {
    CHECK(Named(RunOn("{not json")), "closure: an unparseable validate blob degrades by name");
    CHECK(Named(RunOn(R"("55 48 89 E5")")),
          "closure: a validate that is a bare string, not an object, degrades by name");
}

// -----------------------------------------------------------------------------------------------
// several validators at once
// -----------------------------------------------------------------------------------------------
void test_validators_are_anded_and_ordered() {
    PointXrefAt(kGoodStrOff);
    std::string both = R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":"ScopeAlpha"},)"
                       R"("vtable-member":"CFixtureClass"})";
    CHECK(RunOn(both.c_str()).ok, "multi: two passing validators both pass");

    // A passing string-xref must NOT short-circuit the vtable-member check.
    std::string xrefOkVtableWrong =
        R"({"string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":"ScopeAlpha"},)"
        R"("vtable-member":"CAbsentClass"})";
    Result r = RunOn(xrefOkVtableWrong.c_str());
    CHECK(Named(r) && Mentions(r, "CAbsentClass"),
          "multi: every declared validator runs — a passing one does not short-circuit the rest");

    // Both failing: the CHEAPEST, most local signal is the one reported (prologue < string-xref <
    // vtable-member), regardless of the order the author typed them in.
    std::string bothWrong = R"({"vtable-member":"CAbsentClass","prologue":"90 90 90"})";
    Result q = RunOn(bothWrong.c_str());
    CHECK(Named(q) && Mentions(q, "prologue mismatch"),
          "multi: with two failures the cheapest validator's reason wins, not JSON order");

    std::string xrefAndVtable =
        R"({"vtable-member":"CAbsentClass",)"
        R"("string-xref":{"at":11,"dispOff":3,"instrLen":7,"expect":"ScopeOmega"}})";
    Result s = RunOn(xrefAndVtable.c_str());
    CHECK(Named(s) && Mentions(s, "ScopeOmega"),
          "multi: string-xref is reported ahead of the far more expensive vtable-member");
}

// A validator must never be handed an address with no provenance: the caller range-checks first,
// and this is the backstop that keeps the pure unit self-contained.
void test_an_out_of_text_address_never_reaches_a_validator() {
    g_resolverCalls = 0;
    Result r = RunOn(R"({"vtable-member":"CFixtureClass"})", Image() + 0x10, RealOps());
    CHECK(Named(r), "input: an address outside .text degrades by name");
    CHECK(g_resolverCalls == 0, "input: and no validator ran on it");
}

}  // namespace

// ---------------------------------------------------------------------------
// arg-width — the shape-vs-machine-code check.
// ---------------------------------------------------------------------------

// Build a ModuleView over a caller-owned buffer treated as .text.
static s2validate::ModuleView ViewOver(const uint8_t* buf, std::size_t n) {
    s2validate::ModuleView mv;
    mv.text = buf; mv.textSize = n; mv.lo = buf; mv.hi = buf + n;
    return mv;
}

// THE REGRESSION. CCSGameRules::TerminateRound's REAL prologue, byte for byte:
//   push rbp; mov rbp,rsp; push r15; mov r15d,esi; push r14; lea rsi,[rip+d];
//   push r13; push r12; push rbx; mov rbx,rdi; sub rsp,0xc8;
//   lea rax,[rip+d]; movss %xmm0,-0xd8(%rbp); mov %rdx,-0xe0(%rbp)   <-- 64-BIT store of arg2
// Declared this_f32_i32_i32_i32 this truncated a pointer and segfaulted a live server.
static const uint8_t kTerminateRound[] = {
    0x55, 0x48,0x89,0xE5, 0x41,0x57, 0x41,0x89,0xF7, 0x41,0x56,
    0x48,0x8D,0x35,0x5D,0x91,0x56,0xFF, 0x41,0x55, 0x41,0x54, 0x53,
    0x48,0x89,0xFB, 0x48,0x81,0xEC,0xC8,0x00,0x00,0x00,
    0x48,0x8D,0x05,0x60,0x34,0x4E,0x01,
    0xF3,0x0F,0x11,0x85,0x28,0xFF,0xFF,0xFF,
    0x48,0x89,0x95,0x20,0xFF,0xFF,0xFF,
};

static void test_arg_width_catches_the_terminate_round_truncation() {
    char r[256];
    auto mv = ViewOver(kTerminateRound, sizeof kTerminateRound);
    // this_f32_i32_i32_i32 flattened to INTEGER slots: [this, reason, _unused3, _unused4] — the
    // float delay rides in xmm0 and consumes no integer register. _unused3 is slot 2 = rdx, which
    // the callee stores 64-bit -> REFUSE.
    const uint8_t narrow[4] = { 1, 0, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 4, mv, kTerminateRound, r, sizeof r) == -1,
          "TerminateRound's real prologue is REFUSED under this_f32_i32_i32_i32");
    CHECK(std::strstr(r, "slot 2") != nullptr,
          "and the reason names the offending integer arg slot");

    // this_f32_i32_i64_i64: the trailing pair relayed at full width -> accepted.
    const uint8_t wide[4] = { 1, 0, 1, 1 };
    CHECK(s2validate::ArgWidths(wide, 4, mv, kTerminateRound, r, sizeof r) == 0,
          "and ACCEPTED under this_f32_i32_i64_i64 — the shape that actually shipped");
}

static void test_arg_width_passes_a_clean_32_bit_prologue() {
    // push rbp; mov rbp,rsp; mov %esi,-0x4(%rbp); mov %edx,-0x8(%rbp)   (both 32-bit stores)
    static const uint8_t code[] = {
        0x55, 0x48,0x89,0xE5, 0x89,0x75,0xFC, 0x89,0x55,0xF8, 0xC3 };
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t narrow[3] = { 1, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 3, mv, code, r, sizeof r) == 0,
          "a prologue that stores its args 32-bit agrees with a 32-bit shape");
    CHECK(std::strstr(r, "no narrowing") != nullptr, "and says what it checked");
}

static void test_arg_width_ignores_a_redefined_register() {
    // push rbp; mov rbp,rsp; mov $1,%edx; mov %rdx,-0x8(%rbp)
    // The 64-bit store is of a value the FUNCTION put in rdx, not of the incoming argument. Without
    // redefinition tracking this is a false FAIL — which is how a gate gets switched off.
    static const uint8_t code[] = {
        0x55, 0x48,0x89,0xE5, 0xBA,0x01,0x00,0x00,0x00, 0x48,0x89,0x55,0xF8, 0xC3 };
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t narrow[3] = { 1, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 3, mv, code, r, sizeof r) == 0,
          "a 64-bit store of a REDEFINED register is not evidence about the argument");
}

static void test_arg_width_passes_when_it_sees_nothing() {
    static const uint8_t code[] = { 0x55, 0x48,0x89,0xE5, 0x5D, 0xC3 };   // push;mov;pop;ret
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t narrow[3] = { 1, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 3, mv, code, r, sizeof r) == 0,
          "a prologue that spills nothing PASSES — absence of evidence is not a failure");
    CHECK(std::strstr(r, "not proof") != nullptr,
          "and the reason refuses to be read as a proof");
}

static void test_arg_width_checks_every_param_position() {
    // 64-bit stores of rsi(param0), rdx(param1), rcx(param2), r8(param3) in turn — each must be
    // caught at its OWN index, so a table shifted by one cannot pass.
    struct C { const char* name; std::vector<uint8_t> code; int param; };
    const std::vector<C> cases = {
        { "rsi", { 0x48,0x89,0x75,0xF8, 0xC3 }, 1 },
        { "rdx", { 0x48,0x89,0x55,0xF8, 0xC3 }, 2 },
        { "rcx", { 0x48,0x89,0x4D,0xF8, 0xC3 }, 3 },
        { "r8",  { 0x4C,0x89,0x45,0xF8, 0xC3 }, 4 },
    };
    for (const auto& c : cases) {
        char r[256];
        auto mv = ViewOver(c.code.data(), c.code.size());
        const uint8_t narrow[5] = { 1, 0, 0, 0, 0 };
        CHECK(s2validate::ArgWidths(narrow, 5, mv, c.code.data(), r, sizeof r) == -1,
              std::string("a 64-bit store of ") + c.name + " is refused");
        CHECK(std::strstr(r, ("slot " + std::to_string(c.param)).c_str()) != nullptr,
              std::string("and is attributed to integer arg slot ") + std::to_string(c.param));
    }
}

static void test_arg_width_never_reads_past_the_view() {
    // The view ENDS mid-instruction: the last store is truncated to 2 of its 7 bytes.
    //
    // The buffer is EXACTLY the size of the view, and that is the whole test. An earlier version cut
    // the view logically while handing in the full 55-byte array, so "past the view" was still
    // inside the allocation and ASan had nothing to report — the test passed with the bounded-window
    // copy REMOVED. Past-the-view must be past-the-allocation or this proves nothing.
    char r[256];
    const std::size_t cut = sizeof kTerminateRound - 5;
    std::vector<uint8_t> exact(kTerminateRound, kTerminateRound + cut);
    auto mv = ViewOver(exact.data(), exact.size());
    const uint8_t narrow[4] = { 1, 0, 0, 0 };
    const int rc = s2validate::ArgWidths(narrow, 4, mv, exact.data(), r, sizeof r);
    CHECK(rc == 0, "a view ending mid-instruction stops cleanly instead of reading past it");
    CHECK(std::strstr(r, "arg-width") != nullptr, "and still renders a verdict rather than garbage");
}

// A maximal run of legacy prefixes: hde64's prefix loop runs SIXTEEN iterations before the opcode,
// so a window sized to the longest legal instruction (15) is one byte short of what the DISASSEMBLER
// can touch. This exact input walked off the end of the stack buffer (ASan-confirmed) in the
// function whose comment promised it could not.
static void test_arg_width_survives_a_maximal_prefix_run() {
    std::vector<uint8_t> prefixes(15, 0x66);
    prefixes.push_back(0x90);
    char r[256];
    auto mv = ViewOver(prefixes.data(), prefixes.size());
    const uint8_t narrow[3] = { 1, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 3, mv, prefixes.data(), r, sizeof r) == 0,
          "a maximal legacy-prefix run decodes without reading past the decode window");
}

// TOO FEW ARGUMENTS. A callee that reads slot 4 when the shape declares two is a shape that will
// make the thunk hand the original a register our JS dispatch has already clobbered — the same crash
// as a truncated pointer, by a different route.
//
// This also carries the ASan property the earlier version of this test was written for: the refusal
// must happen BEFORE `wide[u.argIndex]` is indexed, so an out-of-range slot can never read past the
// caller's array. Both facts are asserted here because they share one code path.
static void test_arg_width_refuses_a_shape_that_declares_too_few_args() {
    // mov %r8,-0x8(%rbp) ; ret     -> the callee reads slot 4
    static const uint8_t code[] = { 0x4C,0x89,0x45,0xF8, 0xC3 };
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t twoSlots[2] = { 1, 0 };          // this + one param; slot 4 is NOT declared
    CHECK(s2validate::ArgWidths(twoSlots, 2, mv, code, r, sizeof r) == -1,
          "a callee reading an argument the shape does not declare is REFUSED");
    CHECK(std::strstr(r, "slot 4") != nullptr && std::strstr(r, "2 integer arg slot") != nullptr,
          "and the reason names both the missing slot and what the shape declared");
}

// The other direction is harmless and must PASS: the callee simply ignores arguments we relay that
// it does not take. Only declaring too FEW is dangerous — the same asymmetry as width.
static void test_arg_width_allows_a_shape_that_declares_more_args_than_the_callee_reads() {
    // mov %esi,-0x8(%rbp) ; ret    -> the callee reads only slot 1
    static const uint8_t code[] = { 0x89,0x75,0xF8, 0xC3 };
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t sixSlots[6] = { 1, 0, 0, 0, 0, 0 };
    CHECK(s2validate::ArgWidths(sixSlots, 6, mv, code, r, sizeof r) == 0,
          "declaring MORE arguments than the callee reads is harmless and passes");
}

// The scan must stop at the end of the callee's own flow. Without this it marches into the NEXT
// function and blames its spills on our shape — 490 of 3497 refusals in a libserver.so corpus scan.
static void test_arg_width_stops_at_the_end_of_the_function() {
    // ret, then (as the linker would lay out) another function that spills rdx 64-bit.
    static const uint8_t code[] = { 0xC3, 0x48,0x89,0x55,0xF8, 0xC3 };
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t narrow[4] = { 1, 0, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 4, mv, code, r, sizeof r) == 0,
          "a store past the callee's own `ret` is NOT evidence about its arguments");
}

// The operand-direction case in ISOLATION, asserted on the DECODER rather than through the scan, so
// a regression names the bug instead of surfacing as a distant false refusal.
static void test_arg_width_decoder_gets_alu_operand_direction_right() {
    using s2validate::DecodeArgUse;
    { const uint8_t b[] = {0x48,0x01,0xC2};     // add %rax,%rdx  -> r/m<-r, destination is rdx
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.redefines == 2, "ADD r/m,r redefines the r/m operand (rdx), not the source"); }
    { const uint8_t b[] = {0x48,0x01,0xD0};     // add %rdx,%rax  -> destination is rax, NOT an arg
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.redefines == -1, "and when the destination is not an argument register, nothing is"); }
    { const uint8_t b[] = {0x48,0x0B,0xD1};     // or %rcx,%rdx   -> r<-r/m, destination is rdx
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.redefines == 2, "OR r,r/m redefines the reg operand — the mirror direction"); }
    { const uint8_t b[] = {0x48,0x01,0x57,0x08}; // add %rdx,0x8(%rdi) -> memory dest kills no reg
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.redefines == -1, "an ALU write to MEMORY redefines no register at all"); }
}

// A `call` clobbers every SysV integer argument register, so a 64-bit spill after one describes a
// return value, not an incoming argument. CCSPlayerController_Respawn's real prologue calls at +0xa.
static void test_arg_width_stops_at_a_call() {
    // call rel32 ; mov %rdx,-0x8(%rbp) ; ret
    static const uint8_t code[] = { 0xE8,0x00,0x00,0x00,0x00, 0x48,0x89,0x55,0xF8, 0xC3 };
    char r[256];
    auto mv = ViewOver(code, sizeof code);
    const uint8_t narrow[4] = { 1, 0, 0, 0 };
    CHECK(s2validate::ArgWidths(narrow, 4, mv, code, r, sizeof r) == 0,
          "a 64-bit spill AFTER a call is a return value, not an argument");
}

// movslq is the canonical encoding of "a 32-bit int argument widened for use as an index" — the
// shape is CORRECT and the hook was being refused. mov $imm,%rdx (0xC7) is the sibling of the
// 0xB8 form that was already tracked: same semantics, opposite verdict, purely by encoding.
static void test_arg_width_tracks_the_wider_redefinition_set() {
    struct C { const char* name; std::vector<uint8_t> code; };
    const std::vector<C> cases = {
        { "movslq %edx,%rdx",  { 0x48,0x63,0xD2, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "mov $1,%rdx",       { 0x48,0xC7,0xC2,0x01,0x00,0x00,0x00, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "lea -0x8(%rbp),%rdx",{0x48,0x8D,0x55,0xF8, 0x48,0x89,0x55,0xF0, 0xC3 } },
        { "pop %rdx",          { 0x5A, 0x48,0x89,0x55,0xF8, 0xC3 } },
        // NON-DEGENERATE on purpose. `xor %edx,%edx` (31 D2) has reg == rm, so it passes whether the
        // decoder credits the source or the destination — it is the one encoding that CANNOT expose
        // an operand-direction bug, and using it here is what hid exactly that bug.
        { "xor %eax,%edx (r/m<-r)", { 0x31,0xC2, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "add %rax,%rdx (r/m<-r)", { 0x48,0x01,0xC2, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "or  %rcx,%rdx (r<-r/m)", { 0x48,0x0B,0xD1, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "movzbl (%rax),%edx",     { 0x0F,0xB6,0x10, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "movswl (%rax),%edx",     { 0x0F,0xBF,0x10, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "cqto (kills rdx)",       { 0x48,0x99, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "divq %rcx (kills rdx)",  { 0x48,0xF7,0xF1, 0x48,0x89,0x55,0xF8, 0xC3 } },
        { "movq %xmm0,%rdx",        { 0x66,0x48,0x0F,0x7E,0xC2, 0x48,0x89,0x55,0xF8, 0xC3 } },
    };
    for (const auto& c : cases) {
        char r[256];
        auto mv = ViewOver(c.code.data(), c.code.size());
        const uint8_t narrow[4] = { 1, 0, 0, 0 };
        CHECK(s2validate::ArgWidths(narrow, 4, mv, c.code.data(), r, sizeof r) == 0,
              std::string("a spill after `") + c.name + "` is not evidence about the argument");
    }
}

static void test_arg_width_decoder_reports_redefinitions_and_stores() {
    using s2validate::DecodeArgUse;
    { const uint8_t b[] = {0x48,0x89,0x55,0xF8};             // mov %rdx,-0x8(%rbp)
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.argIndex == 2 && u.wide && u.length == 4, "decodes a 64-bit store of rdx as arg 2"); }
    { const uint8_t b[] = {0x89,0x55,0xF8};                   // mov %edx,-0x8(%rbp)
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.argIndex == 2 && !u.wide, "decodes a 32-bit store of edx as arg 2, not wide"); }
    { const uint8_t b[] = {0x48,0x89,0x5D,0xF8};              // mov %rbx,-0x8(%rbp)  (not an arg reg)
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.argIndex == -1, "a store of a non-argument register is not an observation"); }
    { const uint8_t b[] = {0x48,0x8B,0xD1};                   // mov %rcx,%rdx -> rdx redefined
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.redefines == 2, "MOV r,r/m records the destination as redefined"); }
    { const uint8_t b[] = {0x48,0x89,0xC2};                   // mov %rax,%rdx -> rdx redefined
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.redefines == 2, "MOV r/m,r with a REGISTER destination redefines it"); }
    { const uint8_t b[] = {0x48,0x89};                        // truncated
      auto u = DecodeArgUse(b, sizeof b);
      CHECK(u.length == 0, "a truncated instruction decodes to length 0 and stops the walk"); }
}

int main() {
    std::cout << std::unitbuf;
    BuildImage();

    test_vtable_member_accepts_a_real_member();
    test_vtable_member_rejects_an_in_text_non_member();
    test_vtable_member_walk_termination_is_fail_closed();
    test_vtable_member_unresolvable_class_degrades_by_name();
    test_vtable_member_without_a_resolver_degrades_by_name();
    test_vtable_member_malformed_value_degrades_by_name();

    test_string_xref_accepts_the_right_literal();
    test_string_xref_rejects_a_wrong_expectation();
    test_string_xref_compare_includes_the_nul();
    test_string_xref_rejects_a_target_outside_the_module();
    test_string_xref_rejects_an_out_of_bounds_displacement_read();
    test_string_xref_rejects_a_self_contradictory_descriptor();
    test_string_xref_malformed_shapes_degrade_by_name();

    test_prologue_still_matches_and_still_rejects();

    test_an_unknown_validator_is_never_ignored();
    test_an_unknown_key_is_not_masked_by_a_sibling();
    test_no_validators_declared_passes();
    test_malformed_validate_object_degrades_by_name();

    test_validators_are_anded_and_ordered();
    test_an_out_of_text_address_never_reaches_a_validator();

    test_arg_width_decoder_reports_redefinitions_and_stores();
    test_arg_width_catches_the_terminate_round_truncation();
    test_arg_width_passes_a_clean_32_bit_prologue();
    test_arg_width_ignores_a_redefined_register();
    test_arg_width_passes_when_it_sees_nothing();
    test_arg_width_checks_every_param_position();
    test_arg_width_never_reads_past_the_view();
    test_arg_width_survives_a_maximal_prefix_run();
    test_arg_width_refuses_a_shape_that_declares_too_few_args();
    test_arg_width_allows_a_shape_that_declares_more_args_than_the_callee_reads();
    test_arg_width_stops_at_the_end_of_the_function();
    test_arg_width_decoder_gets_alu_operand_direction_right();
    test_arg_width_stops_at_a_call();
    test_arg_width_tracks_the_wider_redefinition_set();

    if (g_fail) { std::cerr << "call_validate_test: " << g_fail << " FAILURE(S)\n"; return 1; }
    std::cout << "call_validate_test: all checks passed\n";
    return 0;
}
