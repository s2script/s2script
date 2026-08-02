#pragma once
#include <cstdint>

// Plugin-declared engine calls (plugin-gamedata slice, Task 6) — the SHIM half of the feature:
// descriptor RESOLUTION (byte signature or RTTI vtable slot, `prologue`-validated and .text-range
// checked) and the INVOKE itself. Both are plain C-ABI so the Rust core can reach them through two
// appended S2EngineOps fields (Task 7); the core owns the per-plugin descriptor table, the
// permission gate, and every marshalling decision — this TU only ever sees opaque strings, integer
// arg slots, and (index, serial) entity pairs. No raw pointer crosses back to the core except a
// packed CEntityHandle for `returns: "entity"`.
//
// Engine-generic: nothing here names a game class, field, or function. The plugin's own gamedata
// supplies every name as a string.

extern "C" {

// Resolve a descriptor against the LIVE binary. Returns a call id >= 0 usable with
// S2_EngineCallInvoke, or -1 with a human-readable reason written into `reasonOut` (NUL-terminated,
// truncated to `reasonCap`) — the core stores that reason verbatim as the descriptor's named
// degrade reason (spec §12). `kind` is "signature" or "vtable"; `module` defaults to the engine
// server module when null/empty; `resolve` accepts the existing gamedata resolver vocabulary
// ("direct" / "ctor-body-xref" / "lea-disp"); `className` + `vtableIndex` are the vtable path.
//
// `validateJson` is the descriptor's WHOLE `validate` object, serialized (""/"null"/"{}" all mean
// "no validators"). It crosses as one opaque JSON string rather than one C-string parameter per
// validator for two reasons: the closed vocabulary then lives in the shim, which is what actually
// dispatches on it — the same place "unknown resolve strategy" is decided — so an unknown key can
// fail BY NAME instead of being silently dropped; and adding a validator stops being an ABI change.
// A vtable target that declares no `prologue` is REJECTED (see the .cpp header comment for why a
// bare index is never trusted).
int S2_EngineCallResolve(const char* kind, const char* module, const char* pattern,
                         const char* resolve, const char* className, int vtableIndex,
                         const char* validateJson, char* reasonOut, int reasonCap);

// Invoke a resolved descriptor on a serial-gated entity receiver.
// gpKind[i]: 0=scalar, 1=entity((uint64)index<<32|serial), 2=string(index into strs),
//            3=vector(index into vecs, 3 floats each)
// retKind: 0=void 1=bool 2=int 3=float 4=entity
// Returns 1 on success (retOut written), 0 on degrade (stale receiver, unresolved `via` sub-object,
// bad arg budget, unknown kind) — never a crash.
int S2_EngineCallInvoke(int callId, int entIndex, int entSerial, int subObjOff,
                        const uint64_t* gp, const unsigned char* gpKind, int gpCount,
                        const double* fp, int fpCount,
                        const char* const* strs, const float* vecs,
                        int retKind, uint64_t* retOut);

}
