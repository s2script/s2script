# Player Respawn Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. All RE constants are ALREADY resolved (offline, 2026-07-16, in this worktree against the pinned build 2000875) — there is no spike task; Task 2 is directly implementable. The sniper build + Docker live gate (respawn a dead bot: `pawnIsAlive` true, health > 0, fresh spawn position, the demo's own `player_spawn` logger fires) are the **MAIN LOOP's job — out of this plan's scope**; this plan ends at host-build gates + a deployable demo.

**Goal:** `Player.respawn(): boolean` for the TTT port (gap #5): a self-resolved, RTTI-vtable-membership-validated `CCSPlayerController::Respawn` call, queued to the next GameFrame outside the JS isolate borrow so `player_spawn` reaches every plugin.

**Architecture:** One `S2EngineOps` op appended after `transmit_stats`: `player_respawn(idx, serial, alive_off) -> 1 queued / 0 degraded` ENQUEUES into a deduped multi-entry pending set in the shim; a dedicated `Hook_GameFrameRespawnDrain` GameFrame pre-hook (installed eagerly at Load iff BOTH boot gates pass) drains it OUTSIDE the JS borrow — per entry: re-deref serial-gated, re-check `m_bPawnIsAlive` at `alive_off`, `.text`-guard, call `s_pRespawn(controller)`. The sig passes TWO boot gates: `ResolveSigValidated` uniqueness AND `Respawn.vtable-member` — the sig-resolved address must be a member of the RTTI-derived `CCSPlayerController` primary vtable (`s2vtable::GetVTableByName`; CSSharp's borrowed slot 274 was an OFFLINE-ONLY finding aid, never shipped — the sm_slay-400/ChangeTeam-101 borrowed-index failure class). SetPawn is NOT called: Respawn-alone is Plan A (CSSharp's SetPawn sig has 0 hits on 2000875 — stale); Plan B (live-gate fallback, zero new engine facts) is a pawn.js schema pre-write `m_hPawn ← m_hPlayerPawn` + `notifyStateChanged`.

**Tech stack:** Rust core (`core/src/v8host.rs`), C++ shim (Metamod, `shim/src/s2script_mm.cpp`), gamedata JSONC, `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`.

**Spec:** `docs/superpowers/specs/2026-07-16-player-respawn-design.md`. **Graphite stack** (branch names `respawn/<change>`): Task 1 = PR1 `respawn/design-docs`, Task 2 = PR2 `respawn/engine-fact` (core + shim + gamedata, atomic), Task 3 = PR3 `respawn/cs2-surface` (+ changeset), Task 4 = PR4 `respawn/demo`.

## Global Constraints

- **RE doctrine:** ship ONLY the self-derived pattern `55 48 89 E5 41 54 53 48 89 FB E8 ? ? ? ? 48 85 C0 74 ? 48 89 C7 48 8B 00 FF 90 ? ? ? ? 84 C0` (masks the call rel32, jz disp, and virtual-call vtable disp; validated UNIQUE at va `0x14f0ce0` = RTTI vtable slot 274 on 2000875). NEVER ship a vtable index (CSSharp's `{windows:272, linux:274}` is a documented offline-only HINT in the gamedata comment). Prototype: `void CCSPlayerController::Respawn(this /*rdi*/)` — nullary, controller receiver.
- **The vtable-member gate is load-bearing:** uniqueness alone green-lit the wrong function on the round-control slice. `Respawn.vtable-member` must gate `s_pRespawn` assignment AND the drain-hook install; on failure the descriptor degrades with the named reason and the op returns 0 forever. NEVER weaken it to ship.
- **Deferral is a MUST, not polish:** the engine call runs ONLY from the drain hook (outside the JS borrow). An inline call from the op would make every plugin silently miss `player_spawn` (isolate-borrow re-entrancy memory). Consume the pending batch BEFORE calling (the call re-enters engine machinery).
- **ABI discipline:** the new op is APPENDED at the STRUCT TAIL after `transmit_stats`, byte-identical across all lockstep sites: C header typedef + field (`shim/include/s2script_core.h`, field after :386), Rust typedef + field (`core/src/v8host.rs`), BOTH full in-test `S2EngineOps{...}` literals (~:11167 and `mock_event_ops` ~:12028; `transmit_test_ops` spreads `..mock_event_ops()` — untouched), and the shim `ops.` wiring (after :3589). Verify with the ordered-field-name diff (Task 2 Step 3) BEFORE the change (empty) and AFTER (empty). **Collision watch:** PR #67 (round-control), PR #71 (voice), and `feat/writeconvar` all append at OLDER tails — right before submit, re-check what merged and re-anchor after the new tail in the same commit.
- **Boundary:** no CS2 name crosses the C ABI or enters `core/src` (the op signature is opaque ints); `"CCSPlayerController"`/`"m_bPawnIsAlive"` live ONLY in `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`, and the shim. Gates: `make check-boundary`, `./scripts/test-boundary-nameleak.sh`.
- **Degrade-never-crash:** unresolved/failed-gate sig → op returns 0; stale controller at enqueue → 0; stale or re-alive at drain → per-entry logged skip; pending set full (capacity 130) → 0 + log; fn-pointer `.text` range guard at drain; JS `respawn()` returns `false` on every degrade.
- **Never a raw pointer to JS:** JS passes `(index, serial, aliveOff)`; the shim derefs (serial-gated at BOTH enqueue and drain).
- **cargo test is forced single-threaded** via `.cargo/config.toml` — do not pass `--test-threads`.
- **Deploy gotcha (for the main loop):** dist pawn.js is a CONCAT (schema/nav/activity/csitem/pawn) — never raw-cp a single file; rebuild via `make package`.

---

## File Structure

- Task 1: `docs/superpowers/specs/2026-07-16-player-respawn-design.md`, `docs/superpowers/plans/2026-07-16-player-respawn.md` (this file)
- Task 2: `gamedata/core.gamedata.jsonc`, `shim/src/s2script_mm.cpp`, `shim/src/s2script_mm.h`, `shim/include/s2script_core.h`, `core/src/v8host.rs`
- Task 3: `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`, `.changeset/player-respawn-cs2.md`
- Task 4: `examples/respawn-demo/{package.json,tsconfig.json,src/plugin.ts}`, `docs/PROGRESS.md`

---

## Task 1 (PR1, `respawn/design-docs`): design spec + plan

**Files:**
- Add: `docs/superpowers/specs/2026-07-16-player-respawn-design.md` (already written)
- Add: `docs/superpowers/plans/2026-07-16-player-respawn.md` (this file)

**Steps:**

- [ ] **Step 1 — Commit + stack root.**

```bash
cd /home/gkh/projects/s2script-respawn
gt track -p main 2>/dev/null || true    # worktree branches start untracked
git add docs/superpowers/specs/2026-07-16-player-respawn-design.md docs/superpowers/plans/2026-07-16-player-respawn.md
gt create respawn/design-docs -m "respawn/design-docs: player respawn — spec + plan"
```

---

## Task 2 (PR2, `respawn/engine-fact`): Respawn engine fact — gamedata sig + vtable-member gate + deferred op + ABI append

**Files:**
- Modify: `gamedata/core.gamedata.jsonc` (new `Respawn` entry, after the `ChangeTeam` block ending ~:222)
- Modify: `shim/include/s2script_core.h` (op typedef + struct field, tail after :386)
- Modify: `core/src/v8host.rs` (Rust typedef + field + BOTH test literals + native + `set_native` + degrade test)
- Modify: `shim/src/s2script_mm.cpp` (vtable-member validator, op + pending set + drain hook, Load resolve, ops wiring, Unload removal)
- Modify: `shim/src/s2script_mm.h` (drain-hook member declaration, after `Hook_GameFramePre` at :47)

**Interfaces:**
- Produces (C ABI, appended after `transmit_stats`): `typedef int (*s2_player_respawn_fn)(int idx, int serial, int alive_off);` — 1 = queued (executes next GameFrame outside the JS borrow), 0 = degraded.
- Produces (JS native, raw-context): `__s2_player_respawn(index, serial, aliveOff) -> 0|1`.
- Consumes: `ResolveSigValidated` (:2026), `GamedataResult` (:2020), `FindModuleText`/`ModText` (:1990), `s2_deref_handle`, `s_serverText`/`s_serverTextSize`, `s2vtable::GetVTableByName` (`shim/src/vtable.h:30`), `SH_DECL_HOOK3_void(ISource2Server, GameFrame, …)` (already declared, `s2script_mm.cpp:79`), `CEntityHandle`.

**Steps:**

- [ ] **Step 1 — Re-verify the sig + vtable slot offline (2 minutes, no live server).** Scratch script (do not commit) against the pinned binary:

```bash
python3 - <<'EOF'
import struct
P = "/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so"
data = open(P,'rb').read()
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
sig = "55 48 89 E5 41 54 53 48 89 FB E8 ? ? ? ? 48 85 C0 74 ? 48 89 C7 48 8B 00 FF 90 ? ? ? ? 84 C0"
hits = scan(sig)
print("Respawn sig hits:", len(hits), ["va=%#x"%(xv+h) for h in hits])
# RTTI cross-check: primary CCSPlayerController vtable via typeinfo + .rela.dyn RELATIVE addends
e_shoff=struct.unpack_from('<Q',data,0x28)[0]; shsz=struct.unpack_from('<H',data,0x3a)[0]
shn=struct.unpack_from('<H',data,0x3c)[0]; shstr=struct.unpack_from('<H',data,0x3e)[0]
sects=[struct.unpack_from('<IIQQQQIIQQ',data,e_shoff+i*shsz) for i in range(shn)]
soff=sects[shstr][4]
def sn(n):
    e=data.index(b'\0',soff+n); return data[soff+n:e].decode()
rela=[s for s in sects if sn(s[0])=='.rela.dyn'][0]
rel={}
for o in range(rela[4], rela[4]+rela[5], 24):
    r_off,r_info,r_add=struct.unpack_from('<QQq',data,o)
    if r_info&0xffffffff==8: rel[r_off]=r_add
no=data.find(b"19CCSPlayerController\x00")
nva=[s[3]+(no-s[4]) for s in sects if s[1]!=8 and s[4]<=no<s[4]+s[5]][0]
ti=[r-8 for r,a in rel.items() if a==nva][0]
prim=[r for r,a in rel.items() if a==ti and (r-8) not in rel][0]  # offset-to-top 0 (inline)
fn0=prim+8
s274=rel.get(fn0+8*274)
print("vtable[274] = %#x; sig hit == slot 274: %s" % (s274, hits and xv+hits[0]==s274))
EOF
```
Expected output: `Respawn sig hits: 1 ['va=0x14f0ce0']` and `vtable[274] = 0x14f0ce0; sig hit == slot 274: True`. If either differs, STOP — the pinned binary changed; re-run the RTTI recipe from spec §2.1/§2.2 before proceeding.

- [ ] **Step 2 — Gamedata entry.** In `gamedata/core.gamedata.jsonc`, insert directly AFTER the `ChangeTeam` entry's closing `},` (~:222):

```jsonc
    // CCSPlayerController::Respawn(this /*rdi*/) — re-activate a (dead) player's pawn; backs
    // Player.respawn. SELF-RESOLVED against OUR libserver.so (build 2000875), NOT a borrowed constant:
    // CSSharp ships this as a BARE vtable offset ({windows:272, linux:274}, no signature) — the exact
    // borrowed-index class that bit sm_slay (400 = a getter) and ChangeTeam (101 = a ret stub; real 102).
    // The index was used ONLY OFFLINE as a finding aid: RTTI-walk the primary CCSPlayerController vtable
    // (typeinfo name "19CCSPlayerController" -> typeinfo -> offset-to-top-0 vtable, slot pointers from
    // .rela.dyn RELATIVE addends), read slot 274 (= va 0x14f0ce0 on 2000875 — the LAST fn slot of the
    // primary vtable), disassemble, mask the volatile bytes. The body is a clean nullary controller
    // method: fetch an object off the controller, early-branch on a bool virtual (+0xC98, alive-check
    // shape), re-fetch, dispatch — no unique log string exists, so the semantic boot gate is RTTI
    // VTABLE MEMBERSHIP (Respawn.vtable-member): the sig-resolved address must appear among the
    // RTTI-derived primary-vtable fn slots, else the descriptor is disabled (the unique-but-WRONG trap
    // the round-control slice proved real). Masks: call rel32, jz disp, virtual-call vtable disp.
    // Validated UNIQUE (exactly 1 match) in the pinned libserver.so PF_X range; the boot gate
    // re-validates both and SCREAMS if either moves on the update treadmill. Treadmill recipe: re-run
    // the offline RTTI walk (docs/superpowers/specs/2026-07-16-player-respawn-design.md §2.1) and
    // re-derive the mask from the new slot's prologue; CSSharp's slot number is a hint, never a number.
    // NOTE: CSSharp additionally calls CBasePlayerController::SetPawn first — its sig has 0 hits on
    // 2000875 (stale); Respawn-alone is Plan A, measured at the live gate (spec §2.3).
    "Respawn": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 41 54 53 48 89 FB E8 ? ? ? ? 48 85 C0 74 ? 48 89 C7 48 8B 00 FF 90 ? ? ? ? 84 C0",
        "resolve": "direct"
      }
    },
```

- [ ] **Step 3 — C header (ABI tail) + Rust mirror + parity check.**
  - `shim/include/s2script_core.h`, after the checktransmit typedefs (~:259) append:

```c
/* player-respawn slice — APPENDED after transmit_stats; order is the ABI.
 * player_respawn(idx, serial, alive_off) -> 1 queued / 0 degraded. (idx, serial) = the player's
 * CONTROLLER entity; alive_off = the offset of its "pawn is alive" bool field (resolved by the game
 * package; no game names cross this ABI; < 0 skips the drain-time re-check). DEFERRED: the shim
 * queues into a deduped multi-entry set and drains on the next GameFrame OUTSIDE the JS isolate
 * borrow — the engine call fires player_spawn synchronously, and an inline call from JS would
 * silently skip every plugin's handlers via the try_borrow re-entrancy guard. */
typedef int (*s2_player_respawn_fn)(int idx, int serial, int alive_off);
```

  and inside the `S2EngineOps` struct, after `s2_transmit_stats_fn transmit_stats;` (:386):

```c
    /* player-respawn slice — APPENDED after transmit_stats; order is the ABI; do not reorder above. */
    s2_player_respawn_fn player_respawn;
```

  - `core/src/v8host.rs`, after the checktransmit typedefs:

```rust
// --- player-respawn slice (APPENDED after transmit_stats; order is the ABI). ENGINE-GENERIC:
// (controller idx, serial, alive-bool field offset) -> 1 queued / 0 degraded. The shim defers the
// sig-resolved engine call to the next GameFrame OUTSIDE the JS isolate borrow (it fires player_spawn
// synchronously). No game names cross the ABI.
type PlayerRespawnFn = extern "C" fn(c_int, c_int, c_int) -> c_int;
```

  after `pub transmit_stats: …` in the struct:

```rust
    // --- player-respawn slice (APPENDED after transmit_stats; order is the ABI; do not reorder above) ---
    pub player_respawn: Option<PlayerRespawnFn>,
```

  and in BOTH full in-test `S2EngineOps { ... }` literals (after each literal's `transmit_*`/tail fields — the full literal at ~:11167 and `mock_event_ops` at ~:12028; `transmit_test_ops` spreads and needs nothing): `player_respawn: None,`
  - Run the ordered-field-name parity diff BEFORE committing (empty on the pre-change tree, empty again after; after: both lists end `… transmit_stats, player_respawn`):

```bash
diff <(awk '/^typedef struct \{/{n=0} /^[ \t]+s2_[a-z0-9_]+_fn/{sub(/;.*/,"",$2); f[++n]=$2} /^\} S2EngineOps;/{for(i=1;i<=n;i++) print f[i]; exit}' shim/include/s2script_core.h) \
     <(sed -n '/^pub struct S2EngineOps {/,/^}/p' core/src/v8host.rs | sed -nE 's/^[ \t]*pub ([a-z0-9_]+):.*/\1/p')
```

- [ ] **Step 4 — TDD: failing degrade test first.** In `core/src/v8host.rs`, beside `player_change_team_degrades_without_op` (:11445) add:

```rust
    /// player-respawn slice (degrade-never-crash): with NO engine ops installed,
    /// `__s2_player_respawn` returns 0 (an int, never undefined) and never throws — the
    /// `Player.respawn() -> false` degrade contract holds without a shim.
    #[test]
    fn player_respawn_degrades_without_op() {
        LOG.lock().unwrap().clear();
        init(logger).unwrap();
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_player_respawn(3, 7, 1988))"), "0", "degrades to 0 without ops");
        assert_eq!(eval_in_context_string("p", "String(__s2_player_respawn())"), "0", "no-args degrades to 0, no throw");
        shutdown();
    }
```

Run: `cargo test -p s2script-core player_respawn_degrades` — **Expected: FAILS** (`__s2_player_respawn is not defined` → the eval returns an error/undefined). This is the red step.

- [ ] **Step 5 — Core native + registration (green).** In `core/src/v8host.rs`, beside `s2_player_change_team` (:5196):

```rust
/// `__s2_player_respawn(index, serial, aliveOff) -> 0|1` — queue a player respawn via the
/// sig-resolved engine op. (index, serial) identify the player's CONTROLLER entity; aliveOff is the
/// offset of its "pawn is alive" bool field (supplied by the game package — engine-generic here;
/// < 0 skips the shim's drain-time alive re-check). 1 = queued: the shim executes on the NEXT
/// GameFrame, outside the JS isolate borrow, so the resulting player_spawn dispatches to ALL
/// plugins (including the caller). 0 = degraded (no op / unresolved signature / stale controller).
fn s2_player_respawn(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        if args.length() < 2 { return; }
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as c_int;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as c_int;
        let alive_off = if args.length() >= 3 { args.get(2).integer_value(scope).unwrap_or(-1) as c_int } else { -1 };
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.player_respawn else { return };
        rv.set_int32(f(index, serial, alive_off));
    }));
}
```

Register beside the change_team `set_native` (:7030): `set_native(scope, global_obj, "__s2_player_respawn", s2_player_respawn);`
Run: `cargo test -p s2script-core player_respawn_degrades` — **Expected: ok. 1 passed.** Then `cargo test -p s2script-core` — all green.

- [ ] **Step 6 — Shim: vtable-member validator + op + pending set + drain hook.** In `shim/src/s2script_mm.cpp`, directly after the `s2_player_change_team` block (~:1248):

```cpp
// ---------------------------------------------------------------------------
// player_respawn (player-respawn slice) — re-activate a (dead) player via the sig-resolved
// CCSPlayerController::Respawn(this) (s_pRespawn, loaded in Load behind TWO gates: unique-match AND
// the Respawn.vtable-member RTTI check — CSSharp ships a BARE vtable index here, the sm_slay/ChangeTeam
// borrowed-index failure class, so the shipped sig must prove it landed on a genuine CCSPlayerController
// virtual). DEFERRED EXECUTION: Respawn fires player_spawn SYNCHRONOUSLY; called inline from a JS
// native (inside the core's isolate borrow) the re-entry would be try_borrow-skipped and EVERY plugin
// would silently miss the event. So the op only enqueues into a deduped MULTI-ENTRY pending set
// (TTT's round-start loops respawn many players in one dispatch — unlike terminate-round, latest-wins
// would be a correctness bug) and Hook_GameFrameRespawnDrain (installed eagerly at Load iff both gates
// passed) executes OUTSIDE the JS borrow. (idx, serial) = the CONTROLLER entity; alive_off = the
// "pawn is alive" bool offset from the game package (re-checked at drain to close the enqueue->drain
// TOCTOU; < 0 skips the re-check). Serial-gated at BOTH enqueue and drain; .text-guarded like
// ChangeTeam. NOTE Plan A (spec §2.3): Respawn ALONE, no SetPawn pre-call — CSSharp's SetPawn sig is
// STALE on 2000875 (0 hits); if the live gate shows a dead player is not re-activated, Plan B is a
// pawn.js schema pre-write (m_hPawn <- m_hPlayerPawn), zero shim changes.
// ---------------------------------------------------------------------------
typedef void (*Respawn_t)(void* controller);
static Respawn_t s_pRespawn = nullptr;                   // sig-resolved fn ptr (loaded in Load, dual-gated)
struct PendingRespawn { uint32_t handle; int aliveOff; };
static const int kRespawnPendingMax = 130;               // > 64 slots * controller+margin; engine-generic cap
static PendingRespawn s_pendingRespawn[kRespawnPendingMax];
static int s_pendingRespawnCount = 0;
static bool s_respawnDrainHooked = false;                // Load-installed, Unload-removed

static int s2_player_respawn(int idx, int serial, int alive_off) {
    if (!s_pRespawn) return 0;                           // unresolved / failed-gate -> degrade
    CEntityHandle h(idx, serial);
    if (!s2_deref_handle(static_cast<unsigned int>(h.ToInt()))) return 0;  // stale NOW; re-gated at drain
    uint32_t hv = static_cast<uint32_t>(h.ToInt());
    for (int i = 0; i < s_pendingRespawnCount; i++)
        if (s_pendingRespawn[i].handle == hv) return 1;  // dedupe: double-respawn-same-frame is idempotent
    if (s_pendingRespawnCount >= kRespawnPendingMax) {
        META_CONPRINTF("[s2script] player_respawn: pending set full (%d) — rejected\n", kRespawnPendingMax);
        return 0;
    }
    s_pendingRespawn[s_pendingRespawnCount++] = { hv, alive_off };
    return 1;
}
```

Then, beside `Hook_GameFramePre`'s definition (~:3771), add the drain member:

```cpp
void S2ScriptPlugin::Hook_GameFrameRespawnDrain(bool, bool, bool) {
    if (s_pendingRespawnCount > 0) {
        PendingRespawn batch[kRespawnPendingMax];
        int n = s_pendingRespawnCount;
        std::memcpy(batch, s_pendingRespawn, sizeof(PendingRespawn) * n);
        s_pendingRespawnCount = 0;                       // consume BEFORE calling (the call re-enters)
        const uint8_t* f = reinterpret_cast<const uint8_t*>(s_pRespawn);
        if (!s_pRespawn || !s_serverText || f < s_serverText || f >= s_serverText + s_serverTextSize) {
            META_CONPRINTF("[s2script] player_respawn: fn out of libserver .text at drain — batch dropped\n");
            RETURN_META(MRES_IGNORED);
        }
        for (int i = 0; i < n; i++) {
            void* controller = s2_deref_handle(batch[i].handle);   // re-gate: it can die in between
            if (!controller) {
                META_CONPRINTF("[s2script] player_respawn: stale controller at drain — skipped\n");
                continue;
            }
            if (batch[i].aliveOff >= 0 &&
                *reinterpret_cast<const uint8_t*>(reinterpret_cast<const char*>(controller) + batch[i].aliveOff)) {
                continue;                                // came alive between enqueue and drain — skip
            }
            // OUTSIDE the JS isolate borrow: the synchronous player_spawn flows through the normal
            // FireEvent pre-hook -> core dispatch -> every plugin's subscribers.
            s_pRespawn(controller);
        }
    }
    RETURN_META(MRES_IGNORED);
}
```

And in `shim/src/s2script_mm.h`, after `void Hook_GameFramePre(bool simulating, bool first, bool last);` (:47):

```cpp
    void Hook_GameFrameRespawnDrain(bool simulating, bool first, bool last);
```

- [ ] **Step 7 — Shim: vtable-member gate helper.** Beside the other gamedata-validation helpers (after `ResolveSigValidated`, ~:2040):

```cpp
// Semantic load-gate for the Respawn descriptor (uniqueness is NOT enough — the round-control slice
// proved a sig can match exactly once at the WRONG function, and Respawn has no unique log string to
// xref). Runtime-resolves the CCSPlayerController PRIMARY vtable via RTTI (s2vtable::GetVTableByName —
// the trace-slice precedent) and asserts the sig-resolved address is one of its fn slots. The walk
// ends at the first slot value outside libserver .text (the next sub-vtable's offset-to-top header) —
// fail-closed: a truncated walk that misses the fn FAILS the gate, it never passes wrongly. Logs the
// matched slot as a treadmill breadcrumb (CSSharp's offline hint was 274 on 2000875).
static bool ValidateRespawnVtableMember(const uint8_t* fn, const ModText& mt) {
    void** vt = s2vtable::GetVTableByName("libserver.so", "CCSPlayerController");
    if (!vt) return false;
    for (int i = 0; i < 512; i++) {
        const uint8_t* p = reinterpret_cast<const uint8_t*>(vt[i]);
        if (!p || p < mt.text || p >= mt.text + mt.size) break;   // sub-vtable header = end of fn slots
        if (p == fn) {
            META_CONPRINTF("[s2script] Respawn = CCSPlayerController vtable slot %d\n", i);
            return true;
        }
    }
    return false;
}
```

- [ ] **Step 8 — Shim: Load resolve (dual-gated) + eager drain hook + wiring + Unload.** In `S2ScriptPlugin::Load`, directly after the `ChangeTeam` resolve block (~:3100):

```cpp
            // player-respawn slice: resolve CCSPlayerController::Respawn (Player.respawn).
            // DUAL-GATED: unique-match (ResolveSigValidated) AND RTTI vtable membership — CSSharp
            // ships a bare vtable index here (the sm_slay-400/ChangeTeam-101 borrowed-index class),
            // so the shipped self-derived sig must additionally prove it landed on a genuine
            // CCSPlayerController virtual. Failure of either gate leaves s_pRespawn null -> the op
            // degrades to 0 (degrade-never-crash) and the drain hook is never installed.
            auto rsit = sigs.find("Respawn");
            if (rsit == sigs.end()) {
                GamedataResult("Respawn", false, "signature absent from gamedata");
            } else {
                int64_t rsOff = ResolveSigValidated("Respawn", rsit->second);
                ModText rsmt = FindModuleText(rsit->second.module.c_str());
                if (rsOff != s2sig::kFail && rsmt.text) {
                    const uint8_t* rsfn = rsmt.text + rsOff;
                    if (!ValidateRespawnVtableMember(rsfn, rsmt)) {
                        GamedataResult("Respawn.vtable-member", false,
                            "sig-resolved address is NOT a member of the RTTI-derived "
                            "CCSPlayerController primary vtable (unique-but-WRONG match — the "
                            "borrowed-sig trap); descriptor disabled");
                    } else {
                        GamedataResult("Respawn.vtable-member", true, nullptr);
                        s_pRespawn = reinterpret_cast<Respawn_t>(const_cast<uint8_t*>(rsfn));
                        s_serverText = rsmt.text; s_serverTextSize = rsmt.size;
                        META_CONPRINTF("[s2script] Respawn resolved @%p (Player.respawn)\n",
                                       reinterpret_cast<void*>(s_pRespawn));
                        // Eager drain-hook install (NOT lazy): adding a SourceHook from inside a
                        // frame dispatch would mutate the hook chain mid-iteration; one
                        // if-nothing-pending branch per frame is negligible.
                        if (m_server && !s_respawnDrainHooked) {
                            SH_ADD_HOOK(ISource2Server, GameFrame, m_server,
                                        SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRespawnDrain), false);
                            s_respawnDrainHooked = true;
                        }
                    }
                }   // rsOff == kFail: ResolveSigValidated already recorded the reason
            }
```

Wire the op after `ops.transmit_stats = &s2_transmit_stats;` (:3589):

```cpp
    // player-respawn slice — APPENDED after transmit_stats; order MUST match S2EngineOps.
    ops.player_respawn = &s2_player_respawn;
```

In `S2ScriptPlugin::Unload`, beside the existing GameFrame `SH_REMOVE_HOOK` pair (~:3650):

```cpp
    if (s_respawnDrainHooked) {
        SH_REMOVE_HOOK(ISource2Server, GameFrame, m_server,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_GameFrameRespawnDrain), false);
        s_respawnDrainHooked = false;
        s_pendingRespawnCount = 0;
    }
```

- [ ] **Step 9 — Build + gates + commit (PR2).**

```bash
make core && make shim
cargo test -p s2script-core
make check-boundary
./scripts/test-boundary-nameleak.sh
```
Expected: both builds green; all tests pass (incl. `player_respawn_degrades_without_op`); both boundary gates green — the `core/src` diff contains only opaque ints ("CCSPlayerController" appears in shim/gamedata only).

```bash
git add gamedata/core.gamedata.jsonc shim/include/s2script_core.h shim/src/s2script_mm.cpp shim/src/s2script_mm.h core/src/v8host.rs
gt create respawn/engine-fact -m "feat(respawn): CCSPlayerController::Respawn engine fact — self-derived sig + RTTI vtable-member gate + deferred respawn op"
```

---

## Task 3 (PR3, `respawn/cs2-surface`): Player.respawn in @s2script/cs2

**Files:**
- Modify: `games/cs2/js/pawn.js` (Player.prototype.respawn, after `.spectate` ~:115)
- Modify: `packages/cs2/index.d.ts` (`respawn(): boolean` after `spectate()` :125)
- Create: `.changeset/player-respawn-cs2.md`

**Interfaces:**
- Consumes: `__s2_player_respawn` (Task 2), `__s2_schema_offset`, the generated `pawnIsAlive` accessor (`games/cs2/js/schema.generated.js:402`, `CCSPlayerController.m_bPawnIsAlive`).
- Produces: `Player.respawn(): boolean` exactly as spec §3.

**Steps:**

- [ ] **Step 1 — pawn.js.** In `games/cs2/js/pawn.js`, after `Player.prototype.spectate` (~:115):

```js
  // player.respawn() — respawn this (dead) player via the self-resolved CCSPlayerController::Respawn
  // (byte-sig + RTTI-vtable-membership boot-gated; NEVER CSSharp's borrowed vtable index). QUEUED: the
  // shim executes on the NEXT GameFrame outside the JS isolate borrow, so the resulting player_spawn
  // reaches EVERY plugin — safe to call from event/command handlers, no nextFrame wrapping needed
  // (TTT's Server.NextWorldUpdate semantics, built in). The alive-guard runs here (game-side: the
  // CS2 field name stays out of core) AND at drain via the passed offset (closes the 1-frame TOCTOU).
  // Returns false when already alive, the ref is stale, or the Respawn descriptor is degraded.
  // Plan B (spec §2.3, live-gate fallback — do NOT enable unless gate item 2 fails): before the
  // native call, re-point the active pawn from schema —
  //   var hp = this.ref.readUInt32(__s2_schema_offset("CCSPlayerController", "m_hPlayerPawn"));
  //   var ho = __s2_schema_offset("CBasePlayerController", "m_hPawn");
  //   if (hp !== null && ho >= 0 && this.ref.writeUInt32(ho, hp >>> 0)) this.ref.notifyStateChanged(ho);
  Player.prototype.respawn = function () {
    if (this.pawnIsAlive === true) return false;
    if (typeof __s2_player_respawn !== "function") return false;
    var aliveOff = __s2_schema_offset("CCSPlayerController", "m_bPawnIsAlive");
    return __s2_player_respawn(this.ref.index, this.ref.serial, aliveOff) === 1;
  };
```

- [ ] **Step 2 — index.d.ts.** In `packages/cs2/index.d.ts`, after `spectate(): void;` (:125):

```ts
  /** Respawn this (dead) player via the self-resolved CCSPlayerController::Respawn (byte-sig +
   *  RTTI-vtable-membership load-validated). QUEUED: the engine call executes on the NEXT engine
   *  frame, outside the JS isolate borrow, so the resulting player_spawn reaches EVERY plugin's
   *  handlers — including the caller's. Safe from inside event/command handlers; no nextFrame
   *  wrapping needed. Returns false when degraded: the player is already alive, the ref is stale,
   *  or the Respawn descriptor failed its boot gates. */
  respawn(): boolean;
```

- [ ] **Step 3 — Changeset.** Create `.changeset/player-respawn-cs2.md`:

```md
---
"@s2script/cs2": minor
---

Player.respawn(): respawn a dead player via the self-resolved CCSPlayerController::Respawn
(byte-sig + RTTI-vtable-membership load-validated; queued one frame outside the JS isolate borrow
so player_spawn reaches every plugin). Alive-guarded, serial-gated, degrades to false.
```

- [ ] **Step 4 — Gates + commit (PR3).**

```bash
./scripts/check-plugins-typecheck.sh
make check-boundary
cargo test -p s2script-core
```
Expected: all green (the .d.ts addition is additive; existing plugins still typecheck).

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts .changeset/player-respawn-cs2.md
gt create respawn/cs2-surface -m "feat(cs2): Player.respawn — deferred, alive-guarded respawn surface"
```

---

## Task 4 (PR4, `respawn/demo`): respawn-demo + host gates

**Files:**
- Create: `examples/respawn-demo/package.json`, `examples/respawn-demo/tsconfig.json`, `examples/respawn-demo/src/plugin.ts`
- Modify: `docs/PROGRESS.md` (slice entry stub; the live-gate result line is filled by the MAIN LOOP after the gate)

**Steps:**

- [ ] **Step 1 — Demo plugin.** `examples/respawn-demo/package.json`:

```json
{
  "name": "@demo/respawn-demo",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x"
  }
}
```

`examples/respawn-demo/tsconfig.json`:

```json
{
  "extends": "../../tsconfig.base.json",
  "include": ["src", "../../packages/globals/globals.d.ts"]
}
```

`examples/respawn-demo/src/plugin.ts`:

```ts
// @demo/respawn-demo — live gate for the player-respawn slice.
//
//   sm_respawn <slot>   — Player.respawn on one slot. Called FROM a command handler (a JS dispatch —
//                         the exact re-entrancy hazard path); the player_spawn logger below firing
//                         for OUR OWN respawn is the deferred-drain proof.
//   sm_respawnall       — the TTT round-start loop shape: respawn every dead player in ONE dispatch
//                         (multi-entry pending-set proof — all of them must spawn, not just the last).
//
// player_spawn logs slot + pawnIsAlive + health + origin (fresh-spawn-position check);
// player_death logs the death so the gate script can kill-then-respawn deterministically.

import { Events } from "@s2script/sdk/events";
import { Commands } from "@s2script/sdk/commands";
import { Player } from "@s2script/cs2";

export function onLoad(): void {
  Events.on("player_spawn", (e) => {
    const slot = e.getInt("userid");
    const p = Player.fromSlot(slot);
    const pawn = p ? p.pawn : null;
    console.log(`[respawn-demo] player_spawn userid=${slot} alive=${p ? p.pawnIsAlive : null} health=${pawn ? pawn.health : null} origin=${pawn && pawn.origin ? pawn.origin.toString() : null}`);
  });

  Events.on("player_death", (e) => {
    console.log(`[respawn-demo] player_death userid=${e.getInt("userid")}`);
  });

  Commands.register("respawn", (ctx) => {
    const slot = ctx.argInt(0, 0);
    const p = Player.fromSlot(slot);
    if (!p) { ctx.reply(`respawn: no player in slot ${slot}`); return; }
    const wasAlive = p.pawnIsAlive;
    const ok = p.respawn();
    ctx.reply(`respawn slot=${slot} wasAlive=${wasAlive} queued=${ok} (player_spawn log follows next frame if queued)`);
  });

  Commands.register("respawnall", (ctx) => {
    let queued = 0, skipped = 0;
    for (const p of Player.all()) {
      if (p.respawn()) queued++; else skipped++;
    }
    ctx.reply(`respawnall: queued=${queued} skipped=${skipped} (all queued must spawn in the same frame batch)`);
  });

  console.log("[respawn-demo] onLoad — sm_respawn / sm_respawnall registered");
}
```

(If `Player.fromSlot`/`Player.all`/`ctx.argInt` signatures differ from `packages/cs2/index.d.ts` / the commands `.d.ts` at implementation time, adapt the demo to the shipped types — the typecheck gate is authoritative.)

- [ ] **Step 2 — Build + typecheck gates.**

```bash
cd examples/respawn-demo && npx s2script build && cd ../..
./scripts/check-plugins-typecheck.sh
```
Expected: `dist/respawn-demo.s2sp` produced; typecheck gate green.

- [ ] **Step 3 — PROGRESS.md stub + commit (PR4).** Append to `docs/PROGRESS.md`: what the slice built (the one engine fact + dual boot gate + deferred op + Player.respawn), the Plan A/B/C SetPawn decision ladder with the stale-sig evidence, deferred items with reasons (SetPawn Plan C = only if A+B fail live; Pawn.respawn = deprecated upstream; respawnAll/force-flag = no consumer), and a `live-gate: PENDING (main loop)` line.

```bash
git add examples/respawn-demo docs/PROGRESS.md
gt create respawn/demo -m "demo(respawn): respawn live-gate demo + progress entry"
```

- [ ] **Step 4 — Stack submit + handoff to the main loop.** Re-check the ABI tail collision (did #67/#71/writeconvar merge? if so `gt restack`, re-anchor `player_respawn` after the new tail in the engine-fact PR, re-run the parity diff + all gates per PR), then:

```bash
gt submit --no-interactive
```

Hand off to the MAIN LOOP: sniper build (`docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`), `make package`, deploy, and the live gate per spec §9 — including THE Plan A/B decision point (dead-bot respawn) and the STOP conditions in spec §10.

---

## Self-review notes

- **Spec coverage:** §2.1-2.2 sig + vtable gate → Task 2 Steps 1-2, 7-8; §2.3 SetPawn ladder → Plan B comment in Task 3 Step 1 + gamedata comment + PROGRESS stub; §2.4 alive offset → op arg everywhere; §3 API → Task 3 (verbatim); §4.1 deferral + multi-set → Task 2 Steps 6, 8; §4.2 ABI → Task 2 Step 3 + Global Constraints + Task 4 Step 4; §5 boundary → Task 2 Step 9 / Task 3 Step 4 gates; §6-7 safety/degrade → guards in Steps 5-6 + the degrade test; §8 deferred → PROGRESS stub; §9-10 live gate/STOP → Task 4 Step 4 handoff (main-loop scope).
- **Type consistency:** `(int idx, int serial, int alive_off) -> int` is identical across the C typedef (Task 2 Step 3), Rust typedef (Step 3), native (Step 5), and shim impl (Step 6); pawn.js passes `(index, serial, aliveOff)` positionally (Task 3 Step 1).
- **TDD:** Task 2 Step 4 is a genuine red (native unregistered) before Step 5's green; the shim side is compile-gated + boot-gated (no host-runnable unit seam for SourceHooks — the live gate is its test, per every prior engine-fact slice).
- **Placeholder scan:** no TBDs; every code block is complete and paste-able; line anchors are approximate ("~:N") with structural anchors (named functions/fields) authoritative.
- **Known live-resolution items** (expected, not placeholders): the Plan A vs B outcome, the alive-player engine-behavior probe, and the fresh-spawn-position visual are live-gate outcomes owned by the main loop.
