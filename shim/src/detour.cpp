#include "detour.h"
#include "detour_reloc.h"

#include <sys/mman.h>
#include <unistd.h>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <vector>

namespace s2detour {

namespace {

struct Patch {
    uint8_t* target;
    uint8_t  orig[kMaxSteal];
    int      origLen;
    void*    trampoline;   // the page base — also what origTrampoline points at
    size_t   trampSize;
};

std::vector<Patch> g_patches;

// Set once, when the kernel proves it does not honour MAP_FIXED_NOREPLACE (pre-4.17, where the flag
// is silently treated as a plain hint). Latched rather than re-probed: the answer cannot change
// within a process, and re-probing means another speculative mapping every call.
bool g_noFixedNoReplace = false;

// See SetForceFarTierForTest. Always false in a shipped process.
bool g_forceFarTier = false;

bool Protect(void* addr, size_t len, int prot) {
    const long pg = sysconf(_SC_PAGESIZE);
    const uintptr_t a = reinterpret_cast<uintptr_t>(addr);
    const uintptr_t start = a & ~static_cast<uintptr_t>(pg - 1);
    const uintptr_t end = (a + len + static_cast<uintptr_t>(pg) - 1) & ~static_cast<uintptr_t>(pg - 1);
    return mprotect(reinterpret_cast<void*>(start), end - start, prot) == 0;
}

// Map exactly at `addr` or not at all. MAP_FIXED_NOREPLACE is what makes a /proc/self/maps scan
// unnecessary: another thread mapping into our chosen gap makes this fail with EEXIST rather than
// silently clobbering it, and we just try the next candidate.
void* TryMapAt(uintptr_t addr, size_t size) {
    const int prot = PROT_READ | PROT_WRITE | PROT_EXEC;
#ifdef MAP_FIXED_NOREPLACE
    if (!g_noFixedNoReplace) {
        void* p = mmap(reinterpret_cast<void*>(addr), size, prot,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        if (p != MAP_FAILED) {
            if (reinterpret_cast<uintptr_t>(p) == addr) return p;
            munmap(p, size);              // honoured as a plain hint => flag unimplemented; latch it
            g_noFixedNoReplace = true;
            return nullptr;
        }
        if (errno == EINVAL) g_noFixedNoReplace = true;   // flag unknown to this kernel
        return nullptr;
    }
#endif
    void* p = mmap(reinterpret_cast<void*>(addr), size, prot, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    return p == MAP_FAILED ? nullptr : p;                 // caller range-checks what it got
}

// A page within rel32 reach of `target`, or null. Probing candidates outward beats parsing
// /proc/self/maps: the kernel is the authority on what is free, the answer cannot go stale between
// the read and the map, and there is no parser to get wrong.
void* AllocNear(uintptr_t target, size_t size) {
    if (g_forceFarTier) return nullptr;
    const uintptr_t pg = static_cast<uintptr_t>(sysconf(_SC_PAGESIZE));
    const uintptr_t base = target & ~(pg - 1);
    const int64_t   kReach = 0x7FFF0000LL;   // just under 2GB, with room for the encodings
    const uintptr_t kStep  = 1u << 20;       // 1MB — inter-library gaps are far larger than this

    for (uintptr_t d = kStep; d < static_cast<uintptr_t>(kReach); d += kStep) {
        for (int above = 0; above < 2; above++) {
            if (!above && d > base) continue;                       // would underflow
            const uintptr_t cand = above ? base + d : base - d;
            if (cand < 0x10000) continue;                           // never the low guard pages
            void* p = TryMapAt(cand, size);
            if (!p) continue;
            const int64_t delta = static_cast<int64_t>(reinterpret_cast<uintptr_t>(p)) -
                                  static_cast<int64_t>(target);
            if (delta > -kReach && delta < kReach) return p;
            // Only reachable on the plain-hint fallback path, where the kernel places the mapping
            // wherever it likes. It will keep doing that, so stop probing rather than spin.
            munmap(p, size);
            return nullptr;
        }
    }
    return nullptr;
}

InstallResult Fail(const char* reason) {
    InstallResult r;
    r.reason = reason;
    return r;
}

}  // namespace

InstallResult Install(void* target, void* handler, void** origTrampoline,
                      int (*isExecutable)(const void*)) {
    if (!target || !handler || !origTrampoline) return Fail("null target, handler, or trampoline out");

    uint8_t* const  code        = reinterpret_cast<uint8_t*>(target);
    const uintptr_t targetAddr  = reinterpret_cast<uintptr_t>(code);
    const uintptr_t handlerAddr = reinterpret_cast<uintptr_t>(handler);
    const size_t    pageSize    = static_cast<size_t>(sysconf(_SC_PAGESIZE));

    // Both tiers take one page. The page holds, in order:
    //
    //     +0            relocated prologue          <- *origTrampoline: the "call original" entry
    //     +emitted      FF 25 <target+steal>        <- ...which falls through to here, and returns
    //     +emitted+14   FF 25 <handler>             <- the NEAR tier's E9 lands here (unused when far)
    //
    // That last stub is why a 5-byte E9 works at all: the handler lives in OUR .so and need not be
    // within 2GB of the game's, but the near page is within 2GB of the target BY CONSTRUCTION, so
    // the E9 reaches the island and the island reaches the handler absolutely. Getting this wrong —
    // pointing the E9 straight at the handler — is a hook that works only when the loader happens to
    // place the two modules close together.
    const size_t kNeed = static_cast<size_t>(kMaxSteal) + kJmpAbs + kJmpAbs;
    if (kNeed > pageSize) return Fail("trampoline layout does not fit one page");

    uint8_t     body[kMaxSteal];
    BuildResult built;
    void*       tramp = nullptr;
    bool        near  = false;

    // ---- Tier 1: a near page + a 5-byte E9. ---------------------------------------------------
    if (void* nearPage = AllocNear(targetAddr, pageSize)) {
        built = BuildTrampolineBody(code, targetAddr, kJmpRel32,
                                    body, reinterpret_cast<uintptr_t>(nearPage), isExecutable);
        if (built.ok) {
            tramp = nearPage;
            near  = true;
        } else {
            munmap(nearPage, pageSize);
            // This refusal is about the INSTRUCTIONS, and tier 2 steals a superset of these bytes,
            // so it cannot do better. Report tier 1's reason rather than a misleading tier-2 one.
            return Fail(built.reason);
        }
    }

    // ---- Tier 2: a page anywhere + a 14-byte absolute jump. -----------------------------------
    if (!tramp) {
        void* p = mmap(nullptr, pageSize, PROT_READ | PROT_WRITE | PROT_EXEC,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) return Fail("trampoline allocation failed");
        built = BuildTrampolineBody(code, targetAddr, kJmpAbs,
                                    body, reinterpret_cast<uintptr_t>(p), isExecutable);
        if (!built.ok) { munmap(p, pageSize); return Fail(built.reason); }
        tramp = p;
    }

    // ---- Lay out the page. --------------------------------------------------------------------
    uint8_t* const page = reinterpret_cast<uint8_t*>(tramp);
    std::memcpy(page, body, static_cast<size_t>(built.emitted));
    WriteAbsJmp(page + built.emitted, targetAddr + static_cast<uintptr_t>(built.steal));
    WriteAbsJmp(page + built.emitted + kJmpAbs, handlerAddr);

    // ---- Patch the target. Nothing above this line has touched it. ----------------------------
    Patch p{};
    p.target     = code;
    p.origLen    = built.steal;
    p.trampoline = tramp;
    p.trampSize  = pageSize;
    std::memcpy(p.orig, code, static_cast<size_t>(built.steal));

    if (!Protect(code, static_cast<size_t>(built.steal), PROT_READ | PROT_WRITE | PROT_EXEC)) {
        munmap(tramp, pageSize);
        return Fail("could not make the patch site writable");
    }

    int patchLen;
    if (near) {
        const uintptr_t island = reinterpret_cast<uintptr_t>(page + built.emitted + kJmpAbs);
        if (!WriteRelJmp(code, targetAddr, island)) {
            // AllocNear proved the page is in range, so this cannot fire — but restore rather than
            // leave a half-written prologue if it ever does.
            Protect(code, static_cast<size_t>(built.steal), PROT_READ | PROT_EXEC);
            munmap(tramp, pageSize);
            return Fail("near trampoline is out of rel32 range (allocator disagreement)");
        }
        patchLen = kJmpRel32;
    } else {
        WriteAbsJmp(code, handlerAddr);
        patchLen = kJmpAbs;
    }
    // NOP the tail. Never executed — control leaves at the jump — but it keeps a disassembler, and
    // anyone reading a crash dump, from seeing half an instruction.
    for (int i = patchLen; i < built.steal; ++i) code[i] = 0x90;
    Protect(code, static_cast<size_t>(built.steal), PROT_READ | PROT_EXEC);

    g_patches.push_back(p);
    *origTrampoline = tramp;

    InstallResult r;
    r.ok           = true;
    r.stolen       = built.steal;
    r.usedNearJump = near;
    return r;
}

void SetForceFarTierForTest(bool on) { g_forceFarTier = on; }

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
