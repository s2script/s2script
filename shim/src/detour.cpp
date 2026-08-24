#include "detour.h"
#include "detour_reloc.h"
#include "platform/memory.h"

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

// See SetForceFarTierForTest. Always false in a shipped process.
bool g_forceFarTier = false;

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
    const size_t    pageSize    = s2platform::PageSize();
    if (!pageSize) return Fail("platform page size is unavailable");

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
    if (void* nearPage = g_forceFarTier
            ? nullptr
            : s2platform::AllocateExecutableNear(targetAddr, pageSize)) {
        built = BuildTrampolineBody(code, targetAddr, kJmpRel32,
                                    body, reinterpret_cast<uintptr_t>(nearPage), isExecutable);
        if (built.ok) {
            tramp = nearPage;
            near  = true;
        } else {
            s2platform::FreeExecutable(nearPage, pageSize);
            // This refusal is about the INSTRUCTIONS, and tier 2 steals a superset of these bytes,
            // so it cannot do better. Report tier 1's reason rather than a misleading tier-2 one.
            return Fail(built.reason);
        }
    }

    // ---- Tier 2: a page anywhere + a 14-byte absolute jump. -----------------------------------
    if (!tramp) {
        void* p = s2platform::AllocateExecutable(pageSize);
        if (!p) return Fail("trampoline allocation failed");
        built = BuildTrampolineBody(code, targetAddr, kJmpAbs,
                                    body, reinterpret_cast<uintptr_t>(p), isExecutable);
        if (!built.ok) { s2platform::FreeExecutable(p, pageSize); return Fail(built.reason); }
        tramp = p;
    }

    // ---- Lay out the page. --------------------------------------------------------------------
    uint8_t* const page = reinterpret_cast<uint8_t*>(tramp);
    std::memcpy(page, body, static_cast<size_t>(built.emitted));
    WriteAbsJmp(page + built.emitted, targetAddr + static_cast<uintptr_t>(built.steal));
    WriteAbsJmp(page + built.emitted + kJmpAbs, handlerAddr);
    s2platform::FlushInstructionCache(page, pageSize);

    // ---- Patch the target. Nothing above this line has touched it. ----------------------------
    Patch p{};
    p.target     = code;
    p.origLen    = built.steal;
    p.trampoline = tramp;
    p.trampSize  = pageSize;
    std::memcpy(p.orig, code, static_cast<size_t>(built.steal));

    if (!s2platform::ProtectMemory(code, static_cast<size_t>(built.steal),
                                   s2platform::MemoryProtection::ReadWriteExecute)) {
        s2platform::FreeExecutable(tramp, pageSize);
        return Fail("could not make the patch site writable");
    }

    int patchLen;
    if (near) {
        const uintptr_t island = reinterpret_cast<uintptr_t>(page + built.emitted + kJmpAbs);
        if (!WriteRelJmp(code, targetAddr, island)) {
            // AllocNear proved the page is in range, so this cannot fire — but restore rather than
            // leave a half-written prologue if it ever does.
            s2platform::ProtectMemory(code, static_cast<size_t>(built.steal),
                                      s2platform::MemoryProtection::ReadExecute);
            s2platform::FreeExecutable(tramp, pageSize);
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
    s2platform::FlushInstructionCache(code, static_cast<size_t>(built.steal));
    s2platform::ProtectMemory(code, static_cast<size_t>(built.steal),
                              s2platform::MemoryProtection::ReadExecute);

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
        if (s2platform::ProtectMemory(p.target, static_cast<size_t>(p.origLen),
                                      s2platform::MemoryProtection::ReadWriteExecute)) {
            std::memcpy(p.target, p.orig, static_cast<size_t>(p.origLen));
            s2platform::FlushInstructionCache(p.target, static_cast<size_t>(p.origLen));
            s2platform::ProtectMemory(p.target, static_cast<size_t>(p.origLen),
                                      s2platform::MemoryProtection::ReadExecute);
        }
        s2platform::FreeExecutable(p.trampoline, p.trampSize);
    }
    g_patches.clear();
}

}  // namespace s2detour
