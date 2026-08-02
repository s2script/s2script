# Declarative Inbound Hooks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A game package or plugin declares an engine detour in its own gamedata, and plugins subscribe to it through `ctx` — closing the core-stabilization audit's headline gap, that core has a declarative *outbound* path but no declarative *inbound* one.

**Architecture:** Location is data, shape is compile-time. The shim ships a closed vocabulary of inbound *thunk shapes* (concrete C++ signatures) and never names a game function; gamedata says which shape a hook uses and where the function is. Resolution and validation reuse A5b's `S2_EngineCallResolve` unchanged. A bypass latch — SourceMod's `g_pIgnoreTerminateDetour`, ported — makes our own outbound descriptor call skip our own hook, which also means a hook only fires when core is *not* borrowed, so the pre-hook re-entrancy wall never arises.

**Tech Stack:** C++17 (shim, `s2detour::Install`, vendored HDE), Rust (core, `fan_out_collapsing`), JavaScript (`core/js/prelude.js`, `games/cs2/js/pawn.js`), TypeScript codegen (`packages/sdk` → `packages/cs2`).

**Spec:** `docs/superpowers/specs/2026-08-02-declarative-inbound-hooks-design.md`. Read it first; it is approved and authoritative.

## Global Constraints

- **Branch:** `core/declarative-inbound-hooks`. One slice, one PR, squash-merged. Never commit to `main`.
- **Depends on A5b** (PR #73) being on `main` — it supplies `call_validate.{h,cpp}`, the `validateJson` C-ABI, the `vtable-member` and `string-xref` validators, and the `@s2script/cs2` descriptor owner. Do not start Task 2 before it merges.
- **A validator is MANDATORY for every hook.** A `hooks` entry with no `validate` fails the **build**, not the load. A wrong call misbehaves; a wrong detour overwrites the prologue of whatever is actually there.
- **Degrade, never crash:** every failure path disables exactly one hook with a *named* reason and lets the framework keep running.
- **Engine-generic shim:** nothing in `shim/src` may name a game class, field, or function. Every name arrives as an opaque string from gamedata. `scripts/check-gamedata-owners.sh` is the gate.
- **No raw pointer crosses into JS.** A surfaced receiver goes through the books-gated `(index, id)` `EntityRef` path. The arg view is block-scoped and cannot outlive the dispatch.
- **New gates go in `scripts/ci-native.sh`**, never in `.github/workflows/*.yml` (add trigger *paths* to the YAML only).
- **Collapse contract:** `Handled` or `Stop` suppresses the original call; `Changed` writes mutable params back and still calls it; `Continue` passes through. Matches SM's `result >= Pl_Handled`.
- **Do not run** docker, the live CS2 gate, or `scripts/build-sniper.sh` — the human runs those.

---

### Task 1: The shape vocabulary and dispatch policy (engine-free)

The policy half, extracted so it is testable without the game — the pattern `shim/src/defer_queue.cpp` and `shim/src/call_validate.cpp` already established, both because a rule that cannot fail in a test is decoration.

**Files:**
- Create: `shim/src/hook_dispatch.h`, `shim/src/hook_dispatch.cpp`
- Create: `shim/tests/hook_dispatch_test.cpp`
- Create: `scripts/test-hook-dispatch.sh`
- Modify: `shim/CMakeLists.txt` (add a `hook_dispatch_selftest` target beside `gamedata_selftest`)
- Modify: `scripts/ci-native.sh` (add the gate after `test-defer-queue.sh`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `enum S2HookShape { S2_HOOK_SHAPE_THIS_VOID = 0, S2_HOOK_SHAPE_THIS_F32_I32_I32_I32 = 1 };`
  - `int S2Hook_ShapeFromName(const char* name);` → shape id, or `-1` for an unknown name.
  - `const char* S2Hook_ShapeName(int shape);` → the canonical name, or `nullptr`.
  - `struct S2HookOps { int (*dispatch)(int hookId, void* argView); };`
  - `void S2Hook_SetOps(const S2HookOps& ops);`
  - `bool S2Hook_BypassTake(int hookId);` — returns true **and clears** if the latch is set.
  - `void S2Hook_BypassArm(int hookId);`
  - `bool S2Hook_Suppresses(int hookResult);` — true for `Handled` (2) and `Stop` (3).

- [ ] **Step 1: Write the failing test**

Create `shim/tests/hook_dispatch_test.cpp`:

```cpp
// Unit test for the engine-free half of declarative inbound hooks (the shape table, the bypass
// latch, and the collapse contract). Self-contained: no engine, no gamedata, no CMake SDK paths.
#include "../src/hook_dispatch.h"
#include <cstring>
#include <iostream>

static int g_fail = 0;
#define CHECK(cond, msg)                                                   \
    do {                                                                   \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }  \
        else         { std::cout << "ok:   " << (msg) << "\n"; }            \
    } while (0)

static void test_shape_vocabulary_is_closed() {
    CHECK(S2Hook_ShapeFromName("this_void") == S2_HOOK_SHAPE_THIS_VOID, "this_void resolves");
    CHECK(S2Hook_ShapeFromName("this_f32_i32_i32_i32") == S2_HOOK_SHAPE_THIS_F32_I32_I32_I32,
          "this_f32_i32_i32_i32 resolves");
    // An unknown shape must be a NAMED failure, never a silent default to shape 0 — a typo would
    // otherwise install a detour with the wrong ABI, which corrupts the stack on first call.
    CHECK(S2Hook_ShapeFromName("this_i32") == -1, "an unknown shape is rejected");
    CHECK(S2Hook_ShapeFromName("") == -1, "an empty shape name is rejected");
    CHECK(std::strcmp(S2Hook_ShapeName(S2_HOOK_SHAPE_THIS_VOID), "this_void") == 0,
          "shape ids round-trip back to their names");
    CHECK(S2Hook_ShapeName(99) == nullptr, "an out-of-range shape id has no name");
}

static void test_bypass_latch_is_one_shot_and_per_hook() {
    // The latch is SourceMod's g_pIgnoreTerminateDetour: our own outbound call arms it, the thunk
    // takes it, and the hook does not fire for that one invocation.
    CHECK(!S2Hook_BypassTake(0), "a fresh latch is not set");
    S2Hook_BypassArm(0);
    CHECK(S2Hook_BypassTake(0), "an armed latch is taken");
    CHECK(!S2Hook_BypassTake(0), "and taking it CLEARS it — a stuck latch would silently kill the hook");
    S2Hook_BypassArm(0);
    CHECK(!S2Hook_BypassTake(1), "the latch is per-hook, not global");
    CHECK(S2Hook_BypassTake(0), "and hook 0's latch survived hook 1's take");
}

static void test_collapse_contract() {
    CHECK(!S2Hook_Suppresses(0), "Continue does not suppress");
    CHECK(!S2Hook_Suppresses(1), "Changed does not suppress — it writes params back and still calls");
    CHECK(S2Hook_Suppresses(2), "Handled suppresses the engine call");
    CHECK(S2Hook_Suppresses(3), "Stop suppresses the engine call");
    // An out-of-range collapse must NOT suppress: garbage from a handler must never silently
    // cancel engine behaviour (ARCHITECTURE.md — out-of-range maps to Continue).
    CHECK(!S2Hook_Suppresses(99), "an out-of-range result does not suppress");
    CHECK(!S2Hook_Suppresses(-1), "a negative result does not suppress");
}

static int g_dispatchCalls = 0;
static int g_lastHookId = -1;
static int fake_dispatch(int hookId, void*) { g_dispatchCalls++; g_lastHookId = hookId; return 0; }

static void test_ops_are_injected_not_linked() {
    S2HookOps ops{}; ops.dispatch = &fake_dispatch;
    S2Hook_SetOps(ops);
    g_dispatchCalls = 0;
    CHECK(S2Hook_Dispatch(7, nullptr) == 0, "dispatch forwards to the injected op");
    CHECK(g_dispatchCalls == 1 && g_lastHookId == 7, "and passes the hook id through");
    S2HookOps none{};
    S2Hook_SetOps(none);
    // A null op must be a no-suppress no-op, not a crash: core may not be initialised yet when the
    // engine calls through a detour installed by a previous load.
    CHECK(S2Hook_Dispatch(7, nullptr) == 0, "a null dispatch op degrades to Continue");
}

int main() {
    test_shape_vocabulary_is_closed();
    test_bypass_latch_is_one_shot_and_per_hook();
    test_collapse_contract();
    test_ops_are_injected_not_linked();
    if (g_fail) { std::cerr << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "hook_dispatch_test: all checks passed\n";
    return 0;
}
```

- [ ] **Step 2: Add the build and gate wiring**

In `shim/CMakeLists.txt`, beside the other selftest targets:

```cmake
# Engine-free half of declarative inbound hooks (not shipped; run by scripts/ci-native.sh).
add_executable(hook_dispatch_selftest src/hook_dispatch.cpp tests/hook_dispatch_test.cpp)
target_include_directories(hook_dispatch_selftest PRIVATE src)
```

Create `scripts/test-hook-dispatch.sh`:

```bash
#!/usr/bin/env bash
# Compile + run the engine-free hook dispatch policy test with the host compiler (no SDK, no game).
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/hook_dispatch_test"
g++ -std=c++17 -O2 -Wall -Wextra -o "$out" shim/src/hook_dispatch.cpp shim/tests/hook_dispatch_test.cpp
"$out"
```

```bash
chmod +x scripts/test-hook-dispatch.sh
```

In `scripts/ci-native.sh`, after the `test-defer-queue.sh` block:

```bash
echo "== test-hook-dispatch.sh (hook shape vocabulary, bypass latch, collapse) =="
bash scripts/test-hook-dispatch.sh
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `bash scripts/test-hook-dispatch.sh`
Expected: FAIL — compile error, `hook_dispatch.h: No such file or directory`.

- [ ] **Step 4: Write the header**

Create `shim/src/hook_dispatch.h`:

```cpp
// THE ENGINE-FREE HALF of declarative inbound hooks: the closed shape vocabulary, the per-hook
// bypass latch, and the collapse contract.
//
// WHY ITS OWN TU. The thunks themselves cannot be compiled outside the game (they need the entity
// bridge to marshal a receiver), so any rule living beside them would be untestable — the same hole
// shim/src/defer_queue.cpp and shim/src/call_validate.cpp were extracted to close. Every outside
// contact is INJECTED via S2HookOps, so shim/tests/hook_dispatch_test.cpp drives the SHIPPED code.
//
// ENGINE-GENERIC: nothing here names a game, a class, or a function.
#ifndef S2SCRIPT_HOOK_DISPATCH_H
#define S2SCRIPT_HOOK_DISPATCH_H

// The CLOSED vocabulary of inbound thunk shapes. A shape is a concrete C++ signature compiled into
// the shim; gamedata selects one BY NAME. You cannot detour an arbitrary signature from data — a
// handler must have the callee's exact ABI — which is why SourceMod uses per-arity DETOUR_DECL_MEMBER
// macros and why this vocabulary exists. Adding a shape is a CORE CHANGE, deliberately: only adding
// a hook on an EXISTING shape is zero-core-diff.
enum S2HookShape {
    S2_HOOK_SHAPE_THIS_VOID            = 0,  // void(void* self)
    S2_HOOK_SHAPE_THIS_F32_I32_I32_I32 = 1,  // void(void* self, float, int, int, int)
};

// Shape name <-> id. An unknown name returns -1 and an out-of-range id returns nullptr: a typo must
// fail BY NAME, never default to shape 0, whose wrong ABI would corrupt the stack on first call.
int         S2Hook_ShapeFromName(const char* name);
const char* S2Hook_ShapeName(int shape);

// Everything this TU needs from outside, injected so the policy is testable.
struct S2HookOps {
    // Fan the hook out to JS and return the COLLAPSED HookResult. `argView` is opaque here; the
    // thunk that owns the stack frame gives it meaning.
    int (*dispatch)(int hookId, void* argView) = nullptr;
};
void S2Hook_SetOps(const S2HookOps& ops);

// Dispatch through the injected op. A null op returns 0 (Continue) rather than crashing: the engine
// can call through a detour installed by a load whose core is already gone.
int S2Hook_Dispatch(int hookId, void* argView);

// The bypass latch — SourceMod's g_pIgnoreTerminateDetour (extensions/cstrike/forwards.cpp:132),
// made per-hook. Our own outbound descriptor invoke ARMS it; the thunk TAKES it and passes straight
// through to the original without firing the hook.
//
// Two reasons it matters. It is SM's semantic: a plugin-issued call does not fire the hook. And it
// is what keeps a hook from ever firing while core holds the isolate borrow — which is the wall
// that makes pre-hooks undeliverable (see the gamedata-tiering spec §9.2 / D3).
void S2Hook_BypassArm(int hookId);
bool S2Hook_BypassTake(int hookId);   // true AND clears, iff it was armed

// The collapse contract: Handled(2) or Stop(3) suppresses the original call; Continue(0) and
// Changed(1) do not. An out-of-range value does NOT suppress — garbage from a handler must never
// silently cancel engine behaviour (ARCHITECTURE.md maps out-of-range to Continue).
bool S2Hook_Suppresses(int hookResult);

// The number of hooks a build may install. Bounded so the latch array is fixed-size and lock-free.
enum { S2_HOOK_MAX = 64 };

#endif
```

- [ ] **Step 5: Write the implementation**

Create `shim/src/hook_dispatch.cpp`:

```cpp
#include "hook_dispatch.h"
#include <cstring>

namespace {
struct ShapeEntry { int id; const char* name; };
const ShapeEntry kShapes[] = {
    { S2_HOOK_SHAPE_THIS_VOID,            "this_void" },
    { S2_HOOK_SHAPE_THIS_F32_I32_I32_I32, "this_f32_i32_i32_i32" },
};
S2HookOps g_ops{};
bool      g_bypass[S2_HOOK_MAX] = { false };
}  // namespace

int S2Hook_ShapeFromName(const char* name) {
    if (!name || !name[0]) return -1;
    for (const ShapeEntry& e : kShapes)
        if (std::strcmp(e.name, name) == 0) return e.id;
    return -1;
}

const char* S2Hook_ShapeName(int shape) {
    for (const ShapeEntry& e : kShapes)
        if (e.id == shape) return e.name;
    return nullptr;
}

void S2Hook_SetOps(const S2HookOps& ops) { g_ops = ops; }

int S2Hook_Dispatch(int hookId, void* argView) {
    if (!g_ops.dispatch) return 0;                 // Continue — never crash on a stale detour
    return g_ops.dispatch(hookId, argView);
}

void S2Hook_BypassArm(int hookId) {
    if (hookId >= 0 && hookId < S2_HOOK_MAX) g_bypass[hookId] = true;
}

bool S2Hook_BypassTake(int hookId) {
    if (hookId < 0 || hookId >= S2_HOOK_MAX) return false;
    const bool was = g_bypass[hookId];
    g_bypass[hookId] = false;                      // one-shot: a stuck latch kills the hook silently
    return was;
}

bool S2Hook_Suppresses(int hookResult) { return hookResult == 2 || hookResult == 3; }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `bash scripts/test-hook-dispatch.sh`
Expected: PASS, ending `hook_dispatch_test: all checks passed`.

- [ ] **Step 7: Prove the tests bite**

A test that cannot fail is decoration. Verify two mutations and report both outputs:

```bash
# 1. Make an unknown shape default to 0 instead of -1.
sed -i 's|    return -1;\n}|    return 0;\n}|' shim/src/hook_dispatch.cpp   # if this no-ops, edit by hand
# 2. Make the latch NOT clear on take.
```

For each: edit, run `bash scripts/test-hook-dispatch.sh`, confirm it FAILS naming the right check, restore with `git checkout shim/src/hook_dispatch.cpp`, confirm it passes. Report the failing check names.

- [ ] **Step 8: Commit**

```bash
git add shim/src/hook_dispatch.h shim/src/hook_dispatch.cpp shim/tests/hook_dispatch_test.cpp \
        scripts/test-hook-dispatch.sh shim/CMakeLists.txt scripts/ci-native.sh
git commit -m "shim: the engine-free half of inbound hooks — shape vocabulary, bypass latch, collapse"
```

---

### Task 2: The thunks, install, and the C-ABI (engine side)

**Files:**
- Create: `shim/src/engine_hooks.h`, `shim/src/engine_hooks.cpp`
- Modify: `shim/CMakeLists.txt` (add `src/engine_hooks.cpp` to the `s2script` target sources)
- Modify: `shim/src/s2script_mm.cpp` (wire `S2_HookInstall`/`S2_HookArmBypass` into `S2EngineOps`, and call `S2Hook_SetOps` at Load)
- Modify: `shim/include/s2script_core.h` (declare the two new ops + the inbound dispatch entry)

**Interfaces:**
- Consumes: everything Task 1 produces; `s2detour::Install` (`shim/src/detour.h`); `S2_EngineCallResolve` (`shim/src/engine_calls.h`) for the address.
- Produces (all `extern "C"`):
  - `int S2_HookInstall(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap);` → 0 on success, -1 with a named reason.
  - `void S2_HookArmBypass(int hookId);`
  - `int S2_HookReadF32(void* argView, int idx, float* out);` / `S2_HookReadI32` / `S2_HookWriteF32` / `S2_HookWriteI32` — the block-scoped arg view accessors, index-checked against the shape.
  - `int S2_HookReceiverHandle(void* argView, uint32_t* outHandle);` → packed `CEntityHandle` for a surfaced receiver, or -1 when the shape/descriptor has none.
  - Core-side entry the thunks call: `int s2script_core_dispatch_hook(int hookId, void* argView);`

- [ ] **Step 1: Write the header**

Create `shim/src/engine_hooks.h`:

```cpp
#pragma once
#include <cstdint>

// Declarative inbound hooks — the ENGINE half: the compile-time thunks, detour installation, and
// the block-scoped arg view. Engine-generic: nothing here names a game class, field, or function;
// the address arrives already resolved and validated by S2_EngineCallResolve, and the shape arrives
// as an id from the closed vocabulary in hook_dispatch.h.
//
// LAZY INSTALL. Core calls S2_HookInstall on the FIRST-EVER subscribe to a hook, idempotently —
// the same discipline as the UserCmd.onRun detour (s2script_mm.cpp). No subscribers means no
// patched bytes and no treadmill surface. s2detour::RemoveAll() at Unload restores every prologue.
extern "C" {

// Install the detour. `addr` is a resolved, validated absolute address. Returns 0, or -1 with a
// NUL-terminated reason in `reasonOut` (s2detour::Install refuses an unrelocatable prologue, and a
// shape with no compiled thunk is a named failure, never a silent skip).
int  S2_HookInstall(int hookId, int shape, int64_t addr, char* reasonOut, int reasonCap);

// Arm this hook's bypass latch. Core calls it immediately before invoking the `bypassWith` call
// descriptor, so our own outbound call does not fire our own hook (SourceMod parity).
void S2_HookArmBypass(int hookId);

// Block-scoped arg view accessors. `idx` is the descriptor's positional param index; every one is
// bounds-checked against the installed shape, so a stale generated binding cannot read off the
// frame. Return 0 on success, -1 on a bad index or a null view.
int  S2_HookReadF32 (void* argView, int idx, float* out);
int  S2_HookReadI32 (void* argView, int idx, int32_t* out);
int  S2_HookWriteF32(void* argView, int idx, float value);
int  S2_HookWriteI32(void* argView, int idx, int32_t value);

// The detour's `this`, as a packed CEntityHandle, when the descriptor surfaces it. -1 when it does
// not. NO RAW POINTER LEAVES THIS TU — core turns the handle into a books-gated EntityRef.
int  S2_HookReceiverHandle(void* argView, uint32_t* outHandle);

}  // extern "C"
```

- [ ] **Step 2: Write the thunks and install**

Create `shim/src/engine_hooks.cpp`. The two thunks share one shape:

```cpp
#include "engine_hooks.h"
#include "hook_dispatch.h"
#include "detour.h"
#include <cstdio>
#include <cstring>

namespace {

// One installed hook. `orig` is s2detour's trampoline to the original function.
struct Installed {
    bool  used  = false;
    int   shape = -1;
    void* orig  = nullptr;
};
Installed g_hooks[S2_HOOK_MAX];

// The block-scoped view over a thunk's own stack args. It lives on the thunk's frame and dies with
// it, so nothing it points at can outlive the dispatch.
struct ArgView {
    int      hookId  = -1;
    int      shape   = -1;
    void*    self    = nullptr;
    float    f[1]    = { 0.0f };   // shape-dependent; f[0] = delay for this_f32_i32_i32_i32
    int32_t  i[3]    = { 0, 0, 0 };
};

// A hook id is carried in the thunk, not looked up — one compiled thunk per (shape, hookId) pair is
// impossible, so each shape gets a small dispatch table keyed by the detour target address.
// Simpler: at most one hook per address, so the thunk finds its id by matching `orig`.
int HookIdForTrampoline(void* fromThunk) {
    for (int i = 0; i < S2_HOOK_MAX; i++)
        if (g_hooks[i].used && g_hooks[i].orig == fromThunk) return i;
    return -1;
}

}  // namespace
```

> **Implementer note — the one genuinely hard part.** A detour handler receives no hook id: the
> engine calls it with the callee's arguments only. Two workable shapes, and you must pick one and
> say which in your report:
>
> **(a) One thunk per hook slot.** Generate `S2_HOOK_MAX` thunk instances per shape via a template
> parameterised on the id (`template <int Id> void Thunk_ThisVoid(void* self)`), take their
> addresses into a table at build time, and hand `s2detour::Install` the slot's own thunk. The id is
> then a compile-time constant inside the handler. Costs `S2_HOOK_MAX × shapes` tiny functions.
>
> **(b) One thunk per shape, id recovered from the return address.** Fragile and platform-specific.
>
> **Take (a).** It is boring, has no ABI risk, and `S2_HOOK_MAX = 64` keeps the table small. State
> the resulting binary-size delta in your report.

The body of each thunk, in order — this sequence is the contract:

```cpp
// 1. Take the bypass latch. Our own outbound call armed it, so pass straight through WITHOUT
//    firing the hook — SourceMod's forwards.cpp:159-163, and what keeps a hook from ever running
//    while core holds the isolate borrow.
if (S2Hook_BypassTake(id)) { call original; return; }

// 2. Build the block-scoped view over our own stack args.
ArgView v; v.hookId = id; v.shape = shape; v.self = self; v.f[0] = delay; v.i[0] = reason; ...

// 3. Fan out to JS and collapse.
const int r = S2Hook_Dispatch(id, &v);

// 4. Handled/Stop suppress the original entirely; Changed/Continue call it with the (possibly
//    rewritten) args read back OUT of the view.
if (S2Hook_Suppresses(r)) return;
call original with v.f[0], v.i[0], ...;
```

`S2_HookInstall` validates the shape id, refuses a duplicate install for the same id
(idempotent — core calls it on every subscribe and only the first should patch), calls
`s2detour::Install(reinterpret_cast<void*>(addr), thunkFor(shape, hookId), &g_hooks[hookId].orig)`,
and writes a named reason on failure.

- [ ] **Step 3: Wire the ops and the dispatch bridge**

In `shim/src/s2script_mm.cpp`, at Load, after core init:

```cpp
    S2HookOps hookOps{};
    hookOps.dispatch = &s2script_core_dispatch_hook;   // core's inbound entry
    S2Hook_SetOps(hookOps);
```

Append `hook_install` and `hook_arm_bypass` to `S2EngineOps` (append-only within a release, per the
convention comment A5b amended), and declare `s2script_core_dispatch_hook` in
`shim/include/s2script_core.h`.

- [ ] **Step 4: Build**

Run: `rm -rf build/shim && cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release -DS2_CORE_LIB_DIR=debug && cmake --build build/shim -j`
Expected: clean. Report any new warning in the edited TUs.

- [ ] **Step 5: Commit**

```bash
git add shim/src/engine_hooks.h shim/src/engine_hooks.cpp shim/src/s2script_mm.cpp \
        shim/include/s2script_core.h shim/CMakeLists.txt
git commit -m "shim: inbound hook thunks, lazy detour install, and the block-scoped arg view"
```

---

### Task 3: The core registry

**Files:**
- Create: `core/src/gamedata_hooks.rs`
- Modify: `core/src/lib.rs` (add the module)
- Modify: `core/src/ffi.rs` (add `s2script_core_dispatch_hook`)
- Modify: `core/src/v8host.rs` (the subscribe native + `S2EngineOps` fields)
- Modify: `core/src/loader.rs` (register a plugin's `hooks` beside its `calls`)

**Interfaces:**
- Consumes: `S2_HookInstall` / `S2_HookArmBypass` / the view accessors (Task 2); `fan_out_collapsing` (`core/src/v8host.rs`); the existing `engine_call_resolve` op.
- Produces:
  - `pub(crate) fn register_owner(owner_id: &str, gamedata_json: &str)` — mirrors `gamedata_calls::register_plugin`.
  - `pub(crate) fn subscribe(owner_id: &str, name: &str, handler: JsHandler) -> Result<(), String>`
  - `pub(crate) fn drop_owner(owner_id: &str)`
  - `pub(crate) fn status(owner_id: &str, name: &str) -> String`
  - native `__s2_hook_on(owner, name, handler)`

- [ ] **Step 1: Write the failing tests**

In `core/src/gamedata_hooks.rs`'s test module. Model the fixtures on `core/src/gamedata_calls.rs`'s
existing tests (`registry_degrades_when_permission_denied`, `status_of_unknown_call_is_named`):

```rust
#[test]
fn a_hook_without_a_validator_is_rejected_by_name() {
    // Mandatory-validator rule (spec §5): a wrong CALL misbehaves, a wrong DETOUR overwrites the
    // prologue of whatever is actually there. Uniqueness alone is not enough.
    let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig"},
                                 "shape":"this_void","expose":{"ctx":"g"}}}}"#;
    register_owner("@t/x", gd);
    assert!(status("@t/x", "onX").contains("validate"),
            "the reason must name the missing validator, got: {}", status("@t/x", "onX"));
}

#[test]
fn an_unknown_shape_is_rejected_by_name() {
    let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig",
                                           "validate":{"prologue":"55"}},
                                 "shape":"this_i32","expose":{"ctx":"g"}}}}"#;
    register_owner("@t/x", gd);
    assert!(status("@t/x", "onX").contains("shape"));
}

#[test]
fn an_unknown_bypass_target_is_rejected_by_name() {
    let gd = r#"{"hooks":{"onX":{"target":{"kind":"signature","name":"Sig",
                                           "validate":{"prologue":"55"}},
                                 "shape":"this_void","bypassWith":"nope","expose":{"ctx":"g"}}}}"#;
    register_owner("@t/x", gd);
    assert!(status("@t/x", "onX").contains("bypassWith"));
}

#[test]
fn install_happens_on_first_subscribe_only() {
    // Lazy install (spec §6): no subscribers, no patched bytes. The second subscribe must NOT
    // re-install — a double detour would chain the trampoline to itself.
    let installs = install_call_count_with_two_subscribes();
    assert_eq!(installs, 1, "install must be idempotent across subscribes");
}

#[test]
fn dropping_an_owner_leaves_the_detour_installed() {
    // §6: uninstalling a live detour races the engine calling through it. SM does not do it either.
    register_owner("@t/x", VALID_GD);
    subscribe("@t/x", "onX", dummy_handler()).unwrap();
    drop_owner("@t/x");
    assert!(detour_still_installed("onX"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p s2script-core gamedata_hooks`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement the registry**

`register_owner` parses `hooks`, and for each entry validates in this order, each failure producing
a **named** degrade reason stored for `status()`:

1. `expose.ctx` present and a plain identifier.
2. `shape` resolves through the shim's `S2Hook_ShapeFromName` (crossed as a string; the shim owns
   the vocabulary so an unknown name fails where the dispatch decision lives).
3. `validate` present and non-empty — **mandatory**, unlike `calls`.
4. `bypassWith`, when present, names a `calls` descriptor in the **same owner**.
5. `params` are unique plain identifiers; `mutable` ⊆ `params`.

Resolution reuses `engine_call_resolve` with the descriptor's `validateJson` — **no second
resolver**. Install is deferred to the first `subscribe`.

Dispatch (`s2script_core_dispatch_hook`) fans out through `fan_out_collapsing`, so priority,
per-handler `TryCatch` isolation and the collapse come for free.

The permission is `engine:hooks` — a **separate** constant from `engine:calls`, because a hook
patches bytes and can suppress engine behaviour. The reserved game-package owner is exempt, via the
same path A5b built.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p s2script-core`
Expected: PASS. Report the counts.

- [ ] **Step 5: Commit**

```bash
git add core/src/gamedata_hooks.rs core/src/lib.rs core/src/ffi.rs core/src/v8host.rs core/src/loader.rs
git commit -m "core: the declarative hook registry — mandatory validators, lazy install, engine:hooks"
```

---

### Task 4: The ctx extension point

**Files:**
- Modify: `core/js/prelude.js:1774` (after `var ctx = makeSubjects(null, ctxReg);`)
- Modify: `games/cs2/js/pawn.js` (contribute the two namespaces)

**Interfaces:**
- Consumes: `__s2_hook_on(owner, name, handler)` (Task 3).
- Produces: the convention `globalThis.__s2pkg_game_ctx = { <ns>: function (reg, viaId) { … } }`.

Ordering is already correct and must not be disturbed: `core/src/v8host.rs:7331-7338` runs the
engine prelude, then `@s2script/cs2`, and `ctx` is built per-plugin **after** both — so a namespace
`pawn.js` registers at package-eval time is present when the prelude builds `ctx`.

- [ ] **Step 1: Add the generic extension point**

In `core/js/prelude.js`, immediately after `ctx.previous = __s2_handoff_take();`:

```js
    // Game packages contribute their own ctx namespaces (e.g. ctx.gameRules) by setting
    // __s2pkg_game_ctx before this runs — the package prelude is evaluated ahead of ctx
    // construction (v8host.rs runs the engine prelude, then @s2script/<game>, then the plugin).
    // ENGINE-GENERIC: core names no game concept here; it merges whatever the package declares,
    // and each factory receives the same ledger registrar every built-in namespace uses, so a
    // game-package subscription is torn down at unload exactly like ctx.events.on.
    var gameCtx = globalThis.__s2pkg_game_ctx;
    if (gameCtx) {
      for (var ns in gameCtx) {
        if (Object.prototype.hasOwnProperty.call(gameCtx, ns) && typeof gameCtx[ns] === "function") {
          ctx[ns] = gameCtx[ns](ctxReg, viaId);
        }
      }
    }
```

- [ ] **Step 2: Contribute the CS2 namespaces**

In `games/cs2/js/pawn.js`, near the other `globalThis.__s2pkg_cs2` assignment:

```js
  // The ctx namespaces this game package contributes. Each factory gets the prelude's ledger
  // registrar, so ctx.gameRules.onTerminateRound is torn down at unload like any other
  // subscription. The hook NAMES here must match the `hooks` keys in gamedata/cs2/game.cs2.jsonc —
  // scripts/check-hooks-generated.sh is what keeps them in step.
  globalThis.__s2pkg_game_ctx = {
    gameRules: function (reg, viaId) {
      return {
        onTerminateRound: function (h) {
          reg(viaId(function () { return __s2_hook_on("@s2script/cs2", "onTerminateRound", h); }));
        },
      };
    },
    players: function (reg, viaId) {
      return {
        onRespawn: function (h) {
          reg(viaId(function () { return __s2_hook_on("@s2script/cs2", "onRespawn", h); }));
        },
      };
    },
  };
```

- [ ] **Step 3: Verify the prelude gate still passes**

`core/js/eslint.config.mjs` derives its globals from `set_native(...)` in `v8host.rs`, so a native
you added in Task 3 but spelled differently here is caught as `no-undef`.

Run: `bash scripts/check-core-js-lint.sh`
Expected: PASS. If it reports `no-undef` on `__s2_hook_on`, the native's name in `set_native(...)`
(Task 3) and the name used here disagree — fix the spelling, do not add it to a globals list.

- [ ] **Step 4: Commit**

```bash
git add core/js/prelude.js games/cs2/js/pawn.js
git commit -m "ctx: a generic game-package namespace extension point, and CS2's two hook namespaces"
```

---

### Task 5: The descriptors

**Files:**
- Modify: `gamedata/cs2/game.cs2.jsonc` (add the `hooks` section)

**Interfaces:**
- Consumes: the grammar from Task 3; the shape names from Task 1.
- Produces: `onTerminateRound` and `onRespawn`, consumed by Tasks 4 and 6.

- [ ] **Step 1: Add the two hooks**

Use the descriptors exactly as spec §5 gives them — including `receiver`, `params`, `mutable`,
`bypassWith` and `expose`. Both reuse signatures **already present** in this file from A5b
(`TerminateRound`, `Respawn`) with their existing `validate` blocks; do not duplicate the signature
entries, reference them by name.

Give the section a header comment recording that a hook's validator is mandatory and why.

- [ ] **Step 2: Verify the gates**

Run: `bash scripts/check-gamedata-owners.sh` — proves the shim still names none of these keys.
Run: `bash scripts/check-gamedata-sigs.sh` — proves no build-specific operand crept in.
Run: `bash scripts/check-call-descriptors.sh` — the A5b descriptor gate must still pass.
Expected: all three pass. Report the output.

- [ ] **Step 3: Commit**

```bash
git add gamedata/cs2/game.cs2.jsonc
git commit -m "gamedata: declare onTerminateRound and onRespawn as inbound hooks"
```

---

### Task 6: Codegen and its freshness gate

**Files:**
- Create: `packages/sdk/src/hookgen/` (model + emit, mirroring `packages/sdk/src/eventgen/`)
- Modify: `packages/sdk/src/cli.ts:23-25` (add `s2s gen-hooks [--check]`)
- Create: `packages/cs2/hooks.generated.d.ts`
- Create: `scripts/check-hooks-generated.sh`
- Modify: `scripts/ci-js.sh` (add the gate)

**Interfaces:**
- Consumes: the descriptors (Task 5).
- Produces: the `PluginContext` augmentation and the per-hook view interfaces, exactly as spec §5b
  shows them.

- [ ] **Step 1: Generate**

Emit into `packages/cs2/hooks.generated.d.ts`: one `declare module "@s2script/sdk/plugin"` block
augmenting `PluginContext` with one readonly member per distinct `expose.ctx`, each carrying its
hooks; plus one exported view interface per hook, with `mutable` params writable and everything else
`readonly`, and a surfaced receiver typed as its marshalled kind.

Model the generator on `packages/sdk/src/eventgen/{model,emit-dts}.ts` — pure model, separate
emitter, so the model is unit-testable without the filesystem.

- [ ] **Step 2: The freshness gate**

Create `scripts/check-hooks-generated.sh` following `scripts/check-events-generated.sh`: run the
generator in `--check` mode and `git diff --exit-code` the generated file. Wire into `scripts/ci-js.sh`.

- [ ] **Step 3: Verify**

Run: `node node_modules/@s2script/sdk/dist/cli.js gen-hooks` then `bash scripts/check-hooks-generated.sh`.
Expected: the second passes with no diff.

Then confirm the typecheck gate bites: add a temporary plugin subscribing to a hook that does not
exist (`ctx.gameRules.onNope(() => {})`), build it, confirm `tsc` errors, and delete it. Report the
error text — this is what makes a stale binding a build failure rather than a silent no-op.

- [ ] **Step 4: Commit**

```bash
git add packages/sdk/src/hookgen packages/sdk/src/cli.ts packages/cs2/hooks.generated.d.ts \
        scripts/check-hooks-generated.sh scripts/ci-js.sh
git commit -m "sdk: generate the ctx hook augmentation from the descriptors, and gate its freshness"
```

---

### Task 7: Documentation and the changeset

**Files:**
- Modify: `docs/ARCHITECTURE.md` (a subsection beside the gamedata-ownership one)
- Modify: `packages/cs2/index.d.ts:336-343` (retarget the `Events.onPre` note)
- Modify: `docs/PROGRESS.md`
- Create: `.changeset/*.md` (patch for `@s2script/cs2` and `@s2script/sdk`)

- [ ] **Step 1: Retarget the `.d.ts` note**

`packages/cs2/index.d.ts:336-343` currently explains the `Events.onPre("round_end")` loss in terms
of the isolate borrow. Accurate, but it now reads as an apology for a limitation that has a
supported answer. Rewrite it to point at `ctx.gameRules.onTerminateRound` as the way to intervene,
keeping the factual note that a pre-hook is not deferrable.

- [ ] **Step 2: ARCHITECTURE.md**

Document the two-axis rule for hooks — **location is data, shape is compile-time** — the mandatory
validator, the bypass latch and what it implies (a hook does not fire for a plugin-issued call), and
that adding a *shape* is a core change while adding a *hook on an existing shape* is not.

- [ ] **Step 3: PROGRESS.md + changeset**

Append the slice entry with the evidence from the live gate the human runs. Add a changeset —
`packages/cs2` gains generated types and `packages/sdk` gains a CLI command, and
`check-changeset.sh` fails the build without one.

- [ ] **Step 4: Full gate**

Run: `CI=1 make ci` — check the exit status directly (`if CI=1 make ci; then echo PASS; else echo FAIL; fi`),
do not infer it from log text.

- [ ] **Step 5: Commit and push**

```bash
git add docs/ARCHITECTURE.md packages/cs2/index.d.ts docs/PROGRESS.md .changeset
git commit -m "docs: declarative inbound hooks — the model, the bypass semantics, and the shape rule"
git push -u origin core/declarative-inbound-hooks
```

Write the PR body to a file and use `gh pr create --body-file` — never a heredoc. Lead with **Why**.

---

## Live gate (the human runs this)

Offline tests cannot prove a detour diverts execution on the live binary. Fixtures go in
`tools/hookgate/`, modelled on `tools/a5bgate/`.

1. A natural round end fires `onTerminateRound` with a plausible `reason`.
2. `GameRules.terminateRound()` from JS does **not** fire it — the bypass.
3. `HookResult.Handled` from a handler actually prevents the round ending.
4. A mutated `reason` reaches the engine — the round ends with the substituted reason.
5. With no subscriber, the boot log shows the detour was never installed.
6. `ctx.players.onRespawn` fires on an engine round-start respawn but not on `player.respawn()`.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §4 shape vocabulary, closed, compile-time | 1 |
| §5 descriptor grammar (`receiver`/`params`/`mutable`/`bypassWith`/`expose`) | 3 (validation), 5 (data) |
| §5 mandatory validator | 3 |
| §5b ctx subscription, declaration merging, ledger teardown | 4 (runtime), 6 (types) |
| §6 resolution reuse, lazy install, no uninstall, collapse, block-scoped view | 2, 3 |
| §7 `engine:hooks` separate permission | 3 |
| §9 offline tests + codegen freshness | 1, 3, 6 |
| §9 live gate | human |
| §10 risks (latch leak, wrong address, shape growth) | 1 (latch), 2 (install failure), 4 (docs) |

**Type consistency:** `S2Hook_*` names, the `S2HookShape` ids, `S2HookOps.dispatch`, `S2_HOOK_MAX`,
and `s2script_core_dispatch_hook` are spelled identically in Tasks 1, 2 and 3. The hook keys
`onTerminateRound`/`onRespawn` and the namespaces `gameRules`/`players` are identical in Tasks 4, 5
and 6.

**Known soft spot, called out rather than hidden:** Task 2's hook-id recovery (one thunk per slot
via a template) is the only place the plan prescribes a mechanism the codebase has no precedent for.
It is bounded by `S2_HOOK_MAX = 64` and the implementer is asked to report the binary-size delta. If
it turns out badly, the fallback is fewer slots, not a different mechanism.
