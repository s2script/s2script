#include "detour.h"

#include <sys/mman.h>
#include <unistd.h>
#include <cstdint>
#include <cstring>
#include <vector>

extern "C" {
#include "hde64.h"
}

namespace s2detour {

// A 64-bit absolute jump: FF 25 00000000 <8-byte target>  ( jmp qword ptr [rip+0] ; .quad target ).
static const int kAbsJmp = 14;

struct Patch {
    uint8_t* target;
    uint8_t  orig[32];
    int      origLen;
    void*    trampoline;
    size_t   trampSize;
};

static std::vector<Patch> g_patches;

static void WriteAbsJmp(uint8_t* at, const void* dest) {
    at[0] = 0xFF;
    at[1] = 0x25;
    *reinterpret_cast<int32_t*>(at + 2) = 0;  // rip-relative disp32 = 0 -> the .quad immediately follows
    *reinterpret_cast<uint64_t*>(at + 6) = reinterpret_cast<uint64_t>(dest);
}

static bool Protect(void* addr, size_t len, int prot) {
    long pg = sysconf(_SC_PAGESIZE);
    uintptr_t a = reinterpret_cast<uintptr_t>(addr);
    uintptr_t start = a & ~(uintptr_t)(pg - 1);
    uintptr_t end = (a + len + pg - 1) & ~(uintptr_t)(pg - 1);
    return mprotect(reinterpret_cast<void*>(start), end - start, prot) == 0;
}

bool Install(void* target, void* handler, void** origTrampoline) {
    uint8_t* code = reinterpret_cast<uint8_t*>(target);

    // 1. Sum whole-instruction lengths until we have room for the 14-byte absolute jump.
    //    Bail on any relative/rip-relative or undecodable instruction — we can only blindly copy
    //    position-independent bytes into the trampoline.
    int steal = 0;
    while (steal < kAbsJmp) {
        hde64s hs;
        unsigned int len = hde64_disasm(code + steal, &hs);
        if (len == 0 || (hs.flags & (F_ERROR | F_RELATIVE))) return false;
        // Reject rip-relative operands (mod=00, r/m=101 with a disp32): can't relocate blindly.
        if ((hs.flags & F_MODRM) && (hs.flags & F_DISP32) && (hs.modrm_mod == 0) && (hs.modrm_rm == 5)) return false;
        steal += static_cast<int>(len);
        if (steal > static_cast<int>(sizeof(((Patch*)0)->orig))) return false;
    }

    // 2. Trampoline: [relocated stolen prologue] + [absolute jump back to target+steal].
    size_t trampSize = static_cast<size_t>(steal) + kAbsJmp;
    void* tramp = mmap(nullptr, trampSize, PROT_READ | PROT_WRITE | PROT_EXEC,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (tramp == MAP_FAILED) return false;
    std::memcpy(tramp, code, static_cast<size_t>(steal));
    WriteAbsJmp(reinterpret_cast<uint8_t*>(tramp) + steal, code + steal);

    // 3. Save original bytes, then overwrite the prologue with the jump to the handler.
    Patch p{};
    p.target = code;
    p.origLen = steal;
    std::memcpy(p.orig, code, static_cast<size_t>(steal));
    p.trampoline = tramp;
    p.trampSize = trampSize;

    if (!Protect(code, static_cast<size_t>(steal), PROT_READ | PROT_WRITE | PROT_EXEC)) {
        munmap(tramp, trampSize);
        return false;
    }
    WriteAbsJmp(code, handler);
    for (int i = kAbsJmp; i < steal; ++i) code[i] = 0x90;  // NOP the tail (never executed; keeps disasm sane)
    Protect(code, static_cast<size_t>(steal), PROT_READ | PROT_EXEC);

    g_patches.push_back(p);
    *origTrampoline = tramp;
    return true;
}

void RemoveAll() {
    for (auto& p : g_patches) {
        if (Protect(p.target, static_cast<size_t>(p.origLen), PROT_READ | PROT_WRITE | PROT_EXEC)) {
            std::memcpy(p.target, p.orig, static_cast<size_t>(p.origLen));
            Protect(p.target, static_cast<size_t>(p.origLen), PROT_READ | PROT_EXEC);
        }
        munmap(p.trampoline, p.trampSize);
    }
    g_patches.clear();
}

}  // namespace s2detour
