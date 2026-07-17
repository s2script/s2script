# Round Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. All RE constants are ALREADY resolved (offline, 2026-07-16, re-verified in this worktree against the pinned build 2000875) — there is no spike task; Task 1 is directly implementable.

**Goal:** Round control for the TTT port: `GameRules.terminateRound(reason, delay?)` (the ONE new engine fact — a self-resolved, semantically load-validated `CCSGameRules::TerminateRound` call, deferred out of the JS isolate borrow), plus a pure-reuse CS2-package surface: round-clock write (`setRoundTime`/`setTimeRemaining`/`addTimeRemaining`), `roundStartTime`/`timeElapsed`/`timeRemaining` reads, `Teams` score API, and the `RoundEndReason`/`WinPanelFinalEvent` const maps.

**Architecture:** One `S2EngineOps` op appended after `usercmd_clear_subtick`: `gamerules_terminate_round(idx, serial, rules_ptr_off, delay, reason)` ENQUEUES a single-slot pending request in the shim; a dedicated `Hook_GameFrameRoundDrain` GameFrame pre-hook (installed eagerly at Load iff the sig resolves) drains it OUTSIDE the JS borrow so the synchronous round_end reaches every plugin. The sig passes TWO boot gates: `ResolveSigValidated` uniqueness AND a new semantic check that the masked `lea` at fn+0xb targets the C-string `"TerminateRound"` (the borrowed CSSharp/Swiftly sig is unique-but-WRONG on this build — va `0xc75ec0` vs the real `0x1384e80`). Everything else is game-package JS over existing primitives (`writeInt32Via`, `notifyStateChanged`, `findByClass`, `Events.fire`).

**Tech stack:** Rust core (`core/src/v8host.rs`), C++ shim (Metamod, `shim/src/s2script_mm.cpp`), gamedata JSONC, `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`.

**Spec:** `docs/superpowers/specs/2026-07-16-round-control-design.md`. **Graphite stack:** Task 1 = PR1 (engine fact, core+shim+gamedata atomic), Task 2 = PR2 (cs2 package surface + changeset), Task 3 = PR3 (demo + live gate + PROGRESS.md).

## Global Constraints

- **RE doctrine:** ship ONLY the self-derived pattern `55 48 89 E5 41 57 41 89 F7 41 56 48 8D 35 ? ? ? ? 41 55 41 54 53 48 89 FB 48 81 EC ? ? ? ?` (masks the string disp + frame size). The borrowed CSSharp/Swiftly sig is documented as a REJECTED HINT in the gamedata comment — it uniquely matches the WRONG function on 2000875. The prototype is delay-first: `void TerminateRound(float delay /*xmm0*/, uint32 reason /*esi*/, void* unk3=0, uint32 unk4=0)` — TTT's reason-first Linux order is a managed-marshaller artifact; copying it into a direct C call swaps `delay` into the reason register.
- **Semantic gate is load-bearing:** uniqueness alone green-lights a corrupting call on this exact build. The `TerminateRound.scope-string` check (lea at fn+0xb → `"TerminateRound"`) must gate `s_pTerminateRound` assignment; on failure the descriptor degrades with the named reason and the op returns 0 forever. NEVER weaken it to ship.
- **Deferral is a MUST, not polish:** the engine call runs ONLY from the drain hook (outside the JS borrow). An inline call from the op would make every plugin silently miss round_end (isolate-borrow re-entrancy memory).
- **ABI discipline:** the new op is APPENDED at the STRUCT TAIL after `usercmd_clear_subtick`, byte-identical across all five touchpoints: C header typedef+field (`shim/include/s2script_core.h`), Rust typedef+field (`core/src/v8host.rs`), BOTH in-test `S2EngineOps{...}` literals (`v8host.rs` ~11042 and ~11695), and the shim `ops.` wiring. Verify with the ordered-field-name diff (Task 1 Step 4) — run it BEFORE the change (must be empty) and AFTER (must be empty again). If `feat/writeconvar` merged first, re-tail after ITS last field instead.
- **Boundary:** no CS2 name crosses the C ABI or enters `core/src` (the op signature is opaque ints/float); `"CCSGameRulesProxy"`/`"CCSGameRules"`/`"CTeam"`/`"cs_team_manager"`/`RoundEndReason` live ONLY in `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`. Gates: `make check-boundary`, `./scripts/test-boundary-nameleak.sh`.
- **Degrade-never-crash:** unresolved/failed-semantic sig → op returns 0; stale proxy at enqueue → 0; stale proxy or null rules pointer at drain → logged drop; reason outside 0..22 → 0 (in-range legacy holes 2/3/15 pass through — the engine's own switch handles them); fn-pointer `.text` range guard (the ChangeTeam guard) at drain.
- **Never a raw pointer to JS:** JS passes `(index, serial, offset)`; the shim derefs (serial-gated at BOTH enqueue and drain).
- **Writers return booleans:** all new pawn.js write surfaces (`setRoundTime`, `setTimeRemaining`, `addTimeRemaining`, `terminateRound`, `Teams.setScore`) return success booleans; `.d.ts` additions are ADDITIVE (existing readonly fields untouched — 5E.1-safe).
- **cargo test is forced single-threaded** via `.cargo/config.toml` — do not pass `--test-threads`.
- **Deploy gotcha:** dist pawn.js is a CONCAT (schema/nav/activity/csitem/pawn) — never raw-cp a single file; rebuild via `scripts/package-addon.sh` / `make package`.

---

## File Structure

- Task 1: `gamedata/core.gamedata.jsonc`, `shim/src/s2script_mm.cpp`, `shim/src/s2script_mm.h`, `shim/include/s2script_core.h`, `core/src/v8host.rs`
- Task 2: `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`, `.changeset/round-control-cs2.md`
- Task 3: `examples/round-control-demo/{package.json,tsconfig.json,src/plugin.ts}`, `docs/PROGRESS.md`

---

## Task 1 (PR1): TerminateRound engine fact — gamedata sig + semantic gate + deferred op + ABI append

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (new `TerminateRound` entry, after the `ChangeTeam` block)
- Modify: `shim/src/s2script_mm.cpp` (`FindModuleBounds`, semantic check, op + pending slot + drain hook, Load resolve, ops wiring, Unload removal)
- Modify: `shim/src/s2script_mm.h` (drain-hook member declaration)
- Modify: `shim/include/s2script_core.h` (op typedef + struct field, tail)
- Modify: `core/src/v8host.rs` (Rust typedef + field + BOTH test literals + native + `set_native` + degrade test)

**Interfaces:**
- Produces (C ABI, appended after `usercmd_clear_subtick`): `typedef int (*s2_gamerules_terminate_round_fn)(int idx, int serial, int rules_ptr_off, float delay, int reason);` — returns 1 = queued (executes next GameFrame outside the JS borrow), 0 = degraded.
- Produces (JS native, raw-context): `__s2_gamerules_terminate_round(index, serial, rulesPtrOff, delay, reason) -> 0|1`.
- Consumes: `ResolveSigValidated`, `GamedataResult`, `FindModuleText`, `s2sig::ResolveLeaDisp` (`shim/src/sigscan.h:33`), `s2_deref_handle`, `s_serverText`/`s_serverTextSize`, `SH_DECL_HOOK3_void(ISource2Server, GameFrame, …)` (already declared, `s2script_mm.cpp:76`).

**Steps:**

- [ ] **Step 1 — Re-verify the sig offline (2 minutes, no live server).** Scratch script (do not commit) against the pinned binary:

```bash
python3 - <<'EOF'
import struct
p = "/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so"
data = open(p,'rb').read()
e_phoff = struct.unpack_from('<Q',data,0x20)[0]
psz = struct.unpack_from('<H',data,0x36)[0]; pn = struct.unpack_from('<H',data,0x38)[0]
xo=xv=xs=0
for i in range(pn):
    o = e_phoff + i*psz
    t,f = struct.unpack_from('<II',data,o)
    off,va,_,fs = struct.unpack_from('<QQQQ',data,o+8)
    if t==1 and (f&1) and fs>xs: xo,xv,xs = off,va,fs
text = data[xo:xo+xs]
def scan(pat):
    toks=[-1 if t in('?','??') else int(t,16) for t in pat.split()]
    return [i for i in range(len(text)-len(toks))
            if all(t==-1 or text[i+j]==t for j,t in enumerate(toks))]
fresh = "55 48 89 E5 41 57 41 89 F7 41 56 48 8D 35 ? ? ? ? 41 55 41 54 53 48 89 FB 48 81 EC ? ? ? ?"
hits = scan(fresh)
print("fresh hits:", len(hits), ["va=%#x"%(xv+h) for h in hits])
fn = hits[0]
disp = struct.unpack_from('<i', text, fn+0xb+3)[0]
sva = xv + fn + 0xb + 7 + disp
for i in range(pn):
    o = e_phoff + i*psz
    t,_ = struct.unpack_from('<II',data,o)
    off,va,_,fs = struct.unpack_from('<QQQQ',data,o+8)
    if t==1 and va<=sva<va+fs: print("lea string:", data[off+(sva-va):off+(sva-va)+15])
EOF
```
Expected output: `fresh hits: 1 ['va=0x1384e80']` and `lea string: b'TerminateRound\x00'`. If either differs, STOP — the pinned binary changed; re-run the string-xref recipe from the spec §2.1 before proceeding.

- [ ] **Step 2 — Gamedata entry.** In `gamedata/core.gamedata.jsonc`, insert directly AFTER the `ChangeTeam` entry's closing `},`:

```jsonc
    // CCSGameRules::TerminateRound(float delay /*xmm0*/, uint32 reason /*esi*/, void* unk3=0 /*rdx*/,
    // uint32 unk4=0 /*ecx*/) — force the current round to end with a RoundEndReason. Backs
    // GameRules.terminateRound. SELF-DERIVED against OUR libserver.so (build 2000875) — do NOT ship the
    // borrowed CSSharp/Swiftly sig ("55 48 89 E5 41 57 41 56 49 89 FE 41 55 41 54 53 48 81 EC ? ? ? ?
    // 48 8D 05 ? ? ? ? F3 0F 11 85"): on this build it matches UNIQUELY yet at the WRONG function
    // (va 0xc75ec0 — treats esi as a POINTER; a uniqueness gate alone would green-light a corrupting
    // call). The real function (va 0x1384e80 on 2000875) is anchored by: (a) a unique
    // `lea rsi,[rip+disp]` to the literal "TerminateRound" telemetry-scope string at fn+0xb (exactly 1
    // xref binary-wide), (b) the sole xref to "TerminateRound: unknown round end ID %i" inside its own
    // cold branch after `cmp $0x16,%r15d` (the reason-enum bound = 22 = SurvivalDraw), (c) the
    // CS:GO-inherited reason -> "#SFUI_Notice_*" switch in the body (16 direct callers in the gamerules
    // TU). This pattern masks the volatile string disp + frame size and keeps the stable `41 89 F7`
    // reason capture. The boot gate re-validates UNIQUE, and a SECOND semantic descriptor
    // (TerminateRound.scope-string) verifies the fn+0xb lea really targets "TerminateRound" — the exact
    // check that separates the real function from the borrowed-sig false positive. Treadmill
    // re-resolution recipe: xref the two unique strings above. NOT virtual on this build (zero
    // data-segment refs) — direct call only. TTT/CSSharp's reason-first Linux arg order is a managed-
    // marshaller artifact; the direct C call is DELAY-FIRST. args 3/4 semantics unknown in every
    // framework (all pass 0,0) — hardcoded 0, never exposed; revisit if a future build uses them.
    "TerminateRound": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 41 57 41 89 F7 41 56 48 8D 35 ? ? ? ? 41 55 41 54 53 48 89 FB 48 81 EC ? ? ? ?",
        "resolve": "direct"
      }
    },
```

- [ ] **Step 3 — C header (ABI tail).** In `shim/include/s2script_core.h`: after the `s2_usercmd_clear_subtick_fn` typedef (line ~261) append:

```c
/* Round-control slice — APPENDED after usercmd_clear_subtick; order is the ABI.
 * gamerules_terminate_round(idx, serial, rules_ptr_off, delay, reason) -> 1 queued / 0 degraded.
 * (idx, serial) = the rules PROXY entity; rules_ptr_off = the offset of its rules-struct pointer
 * field (resolved by the game package; no game names cross this ABI). DEFERRED: the shim queues the
 * call and drains it on the next GameFrame OUTSIDE the JS isolate borrow — the engine call fires the
 * round-end event machinery synchronously, and an inline call from JS would silently skip every
 * plugin's round_end handler via the try_borrow re-entrancy guard. reason is bounded 0..22. */
typedef int (*s2_gamerules_terminate_round_fn)(int idx, int serial, int rules_ptr_off,
                                               float delay, int reason);
```

and inside the `S2EngineOps` struct, after `s2_usercmd_clear_subtick_fn usercmd_clear_subtick;` (line ~375):

```c
    /* Round-control slice — APPENDED after usercmd_clear_subtick; order is the ABI; do not reorder above. */
    s2_gamerules_terminate_round_fn gamerules_terminate_round;
```

- [ ] **Step 4 — Rust mirror + ABI parity check.** In `core/src/v8host.rs`:
  - After `type UsercmdClearSubtickFn = extern "C" fn();` (~line 238):

```rust
// --- Round-control slice (APPENDED after usercmd_clear_subtick; order is the ABI). ENGINE-GENERIC:
// (proxy idx, serial, rules-ptr field offset, delay, reason) -> 1 queued / 0 degraded. The shim defers
// the sig-resolved engine call to the next GameFrame OUTSIDE the JS isolate borrow (it fires round_end
// synchronously). No game names cross the ABI.
type GamerulesTerminateRoundFn = extern "C" fn(c_int, c_int, c_int, f32, c_int) -> c_int;
```

  - After `pub usercmd_clear_subtick:  Option<UsercmdClearSubtickFn>,` (~line 359):

```rust
    // --- Round-control slice (APPENDED after usercmd_clear_subtick; order is the ABI; do not reorder
    // above) ---
    pub gamerules_terminate_round: Option<GamerulesTerminateRoundFn>,
```

  - In BOTH in-test `S2EngineOps { ... }` literals (after each `usercmd_clear_subtick: None,` — ~lines 11042 and 11695): `gamerules_terminate_round: None,`
  - Run the ordered-field-name parity diff BEFORE committing (validate it is empty on the pre-change tree first, then again after):

```bash
diff <(awk '/^typedef struct \{/{n=0} /^[ \t]+s2_[a-z0-9_]+_fn/{sub(/;.*/,"",$2); f[++n]=$2} /^\} S2EngineOps;/{for(i=1;i<=n;i++) print f[i]; exit}' shim/include/s2script_core.h) \
     <(sed -n '/^pub struct S2EngineOps {/,/^}/p' core/src/v8host.rs | sed -nE 's/^[ \t]*pub ([a-z0-9_]+):.*/\1/p')
```
Expected: empty output, exit 0, both before and after (after: both lists end `… usercmd_clear_subtick, gamerules_terminate_round`).

- [ ] **Step 5 — Core native + registration + degrade test.** In `core/src/v8host.rs`:
  - Beside `s2_player_change_team` (~line 5119) add:

```rust
/// `__s2_gamerules_terminate_round(index, serial, rulesPtrOff, delay, reason) -> 0|1` — queue a
/// round-termination via the sig-resolved engine op. (index, serial) identify the game-rules PROXY
/// entity; rulesPtrOff is the offset of its rules-struct pointer field (both supplied by the game
/// package — engine-generic here). 1 = queued: the shim executes on the NEXT GameFrame, outside the
/// JS isolate borrow, so the resulting round-end event dispatches to ALL plugins (including the
/// caller). 0 = degraded (no op / unresolved signature / stale proxy / out-of-range reason).
fn s2_gamerules_terminate_round(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 5 { return; }
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as c_int;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as c_int;
        let off = args.get(2).integer_value(scope).unwrap_or(-1) as c_int;
        let delay = args.get(3).number_value(scope).unwrap_or(0.0) as f32;
        let reason = args.get(4).integer_value(scope).unwrap_or(-1) as c_int;
        if off < 0 { return; }
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.gamerules_terminate_round else { return };
        rv.set_int32(f(index, serial, off, delay, reason));
    }));
}
```

  - Beside the other `set_native` calls (~line 6859): `set_native(scope, global_obj, "__s2_gamerules_terminate_round", s2_gamerules_terminate_round);`
  - Beside `usercmd_accessors_degrade_without_ops` (~line 12507) add:

```rust
    /// Round-control slice (degrade-never-crash): with NO engine ops installed,
    /// `__s2_gamerules_terminate_round` returns 0 (an int, never undefined) and never throws —
    /// the `GameRules.terminateRound -> false` degrade contract holds without a shim.
    #[test]
    fn gamerules_terminate_round_degrades_without_ops() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_gamerules_terminate_round(1, 2, 8, 5.0, 9))"), "0", "degrades to 0 without ops");
        assert_eq!(eval_in_context_string("p", "String(__s2_gamerules_terminate_round())"), "0", "no-args degrades to 0, no throw");
        shutdown();
    }
```

  - Run: `cargo test -p s2script-core gamerules_terminate_round` — Expected: `test result: ok. 1 passed`. Then `cargo test -p s2script-core` — Expected: all green.

- [ ] **Step 6 — Shim: `FindModuleBounds` + semantic check.** In `shim/src/s2script_mm.cpp`, directly after `FindModuleText`:

```cpp
// Full mapped [lo, hi) LOAD extent of the SAME module FindModuleText selects (largest-PF_X-wins,
// Metamod-proxy-safe). Needed because .rodata (where sig-anchoring C-strings live) sits in a LOAD
// segment BELOW the PF_X base — a rip-relative string target is OUTSIDE the .text buffer and must be
// range-guarded against the whole mapping before it is read.
struct ModBounds { const uint8_t* lo; const uint8_t* hi; };
static ModBounds FindModuleBounds(const char* soname) {
    struct Ctx { const char* name; size_t bestX; ModBounds out; } ctx{ soname, 0, { nullptr, nullptr } };
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;
        size_t maxX = 0;
        ElfW(Addr) lo = ~static_cast<ElfW(Addr)>(0), hi = 0;
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type != PT_LOAD) continue;
            if ((ph.p_flags & PF_X) && ph.p_filesz > maxX) maxX = ph.p_filesz;
            if (ph.p_vaddr < lo) lo = ph.p_vaddr;
            if (ph.p_vaddr + ph.p_memsz > hi) hi = ph.p_vaddr + ph.p_memsz;
        }
        if (maxX > c->bestX) {   // same winner rule as FindModuleText: largest PF_X segment
            c->bestX = maxX;
            c->out.lo = reinterpret_cast<const uint8_t*>(info->dlpi_addr + lo);
            c->out.hi = reinterpret_cast<const uint8_t*>(info->dlpi_addr + hi);
        }
        return 0;
    }, &ctx);
    return ctx.out;
}

// Semantic load-gate for the TerminateRound descriptor (uniqueness is NOT enough — the borrowed
// CSSharp/Swiftly sig matches UNIQUELY at the WRONG function on build 2000875). The self-derived
// pattern pins the `48 8D 35` (lea rsi,[rip+disp32]) opcode at fn+0xb and masks only the disp;
// this follows the disp and verifies the target is the literal scope string "TerminateRound".
static bool ValidateTerminateRoundScopeString(const ModText& mt, int64_t fnOff, const char* module) {
    int64_t tgt = s2sig::ResolveLeaDisp(mt.text, mt.size, fnOff + 0xb, /*dispOff=*/3, /*instrLen=*/7);
    if (tgt == s2sig::kFail) return false;
    const uint8_t* p = mt.text + tgt;   // typically BELOW mt.text (.rodata precedes .text in the map)
    ModBounds mb = FindModuleBounds(module);
    static const char kScope[] = "TerminateRound";   // compare INCLUDING the NUL
    if (!mb.lo || p < mb.lo || p + sizeof(kScope) > mb.hi) return false;
    return std::memcmp(p, kScope, sizeof(kScope)) == 0;
}
```
(Confirm `<link.h>`/`dl_iterate_phdr` and `<cstring>` are already included — `FindModuleText` uses them.)

- [ ] **Step 7 — Shim: op + pending slot + drain hook.** In `shim/src/s2script_mm.cpp`, directly after the `s2_player_change_team` block (~line 1210):

```cpp
// ---------------------------------------------------------------------------
// gamerules_terminate_round (round-control slice) — force the round to end via the sig-resolved
// CCSGameRules::TerminateRound(float delay, uint32 reason, void* unk3=0, uint32 unk4=0) (s_pTerminateRound,
// loaded in Load behind TWO gates: unique-match AND the scope-string semantic check — the borrowed
// CSSharp/Swiftly sig is unique-but-WRONG on 2000875). DEFERRED EXECUTION: TerminateRound fires the
// round-end event machinery SYNCHRONOUSLY; called inline from a JS native (inside the core's isolate
// borrow) the round_end re-entry would be try_borrow-skipped and EVERY plugin would silently miss the
// event. So the op only arms a single-slot pending request (latest-wins — a round ends once) and
// Hook_GameFrameRoundDrain (installed eagerly at Load iff the sig resolved; one branch/frame) executes
// it OUTSIDE the JS borrow. (idx, serial) identify the rules PROXY entity and rules_ptr_off the offset
// of its rules-struct pointer field — both come from the game package; no game names live here. The
// proxy is serial-gated at BOTH enqueue (fast feedback) and drain (it can die in between); the fn ptr
// is .text-range-guarded like ChangeTeam. reason is host-bounded 0..22 (mirrors the engine's own
// `cmp $0x16` check; in-range legacy holes 2/3/15 pass through — the engine's switch handles them).
// ---------------------------------------------------------------------------
typedef void (*TerminateRound_t)(void* rules, float delay, uint32_t reason, void* unk3, uint32_t unk4);
static TerminateRound_t s_pTerminateRound = nullptr;     // sig-resolved fn ptr (loaded in Load, dual-gated)
struct PendingTerminate { bool armed; uint32_t proxyHandle; int rulesPtrOff; float delay; int reason; };
static PendingTerminate s_pendingTerminate = { false, 0, 0, 0.0f, 0 };
static bool s_termDrainHooked = false;                   // Load-installed, Unload-removed

static int s2_gamerules_terminate_round(int idx, int serial, int rules_ptr_off, float delay, int reason) {
    if (!s_pTerminateRound) return 0;                    // signature unresolved/failed-semantic -> degrade
    if (reason < 0 || reason > 22) {
        META_CONPRINTF("[s2script] terminate_round: reason %d out of range 0..22 — rejected\n", reason);
        return 0;
    }
    if (rules_ptr_off < 0) return 0;
    CEntityHandle h(idx, serial);
    if (!s2_deref_handle(static_cast<unsigned int>(h.ToInt()))) return 0;  // stale proxy NOW; re-gated at drain
    if (s_pendingTerminate.armed)
        META_CONPRINTF("[s2script] terminate_round: overwriting a pending request (latest wins)\n");
    s_pendingTerminate = { true, static_cast<uint32_t>(h.ToInt()), rules_ptr_off, delay, reason };
    return 1;
}
```

Then, beside `Hook_GameFramePre`'s definition (~line 3650), add the drain member:

```cpp
void S2ScriptPlugin::Hook_GameFrameRoundDrain(bool, bool, bool) {
    if (s_pendingTerminate.armed) {
        PendingTerminate req = s_pendingTerminate;
        s_pendingTerminate.armed = false;               // consume before calling (the call re-enters gamerules)
        void* proxy = s2_deref_handle(req.proxyHandle); // re-gate: the proxy can die between enqueue and drain
        const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pTerminateRound);
        if (proxy && s_pTerminateRound && s_serverText && f >= s_serverText && f < s_serverText + s_serverTextSize) {
            void* rules = *reinterpret_cast<void**>(reinterpret_cast<char*>(proxy) + req.rulesPtrOff);
            if (rules) {
                // OUTSIDE the JS isolate borrow: the synchronous round_end flows through the normal
                // FireEvent pre-hook -> core dispatch -> every plugin's subscribers.
                s_pTerminateRound(rules, req.delay, static_cast<uint32_t>(req.reason), nullptr, 0);
            } else {
                META_CONPRINTF("[s2script] terminate_round: null rules pointer at drain — dropped\n");
            }
        } else {
            META_CONPRINTF("[s2script] terminate_round: stale proxy / fn out of .text at drain — dropped\n");
        }
    }
    RETURN_META(MRES_IGNORED);
}
```

And in `shim/src/s2script_mm.h`, after `void Hook_GameFramePost(bool simulating, bool first, bool last);` (line 40):

```cpp
    void Hook_GameFrameRoundDrain(bool simulating, bool first, bool last);
```

- [ ] **Step 8 — Shim: Load resolve (dual-gated) + eager drain hook + wiring + Unload.** In `S2ScriptPlugin::Load`, directly after the `ChangeTeam` resolve block (~line 2992):

```cpp
            // Round-control slice: resolve CCSGameRules::TerminateRound (GameRules.terminateRound).
            // DUAL-GATED: unique-match (ResolveSigValidated) AND the scope-string semantic check —
            // on THIS build the borrowed CSSharp/Swiftly sig is unique yet lands on the WRONG function,
            // so uniqueness alone must never assign the pointer. Failure of either gate leaves
            // s_pTerminateRound null -> the op degrades to 0 (degrade-never-crash).
            auto trit = sigs.find("TerminateRound");
            if (trit == sigs.end()) {
                GamedataResult("TerminateRound", false, "signature absent from gamedata");
            } else {
                int64_t trOff = ResolveSigValidated("TerminateRound", trit->second);
                ModText trmt = FindModuleText(trit->second.module.c_str());
                if (trOff != s2sig::kFail && trmt.text) {
                    if (!ValidateTerminateRoundScopeString(trmt, trOff, trit->second.module.c_str())) {
                        GamedataResult("TerminateRound.scope-string", false,
                            "prologue lea does not reference the 'TerminateRound' scope string "
                            "(unique-but-WRONG match — the borrowed-sig trap); descriptor disabled");
                    } else {
                        GamedataResult("TerminateRound.scope-string", true, nullptr);
                        s_pTerminateRound = reinterpret_cast<TerminateRound_t>(const_cast<uint8_t*>(trmt.text) + trOff);
                        s_serverText = trmt.text; s_serverTextSize = trmt.size;
                        META_CONPRINTF("[s2script] TerminateRound resolved @%p (GameRules.terminateRound)\n",
                                       reinterpret_cast<void*>(s_pTerminateRound));
                        // Eager drain-hook install (NOT lazy): adding a SourceHook from inside a frame
                        // dispatch would mutate the hook chain mid-iteration; one if-not-armed branch
                        // per frame is negligible.
                        if (m_server && !s_termDrainHooked) {
                            SH_ADD_HOOK(ISource2Server, GameFrame, m_server,
                                        SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRoundDrain), false);
                            s_termDrainHooked = true;
                        }
                    }
                }   // trOff == kFail: ResolveSigValidated already recorded the reason
            }
```

Wire the op after `ops.usercmd_clear_subtick = &s2_usercmd_clear_subtick;` (~line 3476):

```cpp
    // Round-control slice — APPENDED after usercmd_clear_subtick; order MUST match S2EngineOps.
    ops.gamerules_terminate_round = &s2_gamerules_terminate_round;
```

In `S2ScriptPlugin::Unload`, beside the existing GameFrame `SH_REMOVE_HOOK` pair (~line 3537):

```cpp
    if (s_termDrainHooked) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRoundDrain), false);
        s_termDrainHooked = false;
        s_pendingTerminate.armed = false;
    }
```

- [ ] **Step 9 — Build + gates + commit (PR1).**

```bash
make core && make shim
cargo test -p s2script-core
make check-boundary
./scripts/test-boundary-nameleak.sh
```
Expected: both builds green; all tests pass; both boundary gates green (no CS2 name entered `core/src` — the core diff contains only opaque ints/float). Commit:

```bash
git add gamedata/core.gamedata.jsonc shim/include/s2script_core.h shim/src/s2script_mm.cpp shim/src/s2script_mm.h core/src/v8host.rs
git commit -m "feat(round): TerminateRound engine fact — self-derived sig + scope-string semantic gate + deferred terminate op"
```

---

## Task 2 (PR2): CS2 package surface — GameRules writers, Teams, RoundEndReason

**Files:**
- Modify: `games/cs2/js/pawn.js` (GameRulesView additions, `Teams`, `RoundEndReason`, `WinPanelFinalEvent`, export)
- Modify: `packages/cs2/index.d.ts` (GameRulesView additions, `GameRules.terminateRound`, `Teams`, const maps)
- Create: `.changeset/round-control-cs2.md`

**Interfaces:**
- Consumes: `__s2_gamerules_terminate_round` (Task 1), `__s2_schema_offset`, `EntityRef.writeInt32Via/readFloat32Via/writeInt32/readUInt8/readInt32/notifyStateChanged`, `globalThis.__s2pkg_entity.Entity.findByClass`, `globalThis.__s2pkg_server.Server.gameTime`.
- Produces: the spec §3 API exactly (`GameRulesView.roundStartTime/timeElapsed/timeRemaining/setRoundTime/setTimeRemaining/addTimeRemaining/terminateRound`, `GameRules.terminateRound`, `Teams.getScore/setScore/addScore`, `RoundEndReason`, `WinPanelFinalEvent`).

**Steps:**

- [ ] **Step 1 — pawn.js: GameRulesView additions.** In `games/cs2/js/pawn.js`, inside the `Object.defineProperties(GameRulesView.prototype, { ... })` block: append a comma to the current last entry (`hasMatchStarted:       grBool("m_bHasMatchStarted")`) — keep the existing 12 entries otherwise untouched — then add:

```js
    // Round-control slice: m_fRoundStartTime (GameTime_t read as f32 — validated live: ~= gameTime at
    // round_start) + the TTT GetTimeElapsed/GetTimeRemaining compositions over Server.gameTime.
    roundStartTime:        grFloat("m_fRoundStartTime"),
    timeElapsed: { get: function () {
      var st = this.roundStartTime, ft = this.freezeTime;
      var srv = globalThis.__s2pkg_server;
      var now = srv && srv.Server ? srv.Server.gameTime : null;
      if (st === null || ft === null || now === null || now === 0) return null;
      return now - st - ft;
    } },
    timeRemaining: { get: function () {
      var rt = this.roundTime, el = this.timeElapsed;
      if (rt === null || el === null) return null;
      return rt - el;
    } }
```

Directly after the `Object.defineProperties(...)` call, add the methods (writers return booleans — a failed clock write must be detectable):

```js
  // Write m_iRoundTime through the proxy's m_pGameRules chain, then dirty the PROXY at the
  // m_pGameRules offset (a FLAT offset on the proxy root — the TTT/CSSharp
  // SetStateChanged(proxy, "CCSGameRulesProxy", "m_pGameRules") pattern) so the change renetworks.
  // writeInt32Via deliberately does NOT auto-notify; forgetting the notify means the HUD clock never
  // repaints on clients (the live-gate criterion).
  GameRulesView.prototype.setRoundTime = function (seconds) {
    var p = grPath(); if (!p) return false;
    var o = __s2_schema_offset("CCSGameRules", "m_iRoundTime"); if (o < 0) return false;
    if (!this.ref.writeInt32Via(p, o, seconds | 0)) return false;
    this.ref.notifyStateChanged(p[0]);
    return true;
  };
  // TTT SetTimeRemaining: roundTime = elapsed + seconds.
  GameRulesView.prototype.setTimeRemaining = function (seconds) {
    var el = this.timeElapsed; if (el === null) return false;
    return this.setRoundTime(Math.ceil(el + seconds));
  };
  // TTT AddTimeRemaining: roundTime += delta.
  GameRulesView.prototype.addTimeRemaining = function (seconds) {
    var rt = this.roundTime; if (rt === null) return false;
    return this.setRoundTime(rt + Math.ceil(seconds));
  };
  // Force the round to end (CCSGameRules::TerminateRound via the deferred engine op). QUEUED: executes
  // on the NEXT engine frame, outside the JS isolate borrow, so round_end reaches EVERY plugin —
  // including this one. true = queued; false = degraded (unresolved sig / stale proxy / bad reason).
  GameRulesView.prototype.terminateRound = function (reason, delay) {
    var p = grPath(); if (!p) return false;
    if (typeof __s2_gamerules_terminate_round !== "function") return false;
    var d = (delay === undefined || delay === null) ? 5.0 : +delay;   // 5s = TTT parity default
    return __s2_gamerules_terminate_round(this.ref.index, this.ref.serial, p[0], d, reason | 0) === 1;
  };
```

- [ ] **Step 2 — pawn.js: `GameRules.terminateRound` convenience.** In the `GameRules` IIFE's returned object (after the `get:` entry), add:

```js
      ,
      terminateRound: function (reason, delay) {
        var v = this.get();
        return v ? v.terminateRound(reason, delay) : false;
      }
```

- [ ] **Step 3 — pawn.js: `Teams` + const maps.** After the `GameRules` IIFE (before the user-message sugar), add:

```js
  // Team scoreboard scores — cs_team_manager entities (≈4: Unassigned/Spectator/T/CT) matched by
  // m_iTeamNum (NEVER by enumeration order), CTeam.m_iScore written flat + notifyStateChanged at the
  // SAME offset (the TTT SetStateChanged(entry, "CTeam", "m_iScore") pattern). Entities are re-found
  // per call (cold path) — deliberately NO cache: team entities die on map change, and TTT's own
  // `_teamManager ??=` cache is a bug we do not replicate.
  var Teams = {
    _find: function (team) {
      var ent = globalThis.__s2pkg_entity;
      if (!ent || !ent.Entity) return null;
      var tno = __s2_schema_offset("CBaseEntity", "m_iTeamNum"); if (tno < 0) return null;
      var refs = ent.Entity.findByClass("cs_team_manager") || [];
      for (var i = 0; i < refs.length; i++) {
        if (refs[i].readUInt8(tno) === (team | 0)) return refs[i];
      }
      return null;
    },
    getScore: function (team) {
      var ref = Teams._find(team); if (!ref) return null;
      var o = __s2_schema_offset("CTeam", "m_iScore"); if (o < 0) return null;
      return ref.readInt32(o);
    },
    setScore: function (team, score) {
      var ref = Teams._find(team); if (!ref) return false;
      var o = __s2_schema_offset("CTeam", "m_iScore"); if (o < 0) return false;
      if (!ref.writeInt32(o, score | 0)) return false;
      ref.notifyStateChanged(o);
      return true;
    },
    addScore: function (team, delta) {
      var cur = Teams.getScore(team); if (cur === null) return false;
      return Teams.setScore(team, cur + (delta | 0));
    }
  };

  // CS2 round-end reasons ("layout is data, semantics are code" — a name<->number mapping is reviewed
  // code). Values HINTed by the CSSharp enum and BINARY-VALIDATED against our build: the engine's
  // `cmp $0x16` bound (max 22 = SurvivalDraw) + every #SFUI_Notice_* switch string present. Gaps
  // 2/3/15 are removed legacy VIP reasons. Closed-loop re-validated at the live gate (terminateRound
  // reason vs the engine-emitted round_end.reason).
  var RoundEndReason = {
    Unknown: 0, TargetBombed: 1, TerroristsEscaped: 4, CTsPreventEscape: 5,
    EscapingTerroristsNeutralized: 6, BombDefused: 7, CTsWin: 8, TerroristsWin: 9,
    RoundDraw: 10, AllHostagesRescued: 11, TargetSaved: 12, HostagesNotRescued: 13,
    TerroristsNotEscaped: 14, GameCommencing: 16, TerroristsSurrender: 17, CTsSurrender: 18,
    TerroristsPlanted: 19, CTsReachedHostage: 20, SurvivalWin: 21, SurvivalDraw: 22
  };
  // cs_win_panel_round final_event values (HINT: TTT/CSSharp usage; validated at the live gate
  // against a natural round end's engine-emitted value).
  var WinPanelFinalEvent = { CTsWin: 2, TerroristsWin: 3 };
```

- [ ] **Step 4 — pawn.js: export.** In the `globalThis.__s2pkg_cs2 = Object.assign(...)` line at the file tail, add to the object literal: `Teams: Teams, RoundEndReason: RoundEndReason, WinPanelFinalEvent: WinPanelFinalEvent` (GameRules is already exported).

- [ ] **Step 5 — index.d.ts.** In `packages/cs2/index.d.ts`, inside `export interface GameRulesView { ... }` (after `readonly hasMatchStarted`), add:

```ts
  /** m_fRoundStartTime (GameTime_t): the map-time at which the current round started. */
  readonly roundStartTime: number | null;
  /** Server.gameTime - roundStartTime - freezeTime (TTT GetTimeElapsed). null pre-round / no proxy. */
  readonly timeElapsed: number | null;
  /** roundTime - timeElapsed (TTT GetTimeRemaining). */
  readonly timeRemaining: number | null;
  /** Write m_iRoundTime and renetwork it (proxy notifyStateChanged at m_pGameRules — the HUD clock
   *  repaints on clients). Returns false if the proxy is stale or an offset fails to resolve. */
  setRoundTime(seconds: number): boolean;
  /** Set the REMAINING round time (writes roundTime = timeElapsed + seconds). */
  setTimeRemaining(seconds: number): boolean;
  /** Extend/shrink the round clock by delta seconds (writes roundTime += seconds). */
  addTimeRemaining(seconds: number): boolean;
  /** Force the round to end with a RoundEndReason (sig-resolved CCSGameRules::TerminateRound).
   *  QUEUED: executes on the NEXT engine frame, outside the JS isolate borrow, so every plugin's
   *  round_end handler — including the caller's — fires normally (a state read immediately after
   *  still sees the old round). delay (default 5s) is the engine's pre-restart delay. Returns true if
   *  queued; false when degraded (unresolved signature, stale proxy, or reason outside 0..22). */
  terminateRound(reason: number, delay?: number): boolean;
```

Replace the `GameRules` declaration with:

```ts
/** Read + drive CCSGameRules state. get() re-finds the cs_gamerules proxy each call (serial-gated
 *  cache); returns null when no proxy exists (e.g. pre-map-load). */
export declare const GameRules: {
  get(): GameRulesView | null;
  /** Convenience over get()?.terminateRound(reason, delay) — false when no proxy. */
  terminateRound(reason: number, delay?: number): boolean;
};

/** Team scoreboard scores (cs_team_manager entities, CTeam.m_iScore + notifyStateChanged). team is
 *  0..3 (Unassigned/Spectator/T/CT), matched by m_iTeamNum; entities are re-found per call. */
export declare const Teams: {
  getScore(team: number): number | null;
  setScore(team: number, score: number): boolean;
  addScore(team: number, delta: number): boolean;
};

/** CS2 round-end reasons (CCSGameRules::TerminateRound / round_end.reason). Binary-validated against
 *  our build (reason bound = 22; #SFUI_Notice_* switch). Gaps 2/3/15 are removed legacy VIP reasons. */
export declare const RoundEndReason: {
  readonly Unknown: 0; readonly TargetBombed: 1; readonly TerroristsEscaped: 4;
  readonly CTsPreventEscape: 5; readonly EscapingTerroristsNeutralized: 6; readonly BombDefused: 7;
  readonly CTsWin: 8; readonly TerroristsWin: 9; readonly RoundDraw: 10;
  readonly AllHostagesRescued: 11; readonly TargetSaved: 12; readonly HostagesNotRescued: 13;
  readonly TerroristsNotEscaped: 14; readonly GameCommencing: 16; readonly TerroristsSurrender: 17;
  readonly CTsSurrender: 18; readonly TerroristsPlanted: 19; readonly CTsReachedHostage: 20;
  readonly SurvivalWin: 21; readonly SurvivalDraw: 22;
};

/** cs_win_panel_round final_event values (validated at the live gate against a natural round end). */
export declare const WinPanelFinalEvent: { readonly CTsWin: 2; readonly TerroristsWin: 3 };
```

- [ ] **Step 6 — Changeset (packages/* changed here, so the changeset rides THIS PR).** Create `.changeset/round-control-cs2.md`:

```md
---
"@s2script/cs2": minor
---

Round control: GameRules.terminateRound(reason, delay?) (sig-resolved CCSGameRules::TerminateRound,
deferred one frame so round_end reaches every plugin), round-clock write surface
(setRoundTime/setTimeRemaining/addTimeRemaining + roundStartTime/timeElapsed/timeRemaining reads),
Teams score API (cs_team_manager CTeam.m_iScore), and the RoundEndReason/WinPanelFinalEvent const maps.
```

- [ ] **Step 7 — Gates + commit (PR2).**

```bash
./scripts/check-plugins-typecheck.sh
make check-boundary
cargo test -p s2script-core
```
Expected: all green (the .d.ts additions are additive; existing plugins still typecheck). Commit:

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts .changeset/round-control-cs2.md
git commit -m "feat(cs2): round-control surface — terminateRound, round-clock writers, Teams scores, RoundEndReason"
```

---

## Task 3 (PR3): round-control-demo + live gate

**Files:**
- Create: `examples/round-control-demo/package.json`, `examples/round-control-demo/tsconfig.json`, `examples/round-control-demo/src/plugin.ts`
- Modify: `docs/PROGRESS.md` (finished-slice entry + deferred items with reasons)

**Steps:**

- [ ] **Step 1 — Demo plugin.** `examples/round-control-demo/package.json`:

```json
{
  "name": "@demo/round-control-demo",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x"
  }
}
```

`examples/round-control-demo/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/globals/globals.d.ts"]
}
```

`examples/round-control-demo/src/plugin.ts`:

```ts
// @demo/round-control-demo — live gate for the round-control slice.
//
//   sm_endround [reason] [delay]  — GameRules.terminateRound; the round_end logger below then proves
//                                   BOTH the deferred drain (our own handler fires even though WE
//                                   terminated from a JS dispatch) AND the closed-loop reason
//                                   read-back (engine round_end.reason === the reason we passed).
//   sm_settime <sec>              — setTimeRemaining + read-back (HUD clock repaint = human-visual).
//   sm_addtime <sec>              — addTimeRemaining + read-back.
//   sm_teamscore <team> <score>   — Teams.setScore + read-back (scoreboard visual = human).
//   sm_winpanel [2|3]             — synthetic cs_win_panel_round. NOTE: a JS-fired event never
//                                   re-dispatches to JS subscribers (isolate-borrow rule), so our own
//                                   cs_win_panel_round logger will NOT log this fire — expected; the
//                                   client-visible panel is the check.
//
// round_start logs roundStartTime/timeElapsed sanity (roundStartTime ~= gameTime, elapsed ~= 0);
// a NATURAL round end (mp_ignore_round_win_conditions 0, timer expiry) validates the shipped
// RoundEndReason/WinPanelFinalEvent values against engine-emitted events.

import { Commands } from "@s2script/sdk/commands";
import { Events } from "@s2script/sdk/events";
import { Server } from "@s2script/sdk/server";
import { GameRules, Teams, RoundEndReason, WinPanelFinalEvent } from "@s2script/cs2";

let lastTerminateReason: number | null = null;

export function onLoad(): void {
  Events.on("round_end", (e) => {
    const reason = e.getInt("reason");
    const winner = e.getInt("winner");
    const ours = lastTerminateReason !== null;
    const loop = ours ? (reason === lastTerminateReason ? " [OURS — closed-loop OK]" : ` [OURS — MISMATCH, sent ${lastTerminateReason}]`) : "";
    console.log(`[round-demo] round_end reason=${reason} winner=${winner}${loop}`);
    lastTerminateReason = null;
  });

  Events.on("cs_win_panel_round", (e) => {
    console.log(`[round-demo] cs_win_panel_round final_event=${e.getInt("final_event")} (expect ${WinPanelFinalEvent.CTsWin}=CT / ${WinPanelFinalEvent.TerroristsWin}=T on a natural end)`);
  });

  Events.on("round_start", () => {
    const gr = GameRules.get();
    if (!gr) { console.log("[round-demo] round_start: no gamerules proxy"); return; }
    console.log(`[round-demo] round_start roundTime=${gr.roundTime} roundStartTime=${gr.roundStartTime} gameTime=${Server.gameTime} timeElapsed=${gr.timeElapsed} timeRemaining=${gr.timeRemaining}`);
  });

  Commands.register("endround", (ctx) => {
    const reason = ctx.argInt(0, RoundEndReason.TerroristsWin);
    const delay = ctx.argInt(1, 5);
    lastTerminateReason = reason;
    const ok = GameRules.terminateRound(reason, delay);
    if (!ok) lastTerminateReason = null;
    ctx.reply(`endround reason=${reason} delay=${delay} queued=${ok} (round_end log follows next frame if queued)`);
  });

  Commands.register("settime", (ctx) => {
    const sec = ctx.argInt(0, 60);
    const gr = GameRules.get();
    const ok = gr ? gr.setTimeRemaining(sec) : false;
    ctx.reply(`settime ${sec}: ok=${ok} roundTime=${gr ? gr.roundTime : null} timeRemaining=${gr ? gr.timeRemaining : null}`);
  });

  Commands.register("addtime", (ctx) => {
    const sec = ctx.argInt(0, 30);
    const gr = GameRules.get();
    const ok = gr ? gr.addTimeRemaining(sec) : false;
    ctx.reply(`addtime ${sec}: ok=${ok} roundTime=${gr ? gr.roundTime : null} timeRemaining=${gr ? gr.timeRemaining : null}`);
  });

  Commands.register("teamscore", (ctx) => {
    const team = ctx.argInt(0, 2);
    const score = ctx.argInt(1, 10);
    const ok = Teams.setScore(team, score);
    ctx.reply(`teamscore team=${team} -> ${score}: ok=${ok} readback=${Teams.getScore(team)}`);
  });

  Commands.register("winpanel", (ctx) => {
    const fe = ctx.argInt(0, WinPanelFinalEvent.TerroristsWin);
    const fired = Events.fire("cs_win_panel_round", { final_event: fe }, false);
    ctx.reply(`winpanel final_event=${fe} fired=${fired} (client-visible panel is the check; our own JS logger will NOT fire — expected)`);
  });

  console.log("[round-demo] onLoad — sm_endround / sm_settime / sm_addtime / sm_teamscore / sm_winpanel registered");
}
```

- [ ] **Step 2 — Build + typecheck gates.**

```bash
npx s2script build   # from examples/round-control-demo/ (or node packages/cli/dist/cli.js build examples/round-control-demo)
./scripts/check-plugins-typecheck.sh
```
Expected: `dist/round-control-demo.s2sp` produced; typecheck gate green.

- [ ] **Step 3 — Sniper build + deploy.**

```bash
git submodule update --init --recursive
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
make package   # dist pawn.js is a CONCAT — never raw-cp games/cs2/js/pawn.js
# deploy the 2 .so + gamedata + the packaged addon + the demo .s2sp into dist/addons/s2script, then:
docker compose -f docker/docker-compose.yml restart cs2   # NOT --force-recreate (resets gameinfo.gi)
```
Expected: shim GLIBC ≤ 2.14 / core ≤ 2.30; server boots.

- [ ] **Step 4 — Live gate (per spec §7).** Drive via `python3 scripts/rcon.py "<cmd>"`:
  1. Boot log shows `gamedata OK    TerminateRound` AND `gamedata OK    TerminateRound.scope-string` and `GAMEDATA VALIDATION: N ok, 0 FAILED`; demo `onLoad` line present.
  2. `sm_endround 9` → reply `queued=true`; round visibly ends; console shows `[round-demo] round_end reason=9 ... [OURS — closed-loop OK]` (deferred-drain + enum read-back proof — STOP per spec §8 if the round ends but our logger does not fire). Repeat `sm_endround 8`.
  3. `mp_ignore_round_win_conditions 0` + let the round timer expire (use `sm_settime 15` to shorten) → log engine-emitted `round_end reason` (expect 10 RoundDraw or the mode's timeout reason) + `cs_win_panel_round final_event` → record against the shipped consts.
  4. `sm_settime 30` → `ok=true`, read-back changes, round ends ~30s later. HUD clock repaint = human-visual (append to the deferred-live-tests memory if no human joins — STOP-investigate per spec §8 if read-back succeeds but the clock never repaints for a human).
  5. `sm_teamscore 2 15` → `ok=true readback=15` (scoreboard visual = human criterion).
  6. `round_start` log sanity: `roundStartTime ≈ gameTime`, `timeElapsed ≈ 0`.
  7. Soak: `sm_endround` twice within one frame window is impractical by hand — instead run 3 terminate cycles back-to-back + one `changelevel` and confirm no crash and the proxy cache self-heals (`sm_settime` still works post-map-change).

- [ ] **Step 5 — PROGRESS.md + commit (PR3).** Append the finished-slice entry to `docs/PROGRESS.md` (what was built, the live-gate result, and the DEFERRED items with reasons: player respawn = fresh RE, not needed for round 1; sync cvar write = feat/writeconvar's slice; CCSTeam codegen = curated Teams API covers the consumer; freeze/warmup setters + round-state-machine abstraction = no consumer). Commit:

```bash
git add examples/round-control-demo docs/PROGRESS.md
git commit -m "demo(round): round-control live-gate demo + progress entry"
```

- [ ] **Step 6 — Stack + PRs.** Rebase the stack onto current main before the live gate re-run if main moved (re-tail the ABI append if `feat/writeconvar` landed — update the C header, Rust struct, and BOTH test literals in the same commit). Open the 3-PR Graphite stack: PR1 (Task 1) → PR2 (Task 2) → PR3 (Task 3).

---

## Self-review notes

- **Spec coverage:** §2.1 sig + semantic gate → Task 1 Steps 1-2, 6, 8; §2.2 enums → Task 2 Step 3 + Task 3 gate items 2-3; §2.3 reuse surface → Task 2 Steps 1-3; §3 API → Task 2 (shapes match the spec verbatim); §4.1 deferral → Task 1 Steps 7-8 + gate item 2; §4.2 ABI → Task 1 Steps 3-4 + Global Constraints; §4.3 cvar decision → demo comment + gate item 3 (no code — decided out); §4.4 Plan B → degrade paths in Task 1 + PROGRESS notes; §5 boundary → Task 1 Step 9 gates; §6 deferred → Task 3 Step 5; §7 live gate → Task 3 Step 4 (numbered 1:1); §8 STOP conditions → wired into gate items 1, 2, 4.
- **Type consistency:** op signature `(int idx, int serial, int rules_ptr_off, float delay, int reason) -> int` is identical across the C typedef (Task 1 Step 3), Rust typedef (Step 4), native (Step 5), and shim impl (Step 7); JS call sites pass `(index, serial, p[0], delay, reason)` in that order (Task 2 Step 1); `terminateRound(reason, delay?)` reason-first at the JS surface is intentional (ergonomics) and flips to delay-first only inside the native→op boundary — the pawn.js call passes args positionally to the native whose order is (idx, serial, off, delay, reason), matching.
- **Placeholder scan:** no TBDs; every code block is complete and paste-able; line anchors are approximate ("~line N") with structural anchors (named functions/fields) as the authoritative locator.
- **Known live-resolution items** (expected, not placeholders): natural-round-end `reason`/`final_event` observed values (gate item 3) and the HUD-repaint human check (gate item 4) are validation outcomes, not implementation inputs.
