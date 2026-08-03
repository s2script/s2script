// THE CLOSED VALIDATOR VOCABULARY for a gamedata `calls` descriptor — the engine-free half.
//
// WHY THIS IS ITS OWN TU. `engine_calls.cpp` cannot be compiled outside the game: it includes
// entity2/entitysystem.h and links against the entity-system bridge. A validator that only ever ran
// there would be untestable, and a validator that cannot fail in a test is decoration — exactly the
// hole `shim/src/defer_queue.cpp` was extracted to close for the deferred-dispatch drain. So the
// POLICY lives here with every outside contact INJECTED (the module's own mapped bytes as a
// ModuleView, RTTI vtable resolution as an `Ops` function pointer), and `shim/tests/
// call_validate_test.cpp` drives the SHIPPED code over a synthetic module image.
//
// WHAT A VALIDATOR IS FOR (spec §3 of the plugin-gamedata design, §9.1 of the gamedata-tiering
// design). A resolved address being inside the module's `.text` is NECESSARY and provably
// INSUFFICIENT: a borrowed signature can match UNIQUELY at the WRONG in-range function, and a
// borrowed vtable index can land on a real thunk. Both failures are silent — the call misbehaves
// instead of crashing. A validator is the semantic gate that turns "unique but wrong" into a named,
// per-descriptor degrade.
//
// THE VOCABULARY IS CLOSED. An unknown key under `validate` is a FAILURE naming the valid set, never
// a silent skip: a typo (`vtable_member`) would otherwise drop the exact gate the descriptor exists
// to carry, which is the same class of bug as shipping no validator at all.
//
// ENGINE-GENERIC: nothing here names a game, a class, or a function. Every name arrives as an opaque
// string out of gamedata.
#ifndef S2SCRIPT_CALL_VALIDATE_H
#define S2SCRIPT_CALL_VALIDATE_H

#include <cstddef>
#include <cstdint>

namespace s2validate {

// The two views of the winning module a validator needs:
//   text/textSize — its executable segment, where a resolved function must live and where the
//                   vtable walk's termination test is decided;
//   lo/hi         — its FULL mapped LOAD extent, because a rip-relative string target legitimately
//                   sits BELOW the text base (.rodata precedes .text in the mapping). Range-guarding
//                   an xref target against `text` alone would reject every valid one by
//                   construction.
struct ModuleView {
    const uint8_t* text     = nullptr;
    std::size_t    textSize = 0;
    const uint8_t* lo       = nullptr;
    const uint8_t* hi       = nullptr;
};

// Injected: resolve `className`'s PRIMARY vtable inside `module` (the RTTI string -> type_info ->
// vtable walk in vtable.cpp). Null = the class is not resolvable on this build; the caller degrades.
using VtableByNameFn = void** (*)(const char* module, const char* className);

struct Ops {
    VtableByNameFn vtable_by_name = nullptr;
};

// The closed vocabulary, as the reason strings print it ("prologue, string-xref, vtable-member").
const char* VocabularyList();

// Does `validateJson` declare a non-empty `prologue`? A `vtable` target REQUIRES one (a bare
// borrowed index is never trusted) — a rule about which validator must be PRESENT, deliberately
// separate from how validators are evaluated.
bool DeclaresPrologue(const char* validateJson);

// Run EVERY validator declared in `validateJson` against `fn`, AND-ed; the first failure wins the
// reason. Evaluation order is cheapest-and-most-local first (prologue, string-xref, vtable-member)
// so the reported reason is the most informative cheap signal, not whichever key JSON happened to
// order first.
//
// `validateJson` may be null, "", "null" or "{}" — all mean "no validators declared", which passes.
// `fn` MUST already have been range-checked into the module's `.text` by the caller (the two checks
// are different treadmill signals and must stay distinguishable: "outside .text" means the pattern
// computed off the map, a validator failure means it computed a real in-range function and it is the
// WRONG one). This function re-checks defensively because it computes offsets by subtraction.
//
// Returns true on pass; false having written a NUL-terminated named reason into `reasonOut`
// (truncated to `reasonCap`). Never throws, never crashes: every failure disables exactly ONE
// descriptor.
bool Run(const char* validateJson, const ModuleView& mv, const char* module, const void* fn,
         const Ops& ops, char* reasonOut, int reasonCap);

// ---------------------------------------------------------------------------
// ARG-WIDTH — is a declared inbound-hook SHAPE consistent with the callee's machine code?
//
// NOT part of the `validate` vocabulary above, and deliberately so: it takes no gamedata. The
// expectation comes from the SHAPE the hook descriptor already declares. The bug this exists to
// prevent was a hand-written ABI claim drifting from the binary; checking it with a SECOND
// hand-written ABI claim would be the same mistake with more steps. Nothing an author can typo,
// forget, or opt out of.
//
// THE RULE IS ONE-DIRECTIONAL. Declaring a parameter NARROWER than the engine's truncates it — the
// thunk reads 32 bits of a 64-bit register and zero-extends on the way back, so a pointer reaches
// the original with its top half gone and something dereferences it later, far from the hook. That
// is the live-server SIGSEGV on CCSGameRules::TerminateRound. Declaring it WIDER is harmless: SysV
// leaves the upper half of a 32-bit argument undefined, so relaying the whole register preserves
// whatever was actually there. So only narrowing is refused.
//
// A callee tells you how wide an argument is by how it STORES it:
//     mov %esi,%r15d          32-bit — an int
//     mov %rdx,-0xe0(%rbp)    64-bit — only a 64-bit value needs that
//
// SMOKE DETECTOR, NOT FIRE INSPECTION. Absence of evidence is a PASS: a function that never spills
// an argument in the scanned window yields no observation and must succeed. A validator that failed
// on "I could not tell" would refuse most functions and be switched off within a week, which is
// worse than not having it. It catches the class that burned us and is silent about the rest —
// do not read a pass as a proof of the signature.
// ---------------------------------------------------------------------------

/// One decoded observation about the instruction at `p`. Exposed for testing.
struct ArgUse {
    int          argIndex  = -1;     ///< SysV integer arg STORED to memory (0=this/rdi), else -1
    bool         wide      = false;  ///< REX.W — a 64-bit store
    int          redefines = -1;     ///< SysV integer arg this instruction OVERWRITES, else -1
    bool         terminator = false; ///< ret/jmp/jcc/call — the caller must stop scanning here
    unsigned     length    = 0;      ///< 0 = undecodable; the caller must stop
};

/// Decode one instruction, reading at most `avail` bytes and never past them.
ArgUse DecodeArgUse(const uint8_t* p, std::size_t avail);

/// Walk `fn`'s prologue and refuse a shape that declares a parameter narrower than the callee uses.
///
/// `wide[i]` describes SysV INTEGER ARGUMENT SLOT i — NOT declared param i. Slot 0 is `this`.
/// 0 = the shape relays this slot 32-bit, 1 = 64-bit (or it is a pointer we never narrow, like
/// `this`).
///
/// Indexed by slot rather than by param BECAUSE FLOAT PARAMS CONSUME NO INTEGER REGISTER. For
/// `void(this, float, int, int, int)` the integer slots are [this, arg1, arg2, arg3] and the float
/// rides in xmm0 — so declared param 2 lives in slot 2, not slot 3. Mapping param->slot by position
/// silently reports the wrong parameter the moment a float appears before it, which is exactly the
/// shape that motivated this validator. Let the caller, which knows the shape, do the flattening.
///
/// The widths arrive as an array rather than a shape id so this TU stays engine-free and the test can
/// drive it with hand-written widths and no shape enum at all.
///
/// Returns 0 on pass (INCLUDING "nothing observed" — `reasonOut` says which), -1 on a narrowing
/// mismatch with a named reason.
int ArgWidths(const uint8_t* wide, int count, const ModuleView& mv, const void* fn,
              char* reasonOut, int reasonCap);

}  // namespace s2validate

#endif  // S2SCRIPT_CALL_VALIDATE_H
