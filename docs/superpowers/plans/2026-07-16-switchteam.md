# SwitchTeam (`Player.switchTeam` — non-lethal team switch) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `Player.switchTeam(team)` in `@s2script/cs2` — the NON-LETHAL T/CT controller move (alive + weapons kept, pawn may respawn) via the self-resolved `CCSPlayerController::SwitchTeam`, closing TTT-port gap slice #7 (role→team without killing the player).

**Architecture (spec: `docs/superpowers/specs/2026-07-16-switchteam-design.md`):** an exact structural sibling of the shipped changeteam slice — one gamedata signature (verified UNIQUE @va 0x1525f40 on 2000875; SwiftlyS2+CSSharp-corroborated, re-validated at every boot), one engine-generic core op `player_switch_team(idx, serial, team)` appended at the `S2EngineOps` tail (after `transmit_stats`), one shim fn (serial-gate + 0..3 bounds + `.text` guard + **`team <= 1` dispatches to the already-resolved ChangeTeam** — CSSharp/SwiftlyS2 spectator parity), one `pawn.js` method, one `.d.ts` declaration. **Synchronous call, no deferral** (spec §4). Degrade-never-crash throughout.

**Tech Stack:** Rust core (`core/src/v8host.rs`), C++ shim (`shim/src/s2script_mm.cpp`, `shim/include/s2script_core.h`), `gamedata/core.gamedata.jsonc`, `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`, `examples/switchteam-demo`. Graphite stack of 4 PRs.

## Graphite stack map

| PR | Branch (gt) | Content |
|---|---|---|
| PR1 | `switchteam/design-docs` | this plan + the design spec |
| PR2 | `switchteam/engine` | gamedata `"SwitchTeam"` entry + core ABI/native/test + shim resolve/op/wiring |
| PR3 | `switchteam/cs2-surface` | `pawn.js` method + `index.d.ts` + changeset |
| PR4 | `switchteam/demo` | `examples/switchteam-demo` (TTT-three-scenarios-shaped) |

Worktree `/home/gkh/projects/s2script-switchteam` is on `feat/switchteam` at origin/main (bfba15f). If the branch is untracked for gt: `gt track -p main` first. Create each task's branch with `gt create -am "<msg>"` (stacked); `gt submit --no-interactive` at the end. Run the gate suite **per PR** (each must be atomic — PR2 without PR3 is a dormant op + native, safe; PR3 requires PR2 below it, which is what the stack encodes).

## Global Constraints

- **ABI-append discipline — and a LIVE collision hazard.** `player_switch_team` appends after `transmit_stats` (the current origin/main tail) in: (1) C typedef + (2) struct member (`shim/include/s2script_core.h` :259/:386), (3) Rust type alias + (4) struct field (`core/src/v8host.rs` :223 area/:372), (5) BOTH test mock op-structs (locate: `grep -n "transmit_stats: None" core/src/v8host.rs` — two hits, ~:11233 and ~:12098), (6) the wiring after `ops.transmit_stats` (`s2script_mm.cpp:3589`). Order MUST match between the Rust struct and the C twin. **COLLISION:** open PRs #67, #71, #76, #80 all append at this same tail (#67/#71 are stale — still pre-transmit); `S2EngineOps` has no size/version handshake, so a missed re-tail after another stack merges is a **silent function-pointer misdispatch**, not a compile error. At every `gt restack`: re-read the trunk tail in both files and re-append there.
- **RE doctrine:** the sig `55 48 89 E5 41 54 49 89 FC 89 F7` is corroborated (SwiftlyS2 + CSSharp ship it) and **verified unique @0x1525f40 on our 2000875 binary** — but the boot gate (`ResolveSigValidated` + `GamedataResult`) re-validates uniqueness on every load. Never reuse the OLD "SwitchTeam" sig from the changeteam-era notes — it live-gate-proved to hit the deferred `m_bSwitchTeamsOnNextRoundReset` halftime-swap function (see `gamedata/core.gamedata.jsonc:202-214`).
- **Boundary:** no CS2 identifier in `core/src` — core speaks `(idx, serial, team)` ints only. Gates: `make check-boundary` + `bash scripts/test-boundary-nameleak.sh` per PR.
- **Degrade-never-crash:** unresolved sig → `s_pSwitchTeam` null → op no-ops (named `GamedataResult` reason at boot); no op installed → native no-ops (pinned by the in-isolate test); stale ref → serial-gate no-op; out-of-`.text` pointer → logged no-op.
- **Worktree gotcha:** `third_party/` submodules may be empty — `git submodule update --init --recursive` before any shim build.
- **Core tests are single-threaded** (`.cargo/config.toml`): `cargo test -p s2script-core` — do not pass `--test-threads`.
- **pawn.js in dist is a CONCAT** (schema/nav/activity/csitem/pawn) — build via `scripts/package-addon.sh` / the plugin build, never raw-`cp`.
- **Sniper build + Docker live gate are the MAIN LOOP's job** (not this plan's tasks): `docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`, then the spec §10 gate — alive T↔CT with no kill and no drop-to-spectator, dead T→CT with `pawnIsAlive` still false, bulk reveal, pawn-respawn probe.
- **Naming:** PascalCase types, camelCase members (`switchTeam`). Plugins are pure ESM.

## File Structure

- `docs/superpowers/specs/2026-07-16-switchteam-design.md` + this file. *(Task 1.)*
- `gamedata/core.gamedata.jsonc` — `"SwitchTeam"` entry after `"ChangeTeam"` (:215-221). *(Task 2.)*
- `core/src/v8host.rs` — type alias (:223 area) + tail field (:372) + native (~:5211) + `set_native` (:7030) + 2 mock entries + degrade test (~:11455). *(Task 2.)*
- `shim/include/s2script_core.h` — typedef (:259 area) + tail member (:386). *(Task 2.)*
- `shim/src/s2script_mm.cpp` — `SwitchTeam_t`/`s_pSwitchTeam`/op fn (after :1248) + resolve block (after :3100) + wiring (:3589). *(Task 2.)*
- `games/cs2/js/pawn.js` — `Player.prototype.switchTeam` after `spectate` (:115). *(Task 3.)*
- `packages/cs2/index.d.ts` — `switchTeam(team)` after `spectate()` (~:125). *(Task 3.)*
- `.changeset/switchteam.md` — minor `@s2script/cs2`. *(Task 3.)*
- `examples/switchteam-demo/{package.json,tsconfig.json,src/plugin.ts}` (new). *(Task 4.)*

---

## Task 1 (PR1): Design docs

**Files:** create `docs/superpowers/specs/2026-07-16-switchteam-design.md`, `docs/superpowers/plans/2026-07-16-switchteam.md` (this file — both already written by the design step).

### Steps

- [ ] **Step 1: Commit the two docs.**

```bash
git add docs/superpowers/specs/2026-07-16-switchteam-design.md docs/superpowers/plans/2026-07-16-switchteam.md
gt create -am "switchteam/design-docs: SwitchTeam (non-lethal team switch) — spec + plan

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 2 (PR2): Engine — gamedata sig + core op/native/test + shim resolve/call/wiring

**Files:** modify `gamedata/core.gamedata.jsonc`, `core/src/v8host.rs`, `shim/include/s2script_core.h`, `shim/src/s2script_mm.cpp`.

**Interfaces:**
- Produces (consumed by Task 3): the global native `__s2_player_switch_team(index, serial, team)` (void; no-op on degrade) backed by the C op `void (*player_switch_team)(int idx, int serial, int team)` at the `S2EngineOps` tail, which the shim fills with the sig-resolved, serial-gated, spectator-dispatching call.

### Steps

- [ ] **Step 1 (TDD): the failing core test.**

In `core/src/v8host.rs`, immediately after `player_change_team_degrades_without_op` (:11445-11455), add:

```rust
    /// switchteam slice: `__s2_player_switch_team` (and `Player.switchTeam`) no-op with no
    /// `player_switch_team` op (unresolved signature / every in-isolate test) — never a crash.
    #[test]
    fn player_switch_team_degrades_without_op() {
        init(dummy_logger()).unwrap();
        // No ENGINE_OPS installed -> the op is absent -> the native no-ops (returns undefined), no throw.
        let out = eval_std("pst1", r#"
            var r = __s2_player_switch_team(5, 7, 2);
            String(r === undefined);
        "#);
        assert_eq!(out, "true");
        shutdown();
    }
```

Run `cargo test -p s2script-core player_switch_team` — expected: **FAIL** (`__s2_player_switch_team is not defined`).

- [ ] **Step 2: Rust ABI — alias + tail field + mock entries.**

In `core/src/v8host.rs`, next to `type PlayerChangeTeamFn` (:223):

```rust
type PlayerSwitchTeamFn = extern "C" fn(c_int, c_int, c_int);
```

Struct field after `pub transmit_stats: Option<TransmitStatsFn>,` (:372), before the struct closes:

```rust
    // --- switchteam slice (APPENDED after transmit_stats; order is the ABI; do not reorder above) ---
    pub player_switch_team: Option<PlayerSwitchTeamFn>,
```

Then `grep -n "transmit_stats: None" core/src/v8host.rs` (two hits, ~:11233 and ~:12098) and add after EACH:

```rust
            player_switch_team: None,
```

- [ ] **Step 3: The native + registration.**

In `core/src/v8host.rs`, directly after `s2_player_change_team`'s body (:5196-5211):

```rust
/// `__s2_player_switch_team(index, serial, team)` — NON-LETHAL controller team move via the
/// sig-resolved SwitchTeam engine-op (the player stays alive and keeps weapons; the pawn may be
/// respawned — vs `__s2_player_change_team` = jointeam semantics). A thin pass-through: the shim
/// reconstructs + serial-gates the controller from (index, serial), bounds-checks `team` (0..3), and
/// dispatches 0/1 (None/Spectator) to ChangeTeam (CSSharp/SwiftlyS2 parity). No-op without the op
/// (unresolved signature) or on a stale ref. Engine-generic here (only the resolving signature is
/// game-specific).
fn s2_player_switch_team(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 3 { return; }
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as c_int;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as c_int;
        let team = args.get(2).integer_value(scope).unwrap_or(-1) as c_int;
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.player_switch_team else { return };
        f(index, serial, team);
    }));
}
```

Register after the `__s2_player_change_team` line (:7030):

```rust
    set_native(scope, global_obj, "__s2_player_switch_team", s2_player_switch_team);
```

Run `cargo test -p s2script-core player_switch_team` — expected: **PASS**. Then `cargo test -p s2script-core` — full suite green (previous count + 1).

- [ ] **Step 4: C ABI twin.**

In `shim/include/s2script_core.h`, after the `s2_transmit_stats_fn` typedef (:259):

```c
/* switchteam slice: player_switch_team — NON-LETHAL controller team move (idx,serial → serial-gated
 * CCSPlayerController*) to `team` via the sig-resolved CCSPlayerController::SwitchTeam (alive +
 * weapons kept; the pawn may be respawned). team 0/1 (None/Spectator) dispatches to ChangeTeam
 * (CSSharp/SwiftlyS2 parity). No-op if the signature is unresolved or the ref is stale. */
typedef void (*s2_player_switch_team_fn)(int idx, int serial, int team);
```

Struct member after `s2_transmit_stats_fn transmit_stats;` (:386), before `} S2EngineOps;`:

```c
    /* switchteam slice — APPENDED after transmit_stats; order is the ABI; do not reorder above. */
    s2_player_switch_team_fn player_switch_team;
```

- [ ] **Step 5: Gamedata entry.**

In `gamedata/core.gamedata.jsonc`, after the `"ChangeTeam"` entry's closing brace (:221), add:

```jsonc
    // CCSPlayerController::SwitchTeam(unsigned int team) — the NON-LETHAL team move (player stays alive,
    // keeps weapons; the pawn MAY be respawned by the engine). Backs Player.switchTeam (T/CT native;
    // team 0/1 dispatches to ChangeTeam shim-side, CSSharp/SwiftlyS2 parity). HISTORY: the FIRST
    // "SwitchTeam" sig we tried (borrowed from CSSharp during the changeteam slice) resolved to the
    // WRONG function on our build — the deferred m_bSwitchTeamsOnNextRoundReset (halftime-swap) setter,
    // live-gate-proven no-move (see the ChangeTeam comment above). THIS pattern is the real per-player
    // function: corroborated by SwiftlyS2 + current CSSharp shipping the identical bytes, RE-VALIDATED
    // against OUR binary — resolves UNIQUE @va 0x1525f40 on 2000875, adjacent to ChangeTeam (@0x1524770;
    // vtable siblings). Prologue saves `this` into r12 (49 89 FC) and moves team esi->edi (89 F7),
    // confirming the (this /*rdi*/, unsigned int team /*esi*/) ABI. DIRECT: the unique match IS the
    // function entry. The boot gate re-validates uniqueness every load and SCREAMS if the prologue
    // moves on the update treadmill.
    "SwitchTeam": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 41 54 49 89 FC 89 F7",
        "resolve": "direct"
      }
    },
```

- [ ] **Step 6: Shim — typedef, static, op fn (with the spectator dispatch).**

In `shim/src/s2script_mm.cpp`, directly after `s2_player_change_team`'s closing brace (:1248) — `s_pChangeTeam` must be in scope above:

```cpp
// ---------------------------------------------------------------------------
// player_switch_team (switchteam slice) — NON-LETHAL controller team move via
// CCSPlayerController::SwitchTeam(this, team): the player stays alive and keeps weapons (vs ChangeTeam
// = jointeam semantics); the pawn MAY be respawned (consumers re-resolve player.pawn next frame). For
// team <= 1 (None/Spectator) dispatches to s2_player_change_team — CSSharp/SwiftlyS2 parity: the
// engine SwitchTeam is CS:GO-lineage T/CT-only. Guarded identically to change_team: serial-gate +
// 0..3 bounds + .text-range check; any failure degrades to a (logged) no-op, never a crash.
// HISTORY: an earlier borrowed "SwitchTeam" sig hit the WRONG function on our build (the deferred
// m_bSwitchTeamsOnNextRoundReset halftime swap — live-gate-proven no-move); this sig is the real
// per-player function, validated UNIQUE @0x1525f40 on 2000875 and re-validated every boot.
// ABI: void CCSPlayerController::SwitchTeam(this /*rdi*/, unsigned int team /*esi*/).
// ---------------------------------------------------------------------------
typedef void (*SwitchTeam_t)(void* thisptr, int team);
static SwitchTeam_t s_pSwitchTeam = nullptr;             // sig-resolved fn ptr (loaded in Load)
static void s2_player_switch_team(int idx, int serial, int team) {
    if (team < 0 || team > 3) return;                    // Unassigned/Spectator/T/CT only
    if (team <= 1) {                                     // None/Spectator -> ChangeTeam (parity path)
        s2_player_change_team(idx, serial, team);
        return;
    }
    if (!s_pSwitchTeam) return;                          // signature unresolved -> no-op
    CEntityHandle h(idx, serial);
    void* controller = s2_deref_handle(static_cast<unsigned int>(h.ToInt()));  // null if stale/free slot
    if (!controller) return;
    const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pSwitchTeam);
    if (!s_serverText || f < s_serverText || f >= s_serverText + s_serverTextSize) {
        META_CONPRINTF("[s2script] SwitchTeam fn %p out of libserver .text — no-op\n", (const void*)f);
        return;
    }
    s_pSwitchTeam(controller, team);
}
```

- [ ] **Step 7: Shim — Load-time resolve block.**

After the ChangeTeam resolve block's closing brace (~:3100, before the `LegacyGameEventListener` block):

```cpp
            // switchteam slice: resolve CCSPlayerController::SwitchTeam (Player.switchTeam — the
            // NON-LETHAL T/CT move). Sig corroborated by SwiftlyS2 + CSSharp but VALIDATED on OUR
            // libserver.so (unique @0x1525f40 on 2000875) — NOT the changeteam-era borrowed sig that
            // hit the deferred m_bSwitchTeamsOnNextRoundReset function (see the gamedata comment).
            // Degrade-never-crash: unresolved -> switch_team no-ops (the spectator dispatch to
            // ChangeTeam still works if that sig resolved).
            auto swit = sigs.find("SwitchTeam");
            if (swit == sigs.end()) {
                GamedataResult("SwitchTeam", false, "signature absent from gamedata");
            } else {
                int64_t swOff = ResolveSigValidated("SwitchTeam", swit->second);
                ModText swmt = FindModuleText(swit->second.module.c_str());
                if (swOff != s2sig::kFail && swmt.text) {  // resolve=="direct": the unique match IS the function start
                    s_pSwitchTeam = reinterpret_cast<SwitchTeam_t>(const_cast<uint8_t*>(swmt.text) + swOff);
                    META_CONPRINTF("[s2script] SwitchTeam resolved @%p (Player.switchTeam; libserver .text=%p+%zu)\n",
                                   reinterpret_cast<void*>(s_pSwitchTeam), (const void*)s_serverText, s_serverTextSize);
                }   // swOff == kFail: ResolveSigValidated already recorded the reason
            }
```

- [ ] **Step 8: Shim — ops wiring.**

After `ops.transmit_stats = &s2_transmit_stats;` (:3589):

```cpp
    // switchteam slice — APPENDED after transmit_stats; order MUST match S2EngineOps.
    ops.player_switch_team = &s2_player_switch_team;
```

- [ ] **Step 9: Build + gates.**

```bash
git submodule update --init --recursive      # if third_party/ is empty
make core                                    # expected: cargo release build green
cargo test -p s2script-core                  # expected: full suite green (incl. the new test)
make shim                                    # expected: cmake shim build green (host-only sanity; sniper build is the main loop's)
make check-boundary                          # expected: PASS
bash scripts/test-boundary-nameleak.sh       # expected: PASS
bash scripts/check-schema-generated.sh       # expected: PASS (untouched)
```

- [ ] **Step 10: Commit (PR2).**

```bash
git add gamedata/core.gamedata.jsonc core/src/v8host.rs shim/include/s2script_core.h shim/src/s2script_mm.cpp
gt create -am "switchteam/engine: player_switch_team op + self-resolved SwitchTeam sig + shim call

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 3 (PR3): CS2 surface — pawn.js + .d.ts + changeset

**Files:** modify `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`; create `.changeset/switchteam.md`.

**Interfaces:**
- Consumes (Task 2): the `__s2_player_switch_team` native.
- Produces: `Player.switchTeam(team: number): void` (typed, documented).

### Steps

- [ ] **Step 1: pawn.js method.**

In `games/cs2/js/pawn.js`, after `Player.prototype.spectate`'s closing brace (:115):

```js
  // player.switchTeam(team) — NON-LETHAL move between T(2)/CT(3) via the sig-resolved
  // CCSPlayerController::SwitchTeam: the player stays alive and keeps weapons (vs changeTeam =
  // jointeam semantics). The engine MAY respawn the pawn — re-resolve player.pawn next frame before
  // pawn writes. team 0/1 dispatches to ChangeTeam shim-side (CSSharp/SwiftlyS2 parity). Serial-gated;
  // no-op if stale/unresolved.
  Player.prototype.switchTeam = function (team) {
    __s2_player_switch_team(this.ref.index, this.ref.serial, team | 0);
  };
```

- [ ] **Step 2: index.d.ts declaration.**

In `packages/cs2/index.d.ts`, after `spectate(): void;` (~:125, inside the `Player` interface):

```ts
  /**
   * NON-LETHAL team switch between Terrorist (2) and CounterTerrorist (3) via the sig-resolved
   * CCSPlayerController::SwitchTeam: the player stays alive and keeps their weapons (vs `changeTeam`,
   * which has jointeam semantics and usually kills). Works on DEAD controllers too (a pure
   * scoreboard/win-condition team move). CAVEAT: the engine MAY respawn the pawn during the call —
   * re-resolve `player.pawn` on the next frame before any pawn write. Game events the engine fires
   * inside the call do not re-dispatch to JS handlers on that frame (re-entrancy skip). For None (0) /
   * Spectator (1) this dispatches to `changeTeam` (CSSharp/SwiftlyS2 parity) — prefer `spectate()`.
   * Serial-gated; a no-op if the ref is stale or the signature is unresolved. Bounded 0..3 engine-side.
   */
  switchTeam(team: number): void;
```

- [ ] **Step 3: Changeset.**

`.changeset/switchteam.md`:

```md
---
"@s2script/cs2": minor
---

`Player.switchTeam(team)` — non-lethal T/CT team switch (the player stays alive and keeps weapons; the
pawn may be respawned) via the self-resolved `CCSPlayerController::SwitchTeam`. None/Spectator
dispatches to ChangeTeam (CSSharp/SwiftlyS2 parity). Serial-gated; degrades to a no-op when the
signature is unresolved. Closes the TTT-port "role→team without killing the player" gap.
```

- [ ] **Step 4: Gates.**

```bash
bash scripts/check-plugins-typecheck.sh      # expected: PASS (every plugin + example vs the new .d.ts)
bash scripts/check-schema-generated.sh       # expected: PASS (pawn.js is hand-written, but confirm no codegen drift)
make check-boundary && bash scripts/test-boundary-nameleak.sh   # expected: PASS
```

- [ ] **Step 5: Commit (PR3).**

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts .changeset/switchteam.md
gt create -am "switchteam/cs2-surface: Player.switchTeam in @s2script/cs2 (+changeset)

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 4 (PR4): Demo — the three TTT scenarios, bot-provable

**Files:** create `examples/switchteam-demo/{package.json,tsconfig.json,src/plugin.ts}`.

**Interfaces:**
- Consumes: `Player.switchTeam`/`teamNum`/`pawnIsAlive`/`pawn.health`/`pawn.weapons` (`@s2script/cs2`), `Commands.register` (`@s2script/sdk/commands`), `delay` (`@s2script/sdk/timers`).

### Steps

- [ ] **Step 1: Scaffold.**

`examples/switchteam-demo/package.json`:

```json
{
  "name": "@demo/switchteam-demo",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x"
  }
}
```

`examples/switchteam-demo/tsconfig.json`: `cp examples/changeteam-demo/tsconfig.json examples/switchteam-demo/tsconfig.json`.

- [ ] **Step 2: The plugin.**

`examples/switchteam-demo/src/plugin.ts`:

```ts
// @demo/switchteam-demo — live gate for the switchteam primitive (Player.switchTeam), the NON-LETHAL
// T<->CT move (sig-resolved CCSPlayerController::SwitchTeam). Mirrors the TTT port's three consumers:
//
//   sm_switchtest   — role-assignment shape (RoleIconsHandler): the first alive in-game player T<->CT;
//                     asserts the team moved IMMEDIATELY (synchronous read-back), then next-frame:
//                     still alive, weapons survived, and the pawn-respawn probe (EntityRef identity).
//   sm_deadtest     — body-identify shape (BodyPickupListener): a DEAD T/CT controller to the other
//                     team; asserts teamNum moved and pawnIsAlive stayed false (sm_slay a bot first).
//   sm_revealtest   — round-end reveal shape (RoundTimerListener): every T -> CT in bulk, one frame.

import { Commands } from "@s2script/sdk/commands";
import { delay } from "@s2script/sdk/timers";
import { Player } from "@s2script/cs2";

const TEAM = (t: number | null | undefined): string =>
  t === 1 ? "Spectator" : t === 2 ? "T" : t === 3 ? "CT" : `#${t}`;

export function onLoad(): void {
  Commands.register("switchtest", (ctx) => {
    const ps = Player.all(); // in-game (pawn-gated) — alive players
    if (!ps.length) { ctx.reply("switchteam-demo: no in-game players"); return; }
    const p = ps[0];
    const uid = p.userId;
    const before = p.teamNum ?? -1;
    const pawnBefore = p.pawn;
    const refBefore = pawnBefore ? `${pawnBefore.ref.index}:${pawnBefore.ref.serial}` : "none";
    const hpBefore = pawnBefore ? (pawnBefore.health ?? -1) : -1;
    const wepBefore = pawnBefore ? pawnBefore.weapons.length : -1;
    const target = before === 2 ? 3 : 2;
    p.switchTeam(target);
    const immediate = p.teamNum ?? -1; // SYNCHRONOUS re-read — the move must be immediate (spec §4)
    ctx.reply(`switchtest slot=${p.slot}: ${TEAM(before)} -> switchTeam(${TEAM(target)}); immediate=${TEAM(immediate)}`);
    console.log(`[switchteam-demo] slot=${p.slot} uid=${uid} before=${TEAM(before)} immediate=${TEAM(immediate)} ` +
                `hpBefore=${hpBefore} wepBefore=${wepBefore} pawnRefBefore=${refBefore}`);
    delay(600).then(() => {
      const after = Player.fromUserId(uid); // the pawn may have been respawned — re-resolve (TTT pattern)
      const pawn = after ? after.pawn : null;
      const refAfter = pawn ? `${pawn.ref.index}:${pawn.ref.serial}` : "none";
      console.log(`[switchteam-demo] AFTER slot=${p.slot} team=${TEAM(after ? after.teamNum : null)} ` +
                  `alive=${after ? after.pawnIsAlive : null} hp=${pawn ? pawn.health : null} ` +
                  `weapons=${pawn ? pawn.weapons.length : -1} pawnRef=${refAfter} respawned=${refAfter !== refBefore} ` +
                  `— expect team=${TEAM(target)}, alive=true, weapons kept`);
    });
  });

  Commands.register("deadtest", (ctx) => {
    const dead = Player.allConnected().find(
      (p) => p.pawnIsAlive === false && (p.teamNum === 2 || p.teamNum === 3)
    );
    if (!dead) { ctx.reply("switchteam-demo: no dead T/CT player (sm_slay a bot first)"); return; }
    const uid = dead.userId;
    const before = dead.teamNum ?? -1;
    const target = before === 2 ? 3 : 2;
    dead.switchTeam(target);
    const immediate = dead.teamNum ?? -1;
    ctx.reply(`deadtest slot=${dead.slot}: DEAD ${TEAM(before)} -> ${TEAM(target)}; ` +
              `immediate=${TEAM(immediate)} pawnIsAlive=${dead.pawnIsAlive}`);
    delay(600).then(() => {
      const after = Player.fromUserId(uid);
      console.log(`[switchteam-demo] deadtest AFTER team=${TEAM(after ? after.teamNum : null)} ` +
                  `pawnIsAlive=${after ? after.pawnIsAlive : null} — expect ${TEAM(target)} + false ` +
                  `(the TTT BodyPickup contract; if true, log it — spec §9 documented side effect)`);
    });
  });

  Commands.register("revealtest", (ctx) => {
    let n = 0;
    for (const p of Player.allConnected()) {
      if (p.teamNum === 2) { p.switchTeam(3); n++; }
    }
    ctx.reply(`revealtest: moved ${n} T player(s) -> CT (round-end reveal shape)`);
    console.log(`[switchteam-demo] revealtest moved ${n} players T->CT in one frame`);
  });

  console.log("[switchteam-demo] onLoad — sm_switchtest / sm_deadtest / sm_revealtest registered");
}

export function onUnload(): void {
  console.log("[switchteam-demo] onUnload");
}
```

- [ ] **Step 3: Build + gates.**

```bash
( cd examples/switchteam-demo && npx s2script build )   # expected: dist/*.s2sp, strict typecheck PASS
bash scripts/check-plugins-typecheck.sh                  # expected: PASS (demo included)
```

- [ ] **Step 4: Commit + submit the stack.**

```bash
git add examples/switchteam-demo
gt create -am "switchteam/demo: TTT-three-scenarios live-gate demo (alive/dead/bulk switch)

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
gt submit --no-interactive
```

PR bodies (Write tool + `gh pr edit N --body-file`, never a heredoc) must include **Stack Context** (the TTT-port switchteam slice: non-lethal role→team) and **Why** per PR, the wrong-sig history + the @0x1525f40 verification, the synchronous decision (spec §4), the ABI-tail collision note vs #67/#71/#76/#80, the deferred live-gate items, and `https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3`.

**Handoff to the main loop (NOT plan tasks):** sniper build (`rust:bullseye` + `scripts/build-sniper.sh` — core AND shim both changed, both `.so`s rebuild), deploy (`scripts/package-addon.sh` + copy the demo `.s2sp`, recreate writable `configs/`, `docker compose restart cs2` — never `--force-recreate`), then the spec §10 gate via `python3 scripts/rcon.py`: boot-log `SwitchTeam resolved @…`; `bot_add` → `sm_switchtest` (immediate move, **no kill, no drop to spectator**, weapons kept, respawn probe logged); `sm_slay` + `sm_deadtest` (team moved, `pawnIsAlive` false); `sm_revealtest`; `sm_spectest` side-by-side (the two siblings are different functions). STOP conditions per spec §10.

---

## Self-Review

**1. Spec coverage.** §1 three TTT sites → the demo's three commands (Task 4). §2 RE (verified sig, wrong-sig history, boot re-validation, `.text` guard, degrade) → Task 2 Steps 5-7 + the gamedata/shim comments. §3 semantics + spectator dispatch → Task 2 Step 6 (`team <= 1` branch) + both doc comments (Task 3). §4 synchronous decision → no queue anywhere; the demo's synchronous `immediate=` read-back pins it live; the re-entrancy caveat is in the `.d.ts` (Task 3 Step 2). §5 API → Task 3. §6 seven touchpoints + ABI tail + collision → Tasks 2-3 + Global Constraints. §7 boundary → gates in every task. §8 deviations → recorded in the spec. §9 deferred → nothing in this plan builds them (no `pawnIsAlive` re-force in the shim; the demo only *reads* it). §10 live gate → the handoff block (main loop's job, as directed).

**2. Placeholder scan.** No TBD/TODO. Every code block is complete and paste-able; every `:N` anchor was verified against the worktree at bfba15f (`PlayerChangeTeamFn` :223, tail :372/:386, native :5196-5211, `set_native` :7030, mocks :11233/:12098, degrade test :11445, shim op :1234-1248, resolve :3081-3100, wiring :3578/:3589, gamedata `"ChangeTeam"` :215, pawn.js :107-115, `spectate()` ~:125).

**3. Type consistency.** Op: C `void (*)(int,int,int)` ↔ Rust `extern "C" fn(c_int,c_int,c_int)` ↔ shim `static void s2_player_switch_team(int,int,int)` — identical to the change_team triple. Native `__s2_player_switch_team(index, serial, team)` ↔ pawn.js `(this.ref.index, this.ref.serial, team | 0)` ↔ `.d.ts` `switchTeam(team: number): void`. Demo accessors verified present: `teamNum`/`pawnIsAlive` (`packages/cs2/schema.generated.d.ts:27/:163`, both non-readonly), `pawn.ref`/`pawn.health`/`pawn.weapons`/`Player.fromUserId`/`allConnected` (`packages/cs2/index.d.ts`). The spectator-dispatch branch calls `s2_player_change_team(idx, serial, team)` — defined above it in the same file, same arity.
