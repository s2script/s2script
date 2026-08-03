#include "detour_reloc.h"

#include <cstring>

extern "C" {
#include "hde64.h"
}

namespace s2detour {
namespace {

bool Fits32(int64_t v) { return v >= INT32_MIN && v <= INT32_MAX; }

// Where the operands sit inside an encoded instruction. HDE64 hands us the decoded VALUES
// (hs.disp.disp32, hs.imm.imm32) but — unlike Zydis, which safetyhook uses — NOT their byte offsets,
// so we derive them. Verified against third_party/hde/hde64.c: the ModRM block consumes its
// displacement BEFORE the immediate block runs, and a rel32 is always the last four bytes.
//
// Getting this wrong writes four bytes into the middle of an instruction that still decodes and
// still runs. It is the highest-risk arithmetic in the file, which is why detour_reloc_test.cpp
// asserts the emitted bytes exactly, including for an instruction carrying BOTH a disp32 and an
// imm32 (`cmpl $imm,[rip+d]`) — the case that breaks if ImmBytes is ignored.
int ImmBytes(uint32_t flags) {
    int n = 0;
    if (flags & F_IMM8)  n += 1;
    if (flags & F_IMM16) n += 2;
    if (flags & F_IMM32) n += 4;
    if (flags & F_IMM64) n += 8;
    return n;
}

// hde64_disasm may read up to kMaxInsnLen bytes from `at`. Prove the whole range first: probing only
// `at` is what let the old caller-side guard (engine_hooks.cpp's kPatchWindow) under-cover the read
// by up to 14 bytes.
//
// This is deliberately strict rather than page-aware. S2_AddressIsExecutable answers "inside some
// PT_LOAD PF_X segment", and a segment can end mid-page, so page-rounding would ask it a question it
// does not answer. The cost is refusing a function whose prologue begins within 15 bytes of the end
// of an executable segment — not a real function, and a named refusal if it ever is one.
bool ProbeReadable(uintptr_t at, int (*isExec)(const void*)) {
    if (!isExec) return true;  // test-only: the caller supplied its own buffer
    return isExec(reinterpret_cast<const void*>(at)) &&
           isExec(reinterpret_cast<const void*>(at + kMaxInsnLen - 1));
}

}  // namespace

BuildResult BuildTrampolineBody(const uint8_t* code, uintptr_t codeAddr, int minBytes,
                                uint8_t* out, uintptr_t outAddr,
                                int (*isExecutable)(const void*)) {
    BuildResult r;
    int steal = 0, emitted = 0;

    while (steal < minBytes) {
        const uintptr_t insnAddr = codeAddr + static_cast<uintptr_t>(steal);

        if (!ProbeReadable(insnAddr, isExecutable)) {
            r.reason = "prologue leaves the target's executable range (stale gamedata?)";
            return r;
        }

        hde64s hs;
        const unsigned int len = hde64_disasm(code + steal, &hs);
        if (len == 0 || (hs.flags & F_ERROR)) {
            r.reason = "undecodable instruction in prologue";
            return r;
        }
        if (steal + static_cast<int>(len) > kMaxSteal) {
            r.reason = "prologue exceeds the maximum stealable length";
            return r;
        }

        const uint8_t* src     = code + steal;
        uint8_t*       dst     = out + emitted;
        const uintptr_t dstAddr = outAddr + static_cast<uintptr_t>(emitted);
        std::memcpy(dst, src, len);

        // (1) A RIP-relative memory operand: ModRM mod=00, r/m=101 with a disp32. In 64-bit mode
        //     that encoding means [rip+disp32] (r/m=100 would be a SIB instead), and HDE does NOT
        //     set F_RELATIVE for it — F_RELATIVE is only ever a branch immediate — so it needs its
        //     own test. Copied blindly, this silently reads or writes the wrong global.
        if ((hs.flags & F_MODRM) && (hs.flags & F_DISP32) && hs.modrm_mod == 0 && hs.modrm_rm == 5) {
            const int off = static_cast<int>(len) - ImmBytes(hs.flags) - 4;
            if (off < 0) {
                r.reason = "rip-relative displacement does not fit the decoded instruction";
                return r;
            }
            const int64_t absTarget = static_cast<int64_t>(insnAddr) + len +
                                      static_cast<int32_t>(hs.disp.disp32);
            const int64_t newDisp = absTarget - static_cast<int64_t>(dstAddr + len);
            if (!Fits32(newDisp)) {
                r.reason = "rip-relative target out of reach from the trampoline";
                return r;
            }
            const int32_t v = static_cast<int32_t>(newDisp);
            std::memcpy(dst + off, &v, 4);
        }

        // (2) A relative branch immediate. Only the rel32 forms relocate in place; rel8/rel16 would
        //     have to be WIDENED, which changes the copied instruction's size and therefore every
        //     subsequent offset. safetyhook widens; we refuse by name instead, because no s2script
        //     target has ever had a short branch in its prologue and a named refusal turns a future
        //     one into a precise report rather than a mystery. Adding widening later is contained.
        if (hs.flags & F_RELATIVE) {
            const bool rel32 = (hs.flags & F_IMM32) &&
                               (hs.opcode == 0xE8 ||   // call rel32
                                hs.opcode == 0xE9 ||   // jmp  rel32
                                (hs.opcode == 0x0F && hs.opcode2 >= 0x80 && hs.opcode2 <= 0x8F));
            if (!rel32) {
                r.reason = "short branch in stolen prologue (not widened)";
                return r;
            }
            const int off = static_cast<int>(len) - 4;   // a rel32 is always the last four bytes
            const int64_t absTarget = static_cast<int64_t>(insnAddr) + len +
                                      static_cast<int32_t>(hs.imm.imm32);
            const int64_t newRel = absTarget - static_cast<int64_t>(dstAddr + len);
            if (!Fits32(newRel)) {
                r.reason = "branch target out of reach from the trampoline";
                return r;
            }
            const int32_t v = static_cast<int32_t>(newRel);
            std::memcpy(dst + off, &v, 4);
        }

        steal   += static_cast<int>(len);
        emitted += static_cast<int>(len);
    }

    r.ok      = true;
    r.steal   = steal;
    r.emitted = emitted;
    return r;
}

void WriteAbsJmp(uint8_t* at, uintptr_t dest) {
    at[0] = 0xFF;
    at[1] = 0x25;
    const int32_t zero = 0;                  // rip-relative disp32 = 0 -> the .quad immediately follows
    std::memcpy(at + 2, &zero, 4);
    std::memcpy(at + 6, &dest, 8);
}

bool WriteRelJmp(uint8_t* at, uintptr_t atAddr, uintptr_t dest) {
    const int64_t rel = static_cast<int64_t>(dest) - static_cast<int64_t>(atAddr + kJmpRel32);
    if (!Fits32(rel)) return false;
    at[0] = 0xE9;
    const int32_t v = static_cast<int32_t>(rel);
    std::memcpy(at + 1, &v, 4);
    return true;
}

}  // namespace s2detour
