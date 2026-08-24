#include "call_validate.h"

#include "sigscan.h"
#include "../third_party/json.hpp"

extern "C" {
#include "hde64.h"
}

#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace s2validate {
namespace {

using nlohmann::json;

// THE VOCABULARY, spelled ONCE, in evaluation order (cheapest and most local first):
//
//   prologue     — a masked byte compare at `fn`. One bounds-checked read of a handful of bytes.
//   string-xref  — one bounds-checked i32 read plus one bounded memcmp in .rodata.
//   vtable-member— an RTTI .rodata name scan plus a walk of up to kMaxVtableSlots slots. By far the
//                  most expensive, and the only one that can fail for a reason unrelated to `fn`
//                  (the class's RTTI is absent from this build).
//
// The dispatcher switches on this array and the unknown-validator reason prints it, so a validator
// cannot be added to one without the other.
const char* const kVocabulary[] = { "prologue", "string-xref", "vtable-member" };
constexpr int kVocabularyCount = 3;

// A vtable has a naturally small bound. The walk terminates on its own at the first slot outside
// .text (the next sub-vtable's offset-to-top header); this caps a corrupt or hostile table so the
// walk can never run away — the same bound `engine_calls.cpp` applies to a declared slot index.
constexpr int kMaxVtableSlots = 512;

// Bounds the compare AND the reason string. 256 is ample for an engine log/scope literal.
constexpr std::size_t kMaxExpect = 256;

bool Fail(char* out, int cap, const char* fmt, ...) {
    if (out && cap > 0) {
        va_list ap;
        va_start(ap, fmt);
        std::vsnprintf(out, static_cast<std::size_t>(cap), fmt, ap);
        va_end(ap);
    }
    return false;
}

bool InText(const ModuleView& mv, const void* p) {
    if (!mv.text || !p) return false;
    const uint8_t* q = static_cast<const uint8_t*>(p);
    return q >= mv.text && q < mv.text + mv.textSize;
}

// Parse the whole `validate` object. Non-throwing (`allow_exceptions=false`): a malformed blob is a
// named degrade of ONE descriptor, never an exception escaping into the load path.
// Absent/""/"null"/"{}" all mean "no validators declared" and yield an empty object.
bool ParseValidate(const char* validateJson, json* out) {
    *out = json::object();
    if (!validateJson || !validateJson[0]) return true;
    json j = json::parse(validateJson, nullptr, /*allow_exceptions=*/false, /*ignore_comments=*/true);
    if (j.is_discarded()) return false;
    if (j.is_null()) return true;
    if (!j.is_object()) return false;
    *out = std::move(j);
    return true;
}

// -----------------------------------------------------------------------------------------------
// prologue — the masked compare at `fn`.
//
// The original (and still mandatory) validator for a `vtable` target: on the pinned libserver.so the
// borrowed ItemServices slots 24/25/26 are overload THUNKS, which are valid in-range code, so the
// .text-range guard does not catch them. Kind-agnostic here: a `signature` target may declare one
// too, and it is then checked identically.
// -----------------------------------------------------------------------------------------------
bool ValidatePrologue(const json& v, const ModuleView& mv, const void* fn, char* out, int cap) {
    if (!v.is_string() || v.get<std::string>().empty())
        return Fail(out, cap, "malformed validate.prologue (expected a byte pattern string)");
    std::vector<int> pat = s2sig::ParsePattern(v.get<std::string>());
    if (pat.empty()) return Fail(out, cap, "malformed validate.prologue pattern");

    const uint8_t* p = static_cast<const uint8_t*>(fn);
    // Fully bounds-checked against the module's text BEFORE any read.
    if (!mv.text || !p || p < mv.text || p + pat.size() > mv.text + mv.textSize)
        return Fail(out, cap, "prologue mismatch (resolved slot is not the intended function)");
    for (std::size_t i = 0; i < pat.size(); i++) {
        if (pat[i] >= 0 && p[i] != static_cast<uint8_t>(pat[i]))
            return Fail(out, cap, "prologue mismatch (resolved slot is not the intended function)");
    }
    return true;
}

// -----------------------------------------------------------------------------------------------
// string-xref — follow ONE rip-relative displacement out of the resolved function and assert it
// targets a known literal.
//
// The generalization of the bespoke gate the round-control slice needed: a borrowed signature that
// matches UNIQUELY at the WRONG function is in-range, unique, and silent — but the wrong function
// does not reference the right string.
//
//   at       — offset from the function start to the first byte of the rip-relative instruction
//   dispOff  — offset of the little-endian int32 displacement WITHIN that instruction
//   instrLen — total instruction length (the rip base is at + instrLen)
//   expect   — the exact C string the displacement must target, compared INCLUDING its NUL
//
// Deliberately NOT named `lea-disp` even though the closed RESOLVER vocabulary already has one: the
// resolver hardcodes (dispOff=3, instrLen=7) and RETURNS the target; the validator parameterizes
// them and COMPARES it. Same primitive, different verb.
//
// This validator cannot verify that an instruction begins at `at`, let alone that it is a `lea` —
// s2sig::ResolveLeaDisp just reads four bytes. The invariant that makes it sound is that the
// SIGNATURE PATTERN pins the opcode bytes and wildcards only the displacement, which is enforced
// statically by scripts/check-gamedata-sigs.sh, before shipping — a stronger check than a runtime
// byte compare could be.
// -----------------------------------------------------------------------------------------------
bool ValidateStringXref(const json& v, const ModuleView& mv, const void* fn, char* out, int cap) {
    static const char kMalformed[] =
        "malformed validate.string-xref (expected {at, dispOff, instrLen, expect})";
    if (!v.is_object()) return Fail(out, cap, "%s", kMalformed);
    for (const char* key : { "at", "dispOff", "instrLen" }) {
        if (!v.contains(key) || !v[key].is_number_integer()) return Fail(out, cap, "%s", kMalformed);
    }
    if (!v.contains("expect") || !v["expect"].is_string()) return Fail(out, cap, "%s", kMalformed);

    const int64_t at       = v["at"].get<int64_t>();
    const int64_t dispOff  = v["dispOff"].get<int64_t>();
    const int64_t instrLen = v["instrLen"].get<int64_t>();
    if (at < 0 || dispOff < 0 || instrLen <= 0)
        return Fail(out, cap, "validate.string-xref has a negative/zero offset");
    // Self-contradictory descriptor: the displacement would lie past the end of its own
    // instruction, so the rip base could never be right. Catchable without touching the binary.
    if (dispOff + 4 > instrLen)
        return Fail(out, cap, "validate.string-xref displacement lies outside the instruction");
    // The int arithmetic below is done in int64 and only narrowed once the values are known sane.
    if (at > INT32_MAX || instrLen > INT32_MAX)
        return Fail(out, cap, "validate.string-xref offsets are out of range");

    const std::string expect = v["expect"].get<std::string>();
    if (expect.empty() || expect.size() > kMaxExpect)
        return Fail(out, cap,
                    "validate.string-xref 'expect' must be a non-empty string of at most %zu bytes",
                    kMaxExpect);
    // A JSON string CAN carry an interior NUL (a unicode escape of zero). Core's fail-closed
    // C-string rule never sees this one, because `expect` crosses inside the JSON blob rather
    // than as a C string of its own — so fail closed HERE rather than compare against a string
    // the reason line cannot even print.
    if (expect.find('\0') != std::string::npos)
        return Fail(out, cap, "validate.string-xref 'expect' contains an interior NUL");

    if (!mv.text) return Fail(out, cap, "validate.string-xref: module text unavailable");
    const int64_t fnOff = static_cast<const uint8_t*>(fn) - mv.text;   // fn is in .text (checked)
    const int64_t tgt = s2sig::ResolveLeaDisp(mv.text, mv.textSize, fnOff + at,
                                              static_cast<int>(dispOff), static_cast<int>(instrLen));
    if (tgt == s2sig::kFail)
        return Fail(out, cap, "validate.string-xref: displacement read is out of bounds");

    // uintptr arithmetic, and the guard is the FULL mapped extent, not .text: the target is normally
    // BELOW the text base (.rodata precedes .text in the mapping), so `tgt` is legitimately
    // NEGATIVE. A .text-range guard here would reject every valid xref by construction.
    const uintptr_t addr = reinterpret_cast<uintptr_t>(mv.text) + static_cast<uintptr_t>(tgt);
    const uintptr_t lo   = reinterpret_cast<uintptr_t>(mv.lo);
    const uintptr_t hi   = reinterpret_cast<uintptr_t>(mv.hi);
    const std::size_t need = expect.size() + 1;                        // compare INCLUDING the NUL
    if (!lo || !hi || addr < lo || addr >= hi || (hi - addr) < need)
        return Fail(out, cap, "validate.string-xref target is outside the module");

    // Including the NUL is load-bearing: without it "Round" is satisfied by "RoundEnd", which is
    // precisely the near-miss a borrowed signature lands on.
    if (std::memcmp(reinterpret_cast<const void*>(addr), expect.c_str(), need) != 0)
        return Fail(out, cap,
                    "the xref at fn+0x%llx does not reference the '%s' string "
                    "(unique-but-WRONG match — the borrowed-sig trap)",
                    static_cast<unsigned long long>(at), expect.c_str());
    return true;
}

// -----------------------------------------------------------------------------------------------
// vtable-member — the resolved address must be one of the function-pointer slots of the named
// class's PRIMARY vtable, derived by the RTTI walk (string -> type_info -> vtable back-reference).
//
// The generalization of the bespoke gate the respawn slice needed: that signature has no unique log
// string to xref, and uniqueness alone was proven insufficient.
//
// The walk terminates at the first slot value outside the module's .text — the next sub-vtable's
// offset-to-top header — and that termination is FAIL-CLOSED: a walk truncated before it reaches
// `fn` FAILS the gate, it never passes wrongly.
//
// The slot INDEX is deliberately not asserted. Asserting it would reintroduce exactly the borrowed-
// index failure class this validator exists to defeat.
//
// Limit worth recording: GetVTableByName resolves only the PRIMARY vtable, so a virtual inherited
// through a SECONDARY base under multiple inheritance is not in it and would be rejected. Fail-
// closed is the right default; the fix, if the treadmill ever hits it, is a vtable.cpp capability
// (enumerate all of a class's vtables), not a gamedata one.
// -----------------------------------------------------------------------------------------------
bool ValidateVtableMember(const json& v, const ModuleView& mv, const char* module, const void* fn,
                          const Ops& ops, char* out, int cap) {
    if (!v.is_string() || v.get<std::string>().empty())
        return Fail(out, cap, "malformed validate.vtable-member (expected a class name string)");
    const std::string cls = v.get<std::string>();
    if (cls.find('\0') != std::string::npos)
        return Fail(out, cap, "validate.vtable-member class name contains an interior NUL");
    if (!ops.vtable_by_name)
        return Fail(out, cap, "validate.vtable-member: RTTI vtable resolution is unavailable");

    // The module is NOT a parameter of the validator: it is the descriptor's own module, the same
    // one the resolve step scanned. One fewer thing to get out of sync, and strictly more general
    // than the hardcoded soname this replaces.
    void** vt = ops.vtable_by_name(module, cls.c_str());
    if (!vt) return Fail(out, cap, "class RTTI vtable '%s' not found on this build", cls.c_str());

    for (int i = 0; i < kMaxVtableSlots; i++) {
        const void* p = vt[i];
        if (!InText(mv, p)) break;   // sub-vtable offset-to-top header = end of the fn slots
        if (p == fn) return true;
    }
    return Fail(out, cap,
                "sig-resolved address is NOT a member of the RTTI-derived %s primary vtable "
                "(unique-but-WRONG match — the borrowed-sig trap)",
                cls.c_str());
}

}  // namespace

const char* VocabularyList() {
    // Built once from kVocabulary so the reason line can never disagree with the dispatcher.
    static const std::string s = [] {
        std::string acc;
        for (int i = 0; i < kVocabularyCount; i++) {
            if (i) acc += ", ";
            acc += kVocabulary[i];
        }
        return acc;
    }();
    return s.c_str();
}

bool DeclaresPrologue(const char* validateJson) {
    json v;
    if (!ParseValidate(validateJson, &v)) return false;
    auto it = v.find("prologue");
    return it != v.end() && it->is_string() && !it->get<std::string>().empty();
}

bool Run(const char* validateJson, const ModuleView& mv, const char* module, const void* fn,
         const Ops& ops, char* reasonOut, int reasonCap) {
    if (reasonOut && reasonCap > 0) reasonOut[0] = '\0';

    json v;
    if (!ParseValidate(validateJson, &v))
        return Fail(reasonOut, reasonCap, "malformed validate (expected a JSON object of validators)");
    if (v.empty()) return true;   // no validators declared

    // Belt-and-braces: the caller range-checks `fn` into .text first (the two checks are different
    // treadmill signals), but every validator below computes from `fn` by subtraction or compares
    // against it, so an unprovenanced address must never reach them.
    if (!InText(mv, fn))
        return Fail(reasonOut, reasonCap, "validator input: address is outside the module's .text");

    // Vocabulary closure FIRST, over the whole object: an unknown key must fail even when a known
    // sibling would have failed anyway, so a typo is never masked by another validator's reason.
    for (auto it = v.begin(); it != v.end(); ++it) {
        bool known = false;
        for (int i = 0; i < kVocabularyCount; i++) {
            if (it.key() == kVocabulary[i]) { known = true; break; }
        }
        if (!known)
            return Fail(reasonOut, reasonCap, "unknown validator '%s' (valid: %s)",
                        it.key().c_str(), VocabularyList());
    }

    // Evaluate in vocabulary order, not JSON order: all validators are AND-ed and the FIRST failure
    // wins the reason, so the reported reason must be deterministic and must be the cheapest,
    // most informative signal — never whichever key the author happened to type first.
    for (int i = 0; i < kVocabularyCount; i++) {
        const char* name = kVocabulary[i];
        auto it = v.find(name);
        if (it == v.end()) continue;
        bool ok;
        // Dispatch on the NAME, not the array index, so reordering the vocabulary cannot silently
        // re-point a validator. The final `else` is closure in the other direction: a name added to
        // kVocabulary with no implementation degrades by name instead of silently PASSING, which
        // would be the same silent-drop hazard as an unknown key being ignored.
        if      (std::strcmp(name, "prologue") == 0)      ok = ValidatePrologue(*it, mv, fn, reasonOut, reasonCap);
        else if (std::strcmp(name, "string-xref") == 0)   ok = ValidateStringXref(*it, mv, fn, reasonOut, reasonCap);
        else if (std::strcmp(name, "vtable-member") == 0) ok = ValidateVtableMember(*it, mv, module, fn, ops, reasonOut, reasonCap);
        else ok = Fail(reasonOut, reasonCap,
                       "validator '%s' is in the vocabulary but has no implementation", name);
        if (!ok) return false;
    }
    return true;
}

// ---------------------------------------------------------------------------
// arg-width. See call_validate.h for why this is NOT in kVocabulary and why only narrowing fails.
// ---------------------------------------------------------------------------
namespace {

// How far into the prologue to look. Short ON PURPOSE: an argument spilled far from entry is not
// prologue evidence, and a long scan is where a redefinition gets missed and a false FAIL is born.
constexpr int         kScanBytes = 64;
constexpr int         kScanInsns = 24;
constexpr std::size_t kMaxInsn   = 15;   // longest x86-64 instruction
// hde64's prefix loop runs 16 iterations BEFORE the opcode, so a maximal run of legacy prefixes can
// push its reads 16 bytes past where the instruction "should" start. A kMaxInsn-sized window is
// therefore one short of its worst case — 15 x 0x66 walks off the end (ASan-confirmed). Size the
// window for what the DISASSEMBLER can touch, not for what an instruction can legally be.
constexpr std::size_t kWindow    = 16 + kMaxInsn;

int ArgOfReg(const s2abi::RegisterMap& map, int reg) {
    return reg >= 0 && reg < 16 ? map.slotOfGpr[reg] : -1;
}

}  // namespace

ArgUse DecodeArgUse(const uint8_t* p, std::size_t avail,
                    const s2abi::RegisterMap& registerMap) {
    ArgUse u;
    if (!p || avail == 0) return u;

    // hde64_disasm reads up to kMaxInsn bytes regardless of what is actually there, so a decode at
    // the very end of the view would read past it. Copy into a zero-padded window instead of
    // trusting the disassembler to stop — the same reason every other validator here bounds first.
    uint8_t win[kWindow] = {};
    const std::size_t n = avail < kMaxInsn ? avail : kMaxInsn;
    std::memcpy(win, p, n);

    hde64s hs;
    const unsigned len = hde64_disasm(win, &hs);
    if (len == 0 || (hs.flags & F_ERROR) || len > n) return u;   // len > n: it decoded padding
    u.length = len;

    const int reg = hs.modrm_reg | (hs.rex_r ? 8 : 0);
    const int rm  = hs.modrm_rm  | (hs.rex_b ? 8 : 0);

    // TERMINATORS. The scan must stop at the end of the callee's own flow, and this is the single
    // highest-value rule in the file: without it the walk marches into the NEXT function and blames
    // its argument spills on our shape (a corpus scan over libserver.so attributed 490 of 3497
    // refusals to stores past the symbol's own size — every one a false refusal of a good hook).
    //
    // `call` is a terminator for the same reason and one more: incoming volatile argument registers
    // are no longer provenance-safe after another call, so a later spill is not evidence about the
    // original argument. CCSPlayerController_Respawn's real prologue calls out at +0xa.
    if (hs.opcode == 0xC3 || hs.opcode == 0xC2 ||                       // ret
        hs.opcode == 0xE9 || hs.opcode == 0xEB ||                       // jmp rel
        hs.opcode == 0xE8 ||                                            // call rel
        (hs.opcode >= 0x70 && hs.opcode <= 0x7F) ||                     // jcc rel8
        (hs.opcode == 0x0F && hs.opcode2 >= 0x80 && hs.opcode2 <= 0x8F) // jcc rel32
       ) { u.terminator = true; return u; }
    if (hs.opcode == 0xFF && (hs.modrm_reg == 2 || hs.modrm_reg == 4)) { // call/jmp r/m
        u.terminator = true; return u;
    }

    if (hs.opcode == 0x89) {                 // MOV r/m, r
        if (hs.modrm_mod != 3) {             // ...to MEMORY: the callee is spilling an argument
            u.argIndex = ArgOfReg(registerMap, reg);
            u.wide     = hs.rex_w != 0;
        } else {                             // ...to a REGISTER: that register is now redefined
            u.redefines = ArgOfReg(registerMap, rm);
        }
    } else if (hs.opcode == 0x8B) {          // MOV r, r/m  -> reg is redefined
        u.redefines = ArgOfReg(registerMap, reg);
    } else if (hs.opcode >= 0xB8 && hs.opcode <= 0xBF) {   // MOV imm32/64 -> reg
        u.redefines = ArgOfReg(registerMap,
                              static_cast<int>(hs.opcode - 0xB8) | (hs.rex_b ? 8 : 0));
    } else if (hs.opcode == 0xC7 && hs.modrm_mod == 3) {   // MOV imm32 -> r/m (sign-extended)
        // The SIBLING of the 0xB8 form above. `mov $1,%edx` (0xBA) was tracked and `mov $1,%rdx`
        // (0x48 C7 C2) was not — same semantics, opposite verdict, purely by encoding.
        u.redefines = ArgOfReg(registerMap, rm);
    } else if (hs.opcode == 0x63 || hs.opcode == 0x8D ||   // movslq / lea      -> reg
               hs.opcode == 0x03 || hs.opcode == 0x0B ||   // add / or   (r <- r/m)
               hs.opcode == 0x23 || hs.opcode == 0x2B ||   // and / sub  (r <- r/m)
               hs.opcode == 0x33) {                        // xor        (r <- r/m)
        // movslq is the CANONICAL encoding of "a 32-bit int argument widened for use as an index" —
        // the shape is right and the hook was being refused. lea and the r<-r/m ALU forms are the
        // rest of the common "compute into an argument register then spill it" shapes.
        u.redefines = ArgOfReg(registerMap, reg);
    } else if (hs.opcode == 0x01 || hs.opcode == 0x09 ||   // add / or   (r/m <- r)
               hs.opcode == 0x21 || hs.opcode == 0x29 ||   // and / sub  (r/m <- r)
               hs.opcode == 0x31) {                        // xor        (r/m <- r)
        // DIRECTION MATTERS. These write r/m and READ reg — the mirror of the group above. Crediting
        // `reg` here (as this did) marks the SOURCE as redefined and leaves the real destination
        // live, which silently weakens every later observation. It hid in the suite because the one
        // test covering it used `xor %edx,%edx`, the degenerate encoding where reg == rm and the
        // direction cannot show.
        if (hs.modrm_mod == 3)
            u.redefines = ArgOfReg(registerMap, rm);   // memory destination kills no register
    } else if ((hs.opcode == 0x0F && (hs.opcode2 == 0xB6 || hs.opcode2 == 0xB7 ||
                                      hs.opcode2 == 0xBE || hs.opcode2 == 0xBF))) {
        // movzx/movsx — a PURE KILL: it fully defines `reg` without reading it. Untracked, a later
        // spill of that register was counted as an incoming argument and refused a correct shape.
        // This is the only confirmed false-refusal source found on the real binary (~0.03%).
        u.redefines = ArgOfReg(registerMap, reg);
    } else if (hs.opcode == 0x99) {                        // cqo/cdq — sign-extends rax INTO rdx
        u.redefines = ArgOfReg(registerMap, 2);                         // rdx
    } else if (hs.opcode == 0xF7 && (hs.modrm_reg == 4 || hs.modrm_reg == 6 || hs.modrm_reg == 7)) {
        u.redefines = ArgOfReg(registerMap, 2);            // mul/div/idiv clobber rdx:rax
    } else if (hs.opcode == 0x0F && hs.opcode2 == 0x7E && hs.rex_w) {
        u.redefines = ArgOfReg(registerMap, rm);           // movq %xmm,%r64 — dest is r/m
    } else if (hs.opcode >= 0x58 && hs.opcode <= 0x5F) {   // pop -> reg
        u.redefines = ArgOfReg(registerMap,
                              static_cast<int>(hs.opcode - 0x58) | (hs.rex_b ? 8 : 0));
    }
    // Anything else records no redefinition. That is UNSOUND IN THE SAFE DIRECTION: a missed
    // redefinition can only produce a false FAIL (we mistake a later value for the argument), never
    // a false pass. kScanBytes bounds how much rope that has.
    return u;
}

int ArgWidths(const uint8_t* wide, int count, const s2abi::RegisterMap& registerMap,
              const ModuleView& mv, const void* fn,
              char* reasonOut, int reasonCap) {
    if (reasonOut && reasonCap > 0) reasonOut[0] = '\0';
    if (!wide || count <= 0 || !fn || !mv.text) {
        if (reasonOut && reasonCap > 0)
            std::snprintf(reasonOut, static_cast<std::size_t>(reasonCap), "arg-width: nothing to check");
        return 0;
    }

    const uint8_t* const code = static_cast<const uint8_t*>(fn);
    const uint8_t* const end  = mv.text + mv.textSize;
    if (code < mv.text || code >= end) {
        if (reasonOut && reasonCap > 0)
            std::snprintf(reasonOut, static_cast<std::size_t>(reasonCap),
                          "arg-width: address outside .text (not checked)");
        return 0;   // the caller's own range check owns this failure; do not double-report it
    }

    bool redefined[8] = { false };
    int  observed     = 0;
    std::size_t off   = 0;

    for (int i = 0; i < kScanInsns && off < static_cast<std::size_t>(kScanBytes); i++) {
        const std::size_t avail = static_cast<std::size_t>(end - (code + off));
        if (avail == 0) break;
        const ArgUse u = DecodeArgUse(code + off, avail, registerMap);
        if (u.length == 0) break;   // undecodable: stop, do NOT guess
        if (u.terminator) break;    // end of the callee's own flow (or a call that clobbers args)

        if (u.argIndex >= 0 && u.argIndex < 8 && !redefined[u.argIndex]) {
            observed++;

            // TOO FEW ARGUMENTS — the same failure as a narrowed one, by a different route.
            //
            // The callee is spilling an incoming argument in a slot this shape does not declare. It
            // cannot be scratch: using a register as scratch means WRITING it first, which is a
            // redefinition, and the guard above already excludes those. A compiler does not spill a
            // register it never defined unless the caller supplied it.
            //
            // Why it crashes: our thunk names only the registers the shape declares, but dispatching
            // into JS clobbers the rest. We then call the original, which reads them as if the engine
            // had set them — garbage in a register the callee may treat as a pointer. Identical
            // outcome to the truncation, and the same one-sided rule: declaring FEWER arguments than
            // the callee takes is dangerous, declaring more is harmless (the callee ignores extras).
            if (u.argIndex >= count) {
                if (reasonOut && reasonCap > 0)
                    std::snprintf(reasonOut, static_cast<std::size_t>(reasonCap),
                                  "shape declares %d integer arg slot(s), but the callee reads slot %d "
                                  "at +0x%zx — an argument the thunk never relays is read by the "
                                  "original from a register our dispatch has already clobbered",
                                  count, u.argIndex, off);
                return -1;
            }

            // `u.argIndex < count` is redundant while the refusal above returns — and that is
            // exactly why it is written out. The bound must not depend on an unrelated policy rule
            // staying a hard refusal: downgrade that to a warning and this read goes out of bounds
            // (ASan-confirmed). One comparison to make the memory safety independent of the policy.
            if (u.argIndex < count && u.wide && wide[u.argIndex] == 0) {
                if (reasonOut && reasonCap > 0)
                    std::snprintf(reasonOut, static_cast<std::size_t>(reasonCap),
                                  "shape relays integer arg slot %d 32-bit, but the callee stores it "
                                  "64-bit at +0x%zx — a pointer relayed through a 32-bit slot is "
                                  "truncated", u.argIndex, off);
                return -1;
            }
        }
        if (u.redefines >= 0 && u.redefines < 8) redefined[u.redefines] = true;
        off += u.length;
    }

    if (reasonOut && reasonCap > 0)
        std::snprintf(reasonOut, static_cast<std::size_t>(reasonCap),
                      observed ? "arg-width: %d argument store(s) checked, no narrowing found"
                               : "arg-width: no argument stores in the prologue window (not proof)",
                      observed);
    return 0;
}

}  // namespace s2validate
