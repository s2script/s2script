// Minimal self-contained x86-64 inline detour (Slice 6.6; two-tier since the relocation slice).
//
// Overwrites a function prologue with a jump to a handler, relocating the stolen prologue into an
// mmap'd trampoline (+ a jump back). Two tiers, cheapest first — the structure SourceMod gets from
// safetyhook, which its CDetour delegates to wholesale:
//
//   1. NEAR: a 5-byte `E9 rel32`, with the trampoline allocated within +/-2GB so the rel32 reaches.
//      Steals 5 bytes instead of 14, which for every CS2 function we target means stealing exactly
//      `push rbp; mov rbp,rsp; push <r>` — 6 position-independent bytes, nothing to relocate.
//   2. FAR: a 14-byte absolute `FF 25` jump, trampoline anywhere. Used only when no page is free
//      within reach.
//
// Either tier relocates what it steals (rip-relative operands and rel32 branches) and refuses the
// rest BY NAME rather than corrupting it. The byte-level half lives in detour_reloc.{h,cpp} so it
// can be tested without patching a live engine function.
//
// Allocation, page protection, and instruction-cache synchronization live behind the platform
// backend; decoding and relocation remain shared.
#pragma once

#include <cstdint>

namespace s2detour {

struct InstallResult {
    bool        ok           = false;
    const char* reason       = nullptr;  // static string, set iff !ok — surface it, don't invent one
    int         stolen       = 0;        // bytes taken from the target
    bool        usedNearJump = false;    // true = 5-byte E9 tier, false = 14-byte FF tier
};

// Install an inline detour at `target` jumping to `handler`. On success `*origTrampoline` receives a
// callable pointer that runs the ORIGINAL function (relocated prologue + jump back).
//
// `isExecutable` is probed for every byte range the length disassembler is about to read, BEFORE it
// reads it — pass S2_AddressIsExecutable. A stale gamedata offset that resolves to a mapped-but-wrong
// address must be refused, and one that resolves to an unmapped address must not SEGV inside the
// disassembler. May be null ONLY in tests that supply their own buffer.
//
// On failure NO memory is touched and `reason` says which of the refusals it was. Safe to leave
// uninstalled: the caller degrades to "no hook".
InstallResult Install(void* target, void* handler, void** origTrampoline,
                      int (*isExecutable)(const void*));

// Restore every installed detour's original bytes and free its trampoline. Call on plugin Unload.
void RemoveAll();

// TEST ONLY: make near allocation fail, forcing the far tier. On a normally-loaded process the near
// tier always succeeds, so without this the 14-byte fallback — the path that runs precisely when
// address space is tight, i.e. when things are already going badly — would ship never having been
// executed once. An untested fallback is not a fallback.
void SetForceFarTierForTest(bool on);

}  // namespace s2detour
