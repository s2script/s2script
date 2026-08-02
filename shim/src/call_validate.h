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

}  // namespace s2validate

#endif  // S2SCRIPT_CALL_VALIDATE_H
