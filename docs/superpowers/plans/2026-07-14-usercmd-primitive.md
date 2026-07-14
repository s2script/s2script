# Usercmd Primitive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Task 1 is a live RE spike — do it INLINE/interactively (like the ChangeTeam slice), not via a subagent; it resolves the constants Tasks 3-4 depend on.**

**Goal:** A SourceMod `OnPlayerRunCmd`-parity primitive: `@s2script/usercmd`'s `UserCmd.onRun(handler)` runs each tick with a block-scoped `Cmd` that reads/modifies a player's input (buttons, view angles, forward/side/up move, impulse) and can block it — unblocking input-based movement styles.

**Architecture:** Detour `CCSPlayerController::ProcessUsercmds` (reuse `shim/src/detour.cpp`, the 6.6 damage-hook engine); each `CUserCmd` wraps a `CSGOUserCmdPB` protobuf at offset `0x10`, read/modified by protobuf reflection (reuse the usermessage `libprotobuf.a` path); a core `USERCMD_MUX` + synchronous `dispatch_usercmd` (mirror `DAMAGE_MUX`/`dispatch_damage`) runs the JS subscribers with `HookResult` collapse. Engine-generic core + module; CS2 hook fn + offsets live in shim/gamedata.

**Tech Stack:** Rust core (`v8`), C++ shim (Metamod, `detour.cpp`, `libprotobuf.a` reflection), gamedata JSONC signatures, a types-only `@s2script/usercmd` package.

## Global Constraints

- **Reuse, don't reinvent:** the detour engine is `shim/src/detour.cpp` (`s2detour::Install(target, handler, &origTramp)` / `RemoveAll()`); protobuf reflection is the usermessage path (`GetDescriptor()`/`GetReflection()`/`FindFieldByName`/`cpp_type()` switch, `is_repeated()` guard). No new mechanisms.
- **RE doctrine:** the `ProcessUsercmds` signature is SELF-RESOLVED on our `libserver.so` (hint from Swiftly `plugin_files/gamedata/cs2/core/signatures.json` — `CCSPlayer_MovementServices_ProcessUserCmd` linux `55 48 89 E5 41 57 41 56 41 55 41 54 53 48 89 FB 48 83 EC ? 89 4D`) and boot-validated via `ResolveSigValidated` + `GamedataResult` — never a borrowed constant. `resolve:"direct"`.
- **ABI discipline:** new `S2EngineOps` ops are APPENDED at the STRUCT TAIL, after `sound_precache_add` (the current last field), byte-identical across ALL five touchpoints: the C header typedef+struct (`shim/include/s2script_core.h`), the Rust mirror typedef+struct (`core/src/v8host.rs`), BOTH in-test `S2EngineOps{...}` literals in the Rust test module, and the shim `ops.<field> = &fn;` table. Verify field-count parity (C header `s2_*_fn` count == Rust `pub` field count) after wiring.
- **Boundary (charter):** core stays engine-generic — no CS2 type names or CS2-specific field strings baked into `core/src`. `CBaseUserCmdPB` field names (`forwardmove`/`leftmove`/`upmove`/`impulse`/`viewangles`/`buttons_pb`) are Source2-shared, so the field enum + the `@s2script/usercmd` module are engine-generic; the CS2 hook fn + the `CUserCmd`/`CSGOUserCmdPB` offsets live only in shim/gamedata. Both `scripts/check-core-boundary.sh` and the shim-owns-CS2 rule stay green.
- **Never a raw pointer to JS:** only `(slot, cmdIndex)` cross to core; the block-scoped `Cmd` reads/writes through ops that operate on a shim-side `s_currentUserCmd` (valid only during dispatch, exactly like `s_currentDamageInfo`).
- **Degrade-never-crash:** every native `catch_unwind`s; `dispatch_usercmd` is `try_borrow_mut`-guarded; every reflection `Set*` is `is_repeated()`/`cpp_type()`-guarded (an `is_repeated` scalar `Set*` is a protobuf `GOOGLE_LOG(FATAL)` process-abort); unresolved sig → the detour is never installed → `UserCmd.onRun` is a silent no-op.
- **Lazy install:** `ProcessUsercmds` fires every tick per player — the detour installs LAZILY on the FIRST `UserCmd.onRun` subscribe (via a `usercmd_hook_install` op, mirroring `entity_listener_install`), so zero overhead when no plugin subscribes.
- **Live gate needs a human client** (bots don't drive this path testably) — same ceiling as SayText2/damage.

---

## File Structure

- `gamedata/core.gamedata.jsonc` — add the `ProcessUsercmds` signature (Task 1).
- `shim/src/s2script_mm.cpp` — the detour handler, `s_currentUserCmd`, the reflection ops, lazy install, Load-time sig-resolve, `RemoveAll` on unload (Tasks 1, 3).
- `shim/include/s2script_core.h` — the op typedef decls + struct fields (Task 3).
- `core/src/v8host.rs` — `USERCMD_MUX`, `dispatch_usercmd`, the op typedefs/struct/literals, the `__s2_usercmd_*` natives, the `@s2script/usercmd` prelude module + s2require registration (Tasks 2, 4).
- `core/src/ffi.rs` — `s2script_core_dispatch_usercmd` FFI export (Task 2).
- `packages/usercmd/{package.json,index.d.ts}` — the types-only module (Task 4).
- `examples/usercmd-demo/` — the live-gate demo (Task 5).

---

## Task 1: Feasibility spike (INLINE — resolve the unknowns)

**Goal:** prove the mechanism on the live binary and resolve the three constants the productionization needs: (a) the exact hook function, (b) `sizeof(CUserCmd)` (the array stride), (c) whether `subtick_moves` must be cleared for a coarse `forwardmove` write to take effect.

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (add the `ProcessUsercmds` sig)
- Modify: `shim/src/s2script_mm.cpp` (a temporary self-contained detour + logging)

**Steps:**

- [ ] **Step 1 — Self-resolve the signature.** Write a scratch Python byte-scanner (like the ChangeTeam slice's) over `docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so`'s PF_X range for the Swiftly hint pattern `55 48 89 E5 41 57 41 56 41 55 41 54 53 48 89 FB 48 83 EC ? 89 4D`. Confirm it is UNIQUE (exactly 1 hit). If not unique or absent, RTTI-walk `CCSPlayerController`'s vtable / xref a `ProcessUsercmds`-adjacent string to locate it (the ChangeTeam CTMDBG-xref technique). Record the confirmed pattern.

- [ ] **Step 2 — Add the gamedata sig.** In `gamedata/core.gamedata.jsonc`'s `signatures` block, add (with a doc comment noting the self-resolve + Swiftly hint + the ABI `(this, CUserCmd* cmds, int numcmds, bool paused, float margin)`):
```jsonc
"ProcessUsercmds": {
  "linuxsteamrt64": { "module": "libserver.so", "pattern": "<confirmed pattern>", "resolve": "direct" }
},
```

- [ ] **Step 3 — Temporary spike detour.** In `s2script_mm.cpp`, sig-resolve + `s2detour::Install` a temporary handler:
```cpp
typedef void (*ProcessUsercmds_t)(void* thisptr, void* cmds, int numcmds, bool paused, float margin);
static ProcessUsercmds_t g_origProcessUsercmds = nullptr;
// Swiftly's CUserCmd layout: char pad0[0x10]; CSGOUserCmdPB cmd; char pad1[0x38];
// SPIKE: try stride candidates; log which one yields a valid protobuf for cmd[1].
static void Spike_ProcessUsercmds(void* thisptr, void* cmds, int numcmds, bool paused, float margin) {
    for (int i = 0; i < numcmds && i < 4; i++) {
        // cmd i's protobuf message at cmds + i*stride + 0x10; start with stride from Swiftly's sizeof.
        auto* m = reinterpret_cast<google::protobuf::Message*>(reinterpret_cast<char*>(cmds) + i * S2_USERCMD_STRIDE + 0x10);
        const auto* d = m->GetDescriptor();
        const auto* r = m->GetReflection();
        const auto* baseF = d ? d->FindFieldByName("base") : nullptr;
        if (baseF) {
            auto* base = r->MutableMessage(m, baseF);
            const auto* bd = base->GetDescriptor(); const auto* br = base->GetReflection();
            const auto* fwd = bd->FindFieldByName("forwardmove");
            if (fwd) META_CONPRINTF("[s2script] SPIKE usercmd[%d] fwd=%.1f desc=%s\n", i, br->GetFloat(*base, fwd), d->full_name().c_str());
        } else if (d) {
            META_CONPRINTF("[s2script] SPIKE usercmd[%d] desc=%s (no 'base' field)\n", i, d->full_name().c_str());
        }
    }
    g_origProcessUsercmds(thisptr, cmds, numcmds, paused, margin);
}
```
Resolve `S2_USERCMD_STRIDE` empirically (Step 5). Install after the sig resolves, guarded like `s_pCommitSuicide`.

- [ ] **Step 4 — Read proof (live, human).** Sniper build, deploy, join as a human on any map, move around. Confirm the log prints a live, sane `forwardmove` (≈ ±450 while pressing W/S, 0 idle) and `desc=CSGOUserCmdPB` — proving the hook fires, the offset `0x10` is right, and reflection reads the real input.

- [ ] **Step 5 — Confirm the stride.** In the spike handler, when `numcmds >= 2`, verify `cmd[1]`'s message is ALSO a valid `CSGOUserCmdPB` with a sane `forwardmove` (a wrong stride yields garbage/crash). If garbage, bisect `S2_USERCMD_STRIDE` (start from Swiftly's `0x10 + sizeof(CSGOUserCmdPB) + 0x38`; the vendored protobuf 3.21.8 makes this deterministic — measure by finding the stride at which cmd[1] parses). Record the confirmed stride constant.

- [ ] **Step 6 — Modify + subtick answer (live, human).** Extend the spike to, on a temporary condition (e.g. a chat command or a fixed tick), set `forwardmove = 0` (and buttons to force `IN_JUMP` = bit for jump) BEFORE calling `g_origProcessUsercmds`. Move as a human and observe: does zeroing `forwardmove` stop forward movement? If NOT, also clear `subtick_moves` (`br->ClearField(base, bd->FindFieldByName("subtick_moves"))`) and re-test. **Record: does a coarse write alone take effect, or is a subtick clear required?** This is the contract input for Task 4/5.

- [ ] **Step 7 — Commit the resolved facts.** Revert the temporary spike handler. Commit ONLY the gamedata sig + a findings note appended to the spec (`## Spike findings` section: confirmed sig, hook fn identity, `S2_USERCMD_STRIDE`, subtick verdict). These constants feed Tasks 3-4.

```bash
git add gamedata/core.gamedata.jsonc docs/superpowers/specs/2026-07-14-usercmd-primitive-design.md
git commit -m "spike(usercmd): resolve ProcessUsercmds sig + CUserCmd stride + subtick verdict"
```

---

## Task 2: Core mux + dispatch + FFI + subscribe native

**Files:**
- Modify: `core/src/v8host.rs` (USERCMD_MUX, dispatch_usercmd, subscribe native, block/read/write natives declared here but wired to ops in Task 3)
- Modify: `core/src/ffi.rs` (the FFI export)
- Test: `core/src/v8host.rs` `#[cfg(test)]` module

**Interfaces:**
- Produces: `pub(crate) fn dispatch_usercmd(slot: i32) -> i32` (returns the collapsed `HookResult` as an int: 0 Continue … 3 Stop); `#[no_mangle] pub extern "C" fn s2script_core_dispatch_usercmd(slot: c_int) -> c_int`; the native `__s2_usercmd_subscribe(handler)` that registers into `USERCMD_MUX` under key `"onRun"` and calls the (Task 3) `usercmd_hook_install` op on the first sub.
- Consumes: `crate::event_mux::EventMux`, `run_chain` (the HookResult collapse used by `dispatch_damage`).

- [ ] **Step 1 — Failing test: dispatch runs a subscriber + collapses HookResult.** Mirror the `dispatch_damage` test. Register a JS `UserCmd.onRun` handler that pushes to a capture buffer and returns `HookResult.Handled`; assert `dispatch_usercmd(3)` runs it, the ctx `slot === 3`, and the returned int is `2` (Handled). Assert a second `dispatch_usercmd` with no subs returns `0` and does not throw. Run: `cargo test -p s2script-core usercmd_dispatch`; Expected: FAIL (undefined).

- [ ] **Step 2 — Implement `USERCMD_MUX` + `dispatch_usercmd`.** Add a `thread_local USERCMD_MUX: RefCell<EventMux<v8::Global<v8::Function>>>` beside `DAMAGE_MUX`. `dispatch_usercmd(slot)` mirrors `dispatch_damage`: snapshot `"onRun"`, `try_borrow_mut`-guard, for each sub build the `Cmd` object (a JS object whose accessors call the `__s2_usercmd_*` natives — defined in the prelude, Task 4) + a ctx `{slot}`, `catch_unwind` per sub, collect the return via `run_chain` → return the collapsed HookResult int. Reset `USERCMD_MUX` in `shutdown`; `remove_by_owner` on unload (add to the unload path beside the other muxes).

- [ ] **Step 3 — FFI export.** In `ffi.rs`, beside `s2script_core_dispatch_damage`:
```rust
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_usercmd(slot: c_int) -> c_int {
    catch_unwind(|| v8host::dispatch_usercmd(slot)).unwrap_or(0)
}
```

- [ ] **Step 4 — Subscribe native.** `__s2_usercmd_subscribe(handler)` registers the handler in `USERCMD_MUX` (owner/generation from the current plugin, like `DAMAGE_MUX.subscribe`) and, if this is the first-ever sub, calls the `usercmd_hook_install` engine op (Option, no-op if absent). Register via `set_native`.

- [ ] **Step 5 — Run tests / commit.** `cargo test -p s2script-core usercmd` green. Commit core mux/dispatch/ffi.

---

## Task 3: The ABI ops (C header + Rust mirror + shim impl) + detour + reflection

**Files:**
- Modify: `shim/include/s2script_core.h` (typedefs + struct fields, tail)
- Modify: `core/src/v8host.rs` (Rust typedefs + struct fields + BOTH test literals; the read/write/clear/install natives calling the ops)
- Modify: `shim/src/s2script_mm.cpp` (the ops impl + the real detour handler + lazy install + Load resolve + unload RemoveAll)

**Interfaces (the op set — APPEND after `sound_precache_add`, in this order):**
```c
/* usercmd slice — APPENDED after sound_precache_add; order is the ABI. All operate on the shim's
 * s_currentUserCmd (the in-flight cmd's CSGOUserCmdPB); valid only during a usercmd dispatch. */
typedef int   (*s2_usercmd_hook_install_fn)(void);              /* lazily install the ProcessUsercmds detour; 1 ok / 0 unresolved */
typedef double(*s2_usercmd_read_fn)(int field);                 /* field: 0 fwd,1 side(left),2 up,3 pitch,4 yaw,5 roll,6 impulse */
typedef void  (*s2_usercmd_write_fn)(int field, double value);
typedef uint64_t (*s2_usercmd_read_buttons_fn)(void);           /* base.buttons_pb.buttonstate1 */
typedef void  (*s2_usercmd_write_buttons_fn)(uint64_t mask);
typedef void  (*s2_usercmd_clear_subtick_fn)(void);             /* clear base.subtick_moves */
```
The Rust mirror types + `pub` fields mirror these names/order exactly; the shim `ops.usercmd_* = &s2_usercmd_*;` assignments follow the same order after `ops.sound_precache_add`.

- [ ] **Step 1 — C header.** After the `sound_precache_add` typedef + struct field, append the six typedefs above + the six `S2EngineOps` fields (`usercmd_hook_install`, `usercmd_read`, `usercmd_write`, `usercmd_read_buttons`, `usercmd_write_buttons`, `usercmd_clear_subtick`), with the `APPENDED after sound_precache_add` comment.

- [ ] **Step 2 — Rust mirror.** In `v8host.rs`, append the six typedefs (e.g. `type UsercmdReadFn = extern "C" fn(c_int) -> f64;` …) after `SoundPrecacheAddFn`, the six `pub` struct fields after `sound_precache_add`, and `<field>: None,` after `sound_precache_add: None,` in BOTH full `S2EngineOps{...}` test literals (the `init` schema-test literal AND `mock_event_ops`). Verify parity: `grep -c 's2_.*_fn ' header` == Rust `pub` field count.

- [ ] **Step 3 — Shim reflection ops.** Implement the field-name navigation once (a helper `s2_usercmd_base()` → `MutableMessage(s_currentUserCmd, FindFieldByName("base"))`), then:
  - `s2_usercmd_read(field)`: map `field`→a `base` leaf name (`forwardmove`/`leftmove`/`upmove`/`impulse`) or a `base.viewangles` axis (`x`/`y`/`z` for pitch/yaw/roll); reflect + return as double. Null/guarded → 0.
  - `s2_usercmd_write(field, value)`: same mapping; `is_repeated()`/`cpp_type()`-guarded `Set*` (Float for move/angles, Int32 for impulse). Null-guarded no-op.
  - `s2_usercmd_read_buttons`/`write_buttons`: navigate `base.buttons_pb`, get/set `buttonstate1` (UInt64).
  - `s2_usercmd_clear_subtick`: `base` reflection `ClearField(base, FindFieldByName("subtick_moves"))`.
  - Each guards `!s_currentUserCmd` → 0/no-op.

- [ ] **Step 4 — The real detour handler.** Replace the spike handler with the production one:
```cpp
static int64_t Detour_ProcessUsercmds(void* thisptr, void* cmds, int numcmds, bool paused, float margin) {
    int slot = /* controller slot from thisptr: CEntityInstance::GetRefEHandle entry index - 1, like Swiftly */;
    for (int i = 0; i < numcmds; i++) {
        s_currentUserCmd = reinterpret_cast<google::protobuf::Message*>(
            reinterpret_cast<char*>(cmds) + i * S2_USERCMD_STRIDE + 0x10);
        int res = s2script_core_dispatch_usercmd(slot);   // JS reads/modifies in place
        if (res >= 2 /*Handled*/) {                       // block == neutralize this cmd
            s2_usercmd_write(0,0); s2_usercmd_write(1,0); s2_usercmd_write(2,0);
            s2_usercmd_write_buttons(0);
            if (S2_SUBTICK_CLEAR_ON_BLOCK) s2_usercmd_clear_subtick();
        }
        s_currentUserCmd = nullptr;
    }
    return reinterpret_cast<ProcessUsercmds_t2>(g_origProcessUsercmds)(thisptr, cmds, numcmds, paused, margin);
}
```
(Return type per the spike; if the fn returns void, drop the return. `S2_SUBTICK_CLEAR_ON_BLOCK` + auto-subtick-clear-on-move-write per the spike verdict — if the spike found coarse writes need a subtick clear, `s2_usercmd_write(0/1/2, …)` also clears subtick.)

- [ ] **Step 5 — Lazy install + Load resolve + unload.** `s2_usercmd_hook_install()`: resolve `s_pProcessUsercmds` (sig-resolved at Load into a static, like `s_pCommitSuicide`) and `s2detour::Install(s_pProcessUsercmds, (void*)&Detour_ProcessUsercmds, (void**)&g_origProcessUsercmds)` once (idempotent flag); return 1 on success / 0 if the sig is unresolved. At Load, resolve the sig via `ResolveSigValidated("ProcessUsercmds", …)` + `GamedataResult` (boot banner) but do NOT install (lazy). Wire `ops.usercmd_* = &s2_usercmd_*`. `s2detour::RemoveAll()` already runs on unload (shared) — confirm it covers this detour.

- [ ] **Step 6 — Degrade tests (core).** In-isolate: with no ops, `__s2_usercmd_read(0)` → 0, `__s2_usercmd_write`/`__s2_usercmd_subscribe` no-throw, `UserCmd.onRun` registers without the install op present (returns cleanly). `cargo test -p s2script-core usercmd` green.

- [ ] **Step 7 — Commit.** `git commit -m "feat(usercmd): ProcessUsercmds detour + protobuf-reflection read/write/block ops"`

---

## Task 4: `@s2script/usercmd` module (prelude JS + types + package)

**Files:**
- Create: `packages/usercmd/package.json`, `packages/usercmd/index.d.ts`
- Modify: `core/src/v8host.rs` (the prelude JS registering `__s2pkg_usercmd`; s2require already maps `@s2script/<name>`→`__s2pkg_<name>` generically — no core list edit)

**Interfaces:**
- Produces: `globalThis.__s2pkg_usercmd = { UserCmd, HookResult }`. `UserCmd.onRun(handler)` → `__s2_usercmd_subscribe(wrapped)` where `wrapped` builds the block-scoped `Cmd` + `{slot}` ctx and returns the handler's HookResult (or 0). `Cmd` getters/setters call `__s2_usercmd_read/write(field)`, `__s2_usercmd_read_buttons/write_buttons`, `__s2_usercmd_clear_subtick`. `viewAngles` get returns a `{x,y,z}` (from fields 3/4/5); set writes all three. `buttons` is a `bigint`.

- [ ] **Step 1 — Prelude JS.** In the prelude (beside `__s2pkg_damage`), define the `Cmd` prototype (accessors over the natives), `UserCmd.onRun`, and `__s2pkg_usercmd`. `HookResult` re-exported from the existing prelude global (as `@s2script/damage`/`@s2script/events` do). The `Cmd` is constructed fresh per dispatch inside `dispatch_usercmd`'s per-sub loop (Task 2 builds it, calling into this prelude constructor) OR the prelude exposes a singleton `Cmd` reading the current s_currentUserCmd — choose the singleton (cheaper; valid only during dispatch, like DamageInfo).

- [ ] **Step 2 — `packages/usercmd`.** `package.json` (types-only, mirror `packages/damage/package.json`) + `index.d.ts` (the `Cmd` interface + `UserCmd.onRun` from the spec §5, `buttons: bigint`, `viewAngles: QAngle` importing `@s2script/math`).

- [ ] **Step 3 — Typecheck gate.** `bash scripts/check-plugins-typecheck.sh` still green (the new package resolves via `paths`). Commit.

---

## Task 5: `usercmd-demo` + live gate (human client)

**Files:**
- Create: `examples/usercmd-demo/{package.json,tsconfig.json,src/plugin.ts}`

- [ ] **Step 1 — Demo.** `UserCmd.onRun((cmd, { slot }) => { … })`: log `slot`, `cmd.forwardMove`, `cmd.buttons` on a throttle (read proof); expose a `force` command (registered via `@s2script/commands`) that, while active for a slot, sets `cmd.forwardMove = 0` + forces `IN_JUMP` (modify proof) or returns `HookResult.Handled` (block proof).

- [ ] **Step 2 — Build + boundary + typecheck gates.** `node packages/cli/dist/cli.js build examples/usercmd-demo`; `bash scripts/check-core-boundary.sh` (green — core has no CS2 strings); `cargo test -p s2script-core` (all green).

- [ ] **Step 3 — Sniper build + deploy.** `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` (init submodules first: `git submodule update --init --recursive`). Verify GLIBC floors (shim ≤2.14, core ≤2.30). Deploy the 2 `.so` + gamedata + pawn.js + the demo `.s2sp` to `/home/gkh/projects/s2script/dist/addons/s2script`; `docker compose restart cs2`.

- [ ] **Step 4 — Live gate (human).** Boot markers: `gamedata OK ProcessUsercmds`, `GAMEDATA VALIDATION: N ok, 0 FAILED`, demo `onLoad`. Join as a human: confirm the read log shows live `forwardMove`/`buttons`; toggle `force` and confirm the modify/block visibly changes movement; server ticking, no crash. (Bots can't drive this — human-client gate.)

- [ ] **Step 5 — Commit + PR.** Commit the demo; add a `@s2script/usercmd` changeset (new package). Rebase onto current main (resolve any ABI-tail conflict — re-append usercmd ops after whatever new tail exists). Open the PR.

---

## Self-review notes

- **Spec coverage:** §3 mechanism → Tasks 1,3,4; §4 boundary → Global Constraints + Task 3 Step 2 (parity) + Task 5 Step 2 (boundary gate); §5 API → Task 4; §6 subtick spike → Task 1 Steps 6-7 (and its verdict feeds Task 3 Step 4 / Task 4); §7 safety → Global Constraints + Task 2 (try_borrow_mut) + Task 3 (is_repeated guard, no raw ptr); §8 live gate → Task 5.
- **Type consistency:** the field enum (0 fwd,1 side,2 up,3 pitch,4 yaw,5 roll,6 impulse) is used identically in Task 3 (shim ops) and Task 4 (prelude accessors); `buttons` is `bigint`/`uint64` end-to-end; `dispatch_usercmd(slot)->i32 HookResult` consistent across Task 2 (core) and Task 3 (shim detour caller).
- **The spike (Task 1) gates Tasks 3-4** on three constants (hook-fn identity, `S2_USERCMD_STRIDE`, `S2_SUBTICK_CLEAR_ON_BLOCK`/auto-clear-on-write); after the spike, fill those into the Task 3/4 code before dispatching subagents for those tasks.
