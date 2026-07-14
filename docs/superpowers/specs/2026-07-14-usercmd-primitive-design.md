# Usercmd primitive — design spec

**Date:** 2026-07-14
**Status:** design (approved) → implementation plan next
**Scope:** s2script core (core + shim + gamedata + a new engine-generic `@s2script/usercmd` module)

## 1. Goal

A SourceMod `OnPlayerRunCmd`-equivalent: let a plugin **read, modify, and block** a player's per-tick input — buttons, view angles, movement (`forwardMove`/`sideMove`/`upMove`), and impulse — inside a hooked per-tick callback. This is the core primitive that unblocks **input-based movement styles** (sideways / backwards / W-only / half-sideways) in the surf-timer port, and generalizes to any input mod (auto-bhop, input remapping, anti-cheat input inspection, etc.).

Reading the *current* button/angle/velocity state is already possible today via schema polling (`m_nButtons`, `eyeAngles`, `absVelocity`) — the port does exactly this. The **new** capability this primitive adds is **intercepting and altering the input before the game processes it**, which the transient usercmd (not stored on any entity) is the only place to do.

## 2. Prior art — this is a solved problem

**Swiftly** (open-source CS2 framework) implements this in production and is the reference for the mechanism (do not reinvent it):

- It detours **`CCSPlayerController::ProcessUsercmds(this, CUserCmd* cmds, int numcmds, bool paused, float margin)`** (a `Pre` function hook) and exposes each usercmd to plugins as an `OnClientProcessUsercmds` event.
- Each **`CUserCmd` wraps a `CSGOUserCmdPB` protobuf at offset `0x10`** (Swiftly's layout: `char pad0[0x10]; CSGOUserCmdPB cmd; char pad1[0x38];`).
- It accesses the usercmd's fields by casting `&cmdList[i].cmd` to a `google::protobuf::Message*` and using **protobuf reflection** — no dependency on the coarse fields being special-cased.

The movement input lives on `base` (a `CBaseUserCmdPB`, from the Source2-shared `usercmd.proto`): `forwardmove`, `leftmove`, `upmove`, `viewangles` (`CMsgQAngle`), `impulse`, `buttons_pb` (`CInButtonStatePB` with `buttonstate1/2/3`), and `subtick_moves` (a repeated `CSubtickMoveStep` carrying per-press `analog_forward_delta`/`analog_left_delta` + a `when` fraction).

**s2script already has both mechanisms this needs**, from prior slices:
- `shim/src/detour.cpp` — the hand-rolled x86-64 inline-detour engine (built for the 6.6 `DispatchTraceAttack` damage hook). We reuse it verbatim to detour `ProcessUsercmds`.
- Protobuf reflection over the vendored self-contained `libprotobuf.a` — the usermessage slice (6.1c / gamerules-usermessages) already links it and reads/writes protobuf message fields by name via `FindFieldByName` + `GetReflection()`. The CUserCmd's `CSGOUserCmdPB` is just another live protobuf message; we get its pointer from the hook (offset `0x10`) and reflect it exactly the same way. **No generated `.pb.h` headers.**

So the RE risk is not "can this be done" (Swiftly proves yes) — it is narrowed to two concrete, spike-able unknowns (§6).

## 3. Mechanism

1. **Hook:** sig-resolve `CCSPlayerController::ProcessUsercmds` on our `libserver.so` (hint sig from Swiftly's `signatures.json` / ModSharp gamedata; self-resolved + boot-validated per the RE doctrine — never a borrowed constant). Install a `Pre` inline detour via `detour.cpp`. Fallback hook point if the controller entry is awkward on our build: `CCSPlayer_MovementServices::ProcessUserCmd` (single-cmd variant; Swiftly ships a sig for it too). The spike picks.
2. **Per-cmd access:** in the detour, capture `CUserCmd* cmds` (arg 1) + `numcmds` (arg 2). For each `i`, the protobuf message is at `((char*)&cmds[i]) + 0x10` as a `google::protobuf::Message*`. The array stride (`sizeof(CUserCmd)`) is resolved in the spike (Swiftly's layout gives `0x10 + sizeof(CSGOUserCmdPB) + 0x38`; because we don't generate the type, the stride is a validated constant — see §6/§8).
3. **Read/modify:** reflect the message → its `base` sub-message (`CBaseUserCmdPB`) → the scalar fields (`forwardmove`/`leftmove`/`upmove`/`impulse`), the `viewangles` sub-message (`CMsgQAngle`: `x`/`y`/`z`), and `buttons_pb` (`CInButtonStatePB`: `buttonstate1`). All via the existing reflection helpers — read = `GetReflection()->Get*`, write = `Set*`, guarded for `is_repeated()`/`cpp_type()` exactly as the usermessage setters are (an `is_repeated` scalar `Set*` is a `GOOGLE_LOG(FATAL)` — already guarded in our usermessage path).
4. **Dispatch:** the detour calls `s2script_core_dispatch_usercmd(controllerSlot, cmdIndex)` for each cmd → a core `USERCMD_MUX` → the JS `UserCmd.onRun` subscribers, run **synchronously under the isolate borrow** with the `try_borrow_mut` re-entrancy guard + `run_chain` `HookResult` collapse (the exact damage/event pre-hook pattern). A returned `HookResult ≥ Handled` suppresses the input (see §5). The subscribers read/modify the live message through block-scoped ops keyed to the current cmd.

## 4. Architecture & boundary (charter)

- **Engine-generic (core):** `CBaseUserCmdPB` and its fields come from the Source2-shared `usercmd.proto`; a "player's per-tick input" is a Source2 concept. So the `USERCMD_MUX`, `dispatch_usercmd`, the field get/set ops, and the `@s2script/usercmd` module are all engine-generic. **The ops key on a NUMERIC field enum** (`0 forwardMove, 1 sideMove, 2 upMove, 3 pitch, 4 yaw, 5 roll, 6 impulse`, plus `buttons`/`clearSubtick`) passed as a plain `int` — NOT a CS2/protobuf field string (unlike the usermessage `set_int(field_name,…)` ops). No CS2 identifier crosses the C ABI; the SHIM alone maps the enum to the `CBaseUserCmdPB`/`CMsgQAngle`/`CInButtonStatePB` nesting. The engine-generic guarantee rests on the fact that **these 7 fields, in this order, are Source2-universal** (`usercmd.proto`), not on protobuf names appearing in core (they never do). This is the entity-`kind`-dispatch precedent, and it's better for a per-tick hot path than string lookups.
- **CS2-only (shim):** the hook *function* `CCSPlayerController::ProcessUsercmds` and the `CUserCmd` offset/stride are CS2 facts → they live in the shim + gamedata, never in `core/src`. `CUserCmd`/`CSGOUserCmdPB` are CS2 engine types → shim-only.
- Both boundary gates (`check-core-boundary.sh` + the shim-owns-CS2 rule) stay green: the core diff contains no CS2 type names or field strings baked in; the JS field-name → protobuf-field-name mapping is in the engine-generic module because the names are Source2-shared.

## 5. API — `@s2script/usercmd`

A types-only package (`packages/usercmd/{package.json,index.d.ts}`) resolving via the standard `@s2script/<name>` prelude rule.

```ts
// Subscribe to the per-tick input hook. The handler runs synchronously during ProcessUsercmds;
// the `cmd` is block-scoped (valid ONLY during the call — a stashed `cmd` post-await reads/writes
// nothing). Return a HookResult >= Handled to SUPPRESS this input (the game processes a zeroed/idle
// command); return Continue/undefined to let the (possibly modified) command through.
export const UserCmd: {
  onRun(handler: (cmd: Cmd, ctx: { slot: number }) => HookResult | void): void;
};

export interface Cmd {
  // Movement (CBaseUserCmdPB scalars) — get and SET.
  forwardMove: number;   // +forward / -back
  sideMove: number;      // +right / -left. NEGATED from the raw protobuf `leftmove` (which is +LEFT):
                         //   the shim's field-1 read AND write flip the sign so sideMove is +right
                         //   (SourceMod convention). Getting the sign wrong silently inverts the headline
                         //   sideways/half-sideways surf styles, so it is gate-checked (D→>0, A→<0).
  upMove: number;        // +up / -down
  impulse: number;
  // The pressed-button mask (CInButtonStatePB.buttonstate1) — get and SET.
  buttons: bigint;       // 64-bit; IN_* bit values (mirrors pawn.buttons)
  // View angles (CMsgQAngle) — get and SET.
  viewAngles: QAngle;    // {x:pitch, y:yaw, z:roll}
  // Subtick interaction helper (see §6): drop the subtick analog moves so a coarse
  // forwardMove/sideMove write isn't overridden. No-op if there are none.
  clearSubtickMoves(): void;
}
```

`HookResult` is the existing prelude global (re-exported, as `@s2script/events`/`@s2script/damage` do). Reads are `T` (never null — the cmd is always live inside the hook); a modify on a resolved field always succeeds or is a logged no-op (degrade-never-crash). `buttons` is a `bigint` (64-bit, like the entity `readUInt64` primitives); a plugin ANDs/ORs `IN_*` bits.

## 6. The one real unknown — the subtick spike (plan task 1)

CS2 movement is refined by `subtick_moves` (per-press analog deltas). The open question: does writing `forwardMove`/`sideMove` on the coarse `base` fields alone change movement, or must the `subtick_moves` also be cleared/rewritten? The **plan's first task is a spike** that:
1. Confirms the hook fires (log `numcmds` + a live `forwardMove`/`buttons` for a moving human).
2. Confirms read via reflection matches the player's actual input.
3. Forces one modify (`forwardMove = 0`, or force an `IN_JUMP` button) and observes the effect on a live human client.
4. Determines whether `clearSubtickMoves()` is required for a coarse modify to take — and validates `sizeof(CUserCmd)` (the array stride) against a second-cmd read.

The shipped API only promises fields the spike proves controllable. **Post-spike, commit to ONE subtick design and state it identically in this §6, in the plan's Task 3 (the write op), and in Task 4 (the `Cmd` API) — do not leave both framings live:** either **(A)** every movement write (`s2_usercmd_write` fields 0/1/2) AUTO-clears `subtick_moves`, or **(B)** the plugin opts in via `clearSubtickMoves()`. The spike's verdict picks A or B; if coarse writes alone suffice, `clearSubtickMoves()` remains an exposed-but-optional helper.

Offline stride seed (SHOULD-FIX 1): rather than live-bisecting `sizeof(CUserCmd)` (each wrong guess is a virtual call through a bad pointer → a crash on a human-joined server), derive `sizeof(CSGOUserCmdPB)` once by `protoc`-compiling the vendored `usercmd.proto`/`cs_usercmd.proto` in a SCRATCH (never-committed) build against the vendored protobuf 3.21.8, and use the live cmd[1] read only to CONFIRM.

## 7. Safety & degrade-never-crash

- The raw `CUserCmd*`/protobuf pointer **never crosses to JS** — only `(slot, cmdIndex)` do; the block-scoped `Cmd` reads/writes through ops that re-resolve the current cmd's message shim-side (valid only during dispatch, like `DamageInfo`/`GameEvent`).
- The core dispatch `catch_unwind`s per subscriber (a plugin throw never corrupts the input or crashes the server) and is `try_borrow_mut`-guarded (a re-entrant dispatch is skipped, per the isolate-borrow rule).
- Every reflection write is `cpp_type()`/`is_repeated()`-guarded (reusing the usermessage guards — an `is_repeated` scalar `Set*` aborts the process; already handled).
- Unresolved signature → the detour is never installed → `UserCmd.onRun` is a no-op (subscribers never fire), never a crash. Boot gate LOUDLY reports the sig status (treadmill).
- The detour is removed on shim unload (`detour.cpp` restores the prologue), like the damage hook.

## 8. Slicing

**This spec = one core slice** (`@s2script/usercmd`: the hook + read + modify + block), implemented spike-first per §6. Live gate: a `usercmd-demo` plugin whose `onRun` (a) logs a human's live `forwardMove`/`buttons` (read proof) and (b) on a toggle forces `forwardMove=0` or an `IN_JUMP` (modify/block proof) — visibly affecting a **human client** (bots don't send usercmds through this path in a testable way; this is a human-client gate, like SayText2/damage).

**Separate, follow-on port slice (NOT this spec):** wire the surf-timer input styles (sideways = route `forwardMove`→`sideMove`; backwards; W-only) onto the published `@s2script/usercmd`. Lives entirely in `../s2s-surftimer-port`, gated on this slice publishing.

## 9. Out of scope / deferred (do NOT build ahead)

- The port's input styles (separate slice, above).
- Auto-bhop / other input mods (the primitive enables them; not built here).
- Writing `subtick_moves` beyond a clear (full subtick synthesis).
- A client-side prediction story (server-authoritative modify only; minor mispredict is acceptable, as with all CS2 movement mods).
- `weaponselect`/`mousedx/dy`/random_seed exposure (add later if a consumer needs them).
