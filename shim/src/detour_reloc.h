// The engine-free, allocation-free half of the inline detour: decode a prologue, RELOCATE what we
// steal, and refuse the rest BY NAME.
//
// Split out of detour.cpp for the same reason hook_dispatch.cpp / call_validate.cpp / defer_queue.cpp
// are split out of their engine TUs: everything interesting here is a pure function from bytes to
// bytes, so shim/tests/detour_reloc_test.cpp drives the SHIPPED relocator against hand-authored
// buffers at arbitrary pretend-addresses. A relocator that is only exercised by patching a live
// engine function is a relocator whose failure mode is a crash on a server.
//
// Nothing in here mmaps, mprotects, or includes an engine header.
#pragma once

#include <cstdint>

namespace s2detour {

// The most bytes we will ever steal. The true worst case is kJmpAbs + 15 - 1 = 28 (the widest patch,
// then one maximal-length x86-64 instruction straddling its last byte); 32 rounds it up. This also
// bounds Patch::orig and the trampoline body buffer.
constexpr int kMaxSteal = 32;

// The two patch forms. `E9 rel32` reaches +/-2GB, so it needs a trampoline allocated near the
// target; `FF 25 <disp32=0>; .quad dest` reaches anywhere.
constexpr int kJmpRel32 = 5;
constexpr int kJmpAbs   = 14;

// The longest x86-64 instruction. hde64_disasm may read this many bytes from the address it is
// given, which is why the probe below has to cover them.
constexpr int kMaxInsnLen = 15;

struct BuildResult {
    bool        ok      = false;
    const char* reason  = nullptr;  // static string, set iff !ok — surfaced verbatim to the caller
    int         steal   = 0;        // bytes consumed at the target
    int         emitted = 0;        // bytes written to `out`. Equals `steal`: we never widen (see
                                    // the short-branch refusal), but the two are distinct concepts
                                    // and conflating them is how a widening relocator gets written
                                    // wrong later.
};

// Decode instructions at `code` — which the caller asserts LIVES at `codeAddr`; the two differ in
// tests, and that separation is the entire reason this is testable — until at least `minBytes` are
// covered, relocating each into `out` as if `out` lives at `outAddr`. Writes at most kMaxSteal bytes.
//
// `isExecutable` (may be null ONLY in tests that supply their own buffer) is consulted for every
// byte range this is about to read, BEFORE reading it. That ordering is the point: an unmapped
// address SEGVs inside the length disassembler, before any guard of ours would run.
BuildResult BuildTrampolineBody(const uint8_t* code, uintptr_t codeAddr, int minBytes,
                                uint8_t* out, uintptr_t outAddr,
                                int (*isExecutable)(const void*));

// `FF 25 00000000 <8-byte dest>` — jmp qword ptr [rip+0]; .quad dest. Position-independent, so it
// needs no address of its own. Writes kJmpAbs bytes.
void WriteAbsJmp(uint8_t* at, uintptr_t dest);

// `E9 rel32`. `at` must live at `atAddr`. Returns false without writing if `dest` is out of rel32
// range — the caller must then fall back to WriteAbsJmp. Writes kJmpRel32 bytes.
bool WriteRelJmp(uint8_t* at, uintptr_t atAddr, uintptr_t dest);

}  // namespace s2detour
