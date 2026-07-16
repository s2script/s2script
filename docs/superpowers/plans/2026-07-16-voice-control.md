# Voice control (voiceMuted + Clients.onVoice) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A real server-side voice mute (`client.voiceMuted` get/set — the sender's outgoing voice silenced for every receiver) plus a `Clients.onVoice(handler)` transmission notification, replacing basecomm's unverified `m_bHasCommunicationAbuseMute` best-effort and closing the TTT `PlayerMuter`/`MuteReward` parity gap.

**Architecture (spec: `docs/superpowers/specs/2026-07-16-voice-control-design.md`):** Two SourceHooks on interfaces the shim ALREADY holds — a PRE param-rewrite hook on `IVEngineServer2::SetClientListening` (eiface.h:330, on `s_pEngine`) consulting a shim-resident `uint8_t s_voiceMuted[64]` (zero FFI in the O(n²) hot path), and a POST notify hook on `ISource2GameClients::ClientVoice` (eiface.h:619, the 7th sibling on `m_gameClients`) throttled to ≤1 dispatch/slot/second into the existing name-keyed `CLIENT_MUX` as event `"voice"`. Two new ops (`voice_set_muted`/`voice_get_muted`) appended at the `S2EngineOps` tail. **No sig-scan, no gamedata, no detour, no game-package change.** Doctrine validation: first-fire arg sanity on the hook + a one-shot `Get/SetClientListening` round-trip when the 2nd client goes active; either failure → named degrade, rewrite disabled, ops return 0/-1.

**Tech Stack:** Rust core (`core/src/{v8host,ffi(unchanged)}.rs`), C++ shim (`shim/src/s2script_mm.{cpp,h}`, `shim/include/s2script_core.h`), `packages/sdk/clients.d.ts`, `plugins/basecomm`, `examples/voice-demo`. Graphite stack of 3 PRs.

## Graphite stack map

| PR | Branch (gt) | Content |
|---|---|---|
| PR1 | `feat/voice-core` | ABI (2 ops, all FIVE places) + natives + prelude (`voiceMuted`, `onVoice`) + `clients.d.ts` + core tests + changeset |
| PR2 | `feat/voice-shim` | shim: both SourceHooks + flag array + throttle + first-fire/round-trip validation + disconnect clear + unload removal + ops wiring + sniper build |
| PR3 | `feat/voice-consumers` | `examples/voice-demo` (TTT-shaped) + basecomm migration + live gate evidence |

Worktree `/home/gkh/projects/s2script-voice` is on `feat/voice` at origin/main. Create each task's branch with `gt create -am "<msg>"` (stacked; `gt submit --stack` at the end). If `gt` is unavailable, fall back to three plain branches PR'd in order with `--base` chaining.

## Global Constraints

- **ABI-append discipline — ALL FIVE places, and a live collision hazard.** The two new ops append after `usercmd_clear_subtick` (the current tail) in: (1) C typedef + (2) struct member (`shim/include/s2script_core.h:261/:375`), (3) Rust type alias + (4) struct field (`core/src/v8host.rs` ~:240/:359), and (5) BOTH test op-structs (`v8host.rs` ~:11042 and ~:11695 — locate with `grep -n "usercmd_clear_subtick: None" core/src/v8host.rs`). Order MUST match. **COLLISION:** the unmerged transmit and round-control stacks append at this same tail and `S2EngineOps` has no size/version handshake (copied by value) — whoever rebases onto the other MUST re-tail all five places together; a missed re-tail is a silent function-pointer misdispatch, not a compile error. Check `git log` for either stack having merged before starting, and re-anchor the tail if so.
- **Boundary — both gates stay green.** `IVEngineServer2`/`ISource2GameClients`/`CPlayerSlot` are Source2 types → shim-only. Core speaks only `(slot, muted)` ints and the event name `"voice"`. NO CS2 identifiers in `core/src`. Gates: `make check-boundary` (== `scripts/check-core-boundary.sh`) plus `bash scripts/test-boundary-nameleak.sh`.
- **RE doctrine:** both vtable positions are borrowed layout facts from a HAND-PATCHED eiface.h region (`#if 0 // Don't really match the binary` + unk301/302 before `SetClientListening`) — HINT source hl2sdk, corroborated by CSSharp (`voice_manager.cpp:26`) and Swiftly. Never trusted bare: the shim validates behaviorally (first-fire sanity + Get/Set round-trip) and degrades with a NAMED reason. Cite the ChangeTeam 102-vs-101 drift lesson in the PR body.
- **Hot path:** `Hook_SetClientListening` does array reads only — no allocation, no FFI, no JS, no logging after first-fire. `Hook_ClientVoice` throttles BEFORE the core dispatch.
- **Degrade-never-crash:** missing interface → hook not installed → ops return 0/-1, `voiceMuted` inert, `onVoice` silent; validation failure → same, with a LOUD named boot/log reason. Natives `catch_unwind`.
- **Worktree gotcha:** `third_party/` submodules are EMPTY in this worktree — run `git submodule update --init --recursive` before any shim build.
- **Sniper build (the only deployable binaries):** `docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`. Core AND shim both change → both `.so`s must rebuild.
- **Core tests are single-threaded** (`.cargo/config.toml`): `cargo test -p s2script-core` — do not pass `--test-threads`.
- **Naming:** PascalCase types, camelCase members (`voiceMuted`, `onVoice`). Plugins are pure ESM.

## File Structure

- `shim/include/s2script_core.h` — +2 op typedefs + 2 tail members. *(Task 1 — the ABI contract.)*
- `core/src/v8host.rs` — 2 type aliases + 2 struct fields + both test op-structs; 2 natives + registration; prelude `voiceMuted` property + `onVoice`; tests. *(Task 1.)*
- `packages/sdk/clients.d.ts` — `Client.voiceMuted` + `Clients.onVoice`. *(Task 1.)*
- `.changeset/voice-control.md` — minor `@s2script/sdk`. *(Task 1.)*
- `shim/src/s2script_mm.h` — 2 hook member decls. *(Task 2.)*
- `shim/src/s2script_mm.cpp` — SH_DECLs, voice statics + ops + validation helper, hook bodies, installs, disconnect clear, unload removal, ops wiring. *(Task 2.)*
- `examples/voice-demo/{package.json,tsconfig.json,src/plugin.ts}` (new) + `plugins/basecomm/src/plugin.ts` (migration). *(Task 3.)*

---

## Task 1 (PR1): Core ABI + natives + prelude + types + tests

**Files:** modify `shim/include/s2script_core.h`, `core/src/v8host.rs`, `packages/sdk/clients.d.ts`; create `.changeset/voice-control.md`.

**Interfaces:**
- Produces (consumed by Task 2): C ops at the struct tail —
  `int (*voice_set_muted)(int slot, int muted)` (1 = recorded+enforceable, 0 = bad slot or degraded) and
  `int (*voice_get_muted)(int slot)` (1 muted / 0 not / -1 bad slot or degraded). The shim fills both.
- Produces (JS-facing): `Client.prototype.voiceMuted` (boolean get/set), `Clients.onVoice(handler(client))` — the handler receives a `Client` for the dispatched slot via the existing `"voice"` `CLIENT_MUX` name (Task 2's shim dispatches it; no core dispatch change needed).

### Steps

- [ ] **Step 1: C ABI — typedefs + tail members.**

In `shim/include/s2script_core.h`, after the `s2_usercmd_clear_subtick_fn` typedef (:261):

```c
/* Voice-control slice — APPENDED after usercmd_clear_subtick; order is the ABI.
 * voice_set_muted: set/clear the per-slot server-side voice mute (sender -> ALL receivers). The flag
 * lives SHIM-side: the SetClientListening pre-hook consults it allocation-free (O(n^2) per game voice
 * refresh), so JS only flips state through this op. Returns 1 = recorded + enforceable; 0 = slot out
 * of range OR the voice descriptor is degraded (hook missing / vtable validation failed) — the flag
 * is then inert and the shim has logged the named reason.
 * voice_get_muted: 1 = muted, 0 = not muted, -1 = slot out of range / degraded. */
typedef int (*s2_voice_set_muted_fn)(int slot, int muted);
typedef int (*s2_voice_get_muted_fn)(int slot);
```

Struct members after `s2_usercmd_clear_subtick_fn usercmd_clear_subtick;` (:375), before `} S2EngineOps;`:

```c
    /* Voice-control slice — APPENDED after usercmd_clear_subtick; order is the ABI. */
    s2_voice_set_muted_fn  voice_set_muted;
    s2_voice_get_muted_fn  voice_get_muted;
```

- [ ] **Step 2: Rust ABI mirror — aliases + fields + both test op-structs.**

In `core/src/v8host.rs`, next to the `UsercmdClearSubtickFn` alias (locate: `grep -n "UsercmdClearSubtickFn =" core/src/v8host.rs`):

```rust
type VoiceSetMutedFn = extern "C" fn(c_int, c_int) -> c_int;
type VoiceGetMutedFn = extern "C" fn(c_int) -> c_int;
```

Struct fields after `pub usercmd_clear_subtick: Option<UsercmdClearSubtickFn>,` (:359):

```rust
    // Voice-control slice — APPENDED after usercmd_clear_subtick; order is the ABI.
    pub voice_set_muted:        Option<VoiceSetMutedFn>,
    pub voice_get_muted:        Option<VoiceGetMutedFn>,
```

Then `grep -n "usercmd_clear_subtick: None" core/src/v8host.rs` (two hits, ~:11042 and ~:11695) and add after EACH:

```rust
            voice_set_muted: None,
            voice_get_muted: None,
```

- [ ] **Step 3: The two natives + registration.**

In `core/src/v8host.rs`, next to `s2_client_kick` (~:7630):

```rust
/// `__s2_voice_set_muted(slot, on)` -> bool. Voice-control slice: set/clear the shim-side per-slot
/// voice-mute flag (sender -> all receivers, enforced by the shim's SetClientListening rewrite).
/// Returns false when degraded (no op / bad slot / voice descriptor disabled) — the prelude setter
/// ignores it (degrade contract: inert no-op; the shim logs the named reason once).
fn s2_voice_set_muted(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 2 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let on = if args.get(1).boolean_value(scope) { 1 } else { 0 };
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_set_muted else { return };
        rv.set_bool(f(slot, on) != 0);
    }));
}

/// `__s2_voice_get_muted(slot)` -> i32 (1 muted / 0 not / -1 degraded-or-invalid). The prelude getter
/// maps `=== 1` to boolean, so degraded reads are `false` (never a phantom mute).
fn s2_voice_get_muted(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_get_muted else { return };
        rv.set_int32(f(slot));
    }));
}
```

Register in `install_natives`, after the `__s2_client_kick` line (~:6838):

```rust
    // Voice-control slice: per-slot voice mute set/get (shim-side flag consulted by the
    // SetClientListening rewrite hook; JS never sits in that hot path).
    set_native(scope, global_obj, "__s2_voice_set_muted", s2_voice_set_muted);
    set_native(scope, global_obj, "__s2_voice_get_muted", s2_voice_get_muted);
```

- [ ] **Step 4: Prelude — `voiceMuted` property + `Clients.onVoice`.**

In the clients prelude (v8host.rs ~:1851), after the `"ip"` `Object.defineProperty` block and before `var __s2_MAX_CLIENTS = 64;`, insert:

```js
  // Voice-control slice: server-side voice mute (this client's OUTGOING voice silenced for every
  // receiver — the shim's SetClientListening rewrite). Framework state: cleared on disconnect. When
  // the voice descriptor is degraded the setter is an inert no-op (shim logs the named reason) and
  // reads stay false (get_muted -1/0 both map to false).
  Object.defineProperty(Client.prototype, "voiceMuted", {
    get: function () { return __s2_voice_get_muted(this.slot) === 1; },
    set: function (on) { __s2_voice_set_muted(this.slot, !!on); }
  });
```

In the `__s2_clients` object (~:1856-1861), after the `onSettingsChanged` line, add:

```js
    // Fires while a client transmits voice (throttled shim-side to <=1 dispatch/slot/second; the FIRST
    // packet of a transmission always fires). Never fires for bots.
    onVoice:           function (h) { __s2_client_on("voice", h); },
```

- [ ] **Step 5: `packages/sdk/clients.d.ts`.**

In the `Client` class, after the `kickWithReason` declaration (:29), add:

```ts
  /**
   * Server-side voice mute: while true, this client's OUTGOING voice is silenced for every receiver.
   * Framework state (not an engine field): cleared automatically on disconnect, persists across map
   * changes while connected. If the voice descriptor is degraded (hook/validation failure — named
   * reason in the server log), setting is an inert no-op and reads stay false.
   */
  voiceMuted: boolean;
```

In the `Clients` const, after the `onSettingsChanged` entry (:43), add:

```ts
  /**
   * Fires while a client transmits voice. Throttled to at most one dispatch per client per second;
   * the FIRST packet of a transmission always fires (a lazy mute-on-talk lands immediately).
   * Handlers should be idempotent. Never fires for bots.
   */
  onVoice(handler: (client: Client) => void): void;
```

- [ ] **Step 6: Core in-isolate tests.**

In the `#[cfg(test)]` module of `core/src/v8host.rs`, next to the existing clients tests (after `client_dispatch_delivers_client_with_slot`, ~:10545). Helpers first (by the other capture helpers):

```rust
    thread_local! { static VOICE_MUTED_CAPTURE: std::cell::RefCell<[i32; 64]> = std::cell::RefCell::new([0; 64]); }
    extern "C" fn capture_voice_set_muted(slot: c_int, muted: c_int) -> c_int {
        if !(0..64).contains(&slot) { return 0; }
        VOICE_MUTED_CAPTURE.with(|a| a.borrow_mut()[slot as usize] = if muted != 0 { 1 } else { 0 });
        1
    }
    extern "C" fn capture_voice_get_muted(slot: c_int) -> c_int {
        if !(0..64).contains(&slot) { return -1; }
        VOICE_MUTED_CAPTURE.with(|a| a.borrow()[slot as usize])
    }
```

Then the three tests:

```rust
    /// Voice-control: Client.voiceMuted round-trips through the voice_set_muted/voice_get_muted ops
    /// (set writes the shim-side flag; get maps 1 -> true, 0 -> false).
    #[test]
    fn voice_muted_property_round_trips_through_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(Some(S2EngineOps {
            voice_set_muted: Some(capture_voice_set_muted),
            voice_get_muted: Some(capture_voice_get_muted),
            ..mock_event_ops()
        }));
        VOICE_MUTED_CAPTURE.with(|a| *a.borrow_mut() = [0; 64]);
        create_plugin_context("pvm");
        assert_eq!(eval_in_context_string("pvm",
            "var c = new __s2pkg_clients.Client(5); c.voiceMuted = true; String(c.voiceMuted)"), "true");
        assert_eq!(VOICE_MUTED_CAPTURE.with(|a| a.borrow()[5]), 1, "op received (5, 1)");
        assert_eq!(eval_in_context_string("pvm", "c.voiceMuted = false; String(c.voiceMuted)"), "false");
        assert_eq!(VOICE_MUTED_CAPTURE.with(|a| a.borrow()[5]), 0, "op received (5, 0)");
        shutdown();
    }

    /// Voice-control degrade: with no engine ops the setter is a silent no-op and reads are false
    /// (get_muted degrades to -1, which must NOT read as muted).
    #[test]
    fn voice_muted_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("pvd");
        assert_eq!(eval_in_context_string("pvd",
            "var c = new __s2pkg_clients.Client(2); c.voiceMuted = true; String(c.voiceMuted)"), "false");
        shutdown();
    }

    /// Voice-control: Clients.onVoice subscribes on the existing CLIENT_MUX under the "voice" name —
    /// a dispatched "voice" event delivers a Client with the slot; other names don't cross-fire.
    #[test]
    fn voice_client_event_dispatches_to_on_voice() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        load_plugin_js("pvv", r#"
            __s2pkg_clients.Clients.onVoice(function (c) {
                globalThis.__v_ran  = (globalThis.__v_ran || 0) + 1;
                globalThis.__v_slot = c.slot;
            });
        "#, "{}");
        dispatch_client_event("voice", 4);
        assert_eq!(read_i32_global_in("pvv", "__v_ran"), 1, "onVoice handler runs once");
        assert_eq!(read_i32_global_in("pvv", "__v_slot"), 4, "handler receives the dispatched slot");
        dispatch_client_event("settingschanged", 4);   // a different name must not re-run it
        assert_eq!(read_i32_global_in("pvv", "__v_ran"), 1);
        shutdown();
    }
```

- [ ] **Step 7: Run tests + gates.**

```bash
cargo test -p s2script-core voice_          # expected: 3 new tests PASS
cargo test -p s2script-core                 # expected: full suite, 0 failures (previous count + 3)
make check-boundary                          # expected: PASS
bash scripts/test-boundary-nameleak.sh       # expected: PASS
( cd packages/cli && node build.mjs ) && bash scripts/check-plugins-typecheck.sh   # expected: PASS (new .d.ts compiles)
```

- [ ] **Step 8: Changeset + commit (PR1).**

`.changeset/voice-control.md`:

```md
---
"@s2script/sdk": minor
---

Voice control: `Client.voiceMuted` (get/set — server-side mute of the client's outgoing voice for all
receivers, enforced by a SetClientListening rewrite hook) and `Clients.onVoice(handler)` (throttled
voice-transmission notification). Degrades to an inert no-op with a named reason if the voice
descriptor fails validation.
```

```bash
git add shim/include/s2script_core.h core/src/v8host.rs packages/sdk/clients.d.ts .changeset/voice-control.md
gt create -am "feat(voice): core ABI + Client.voiceMuted + Clients.onVoice + tests

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 2 (PR2): Shim — both SourceHooks, validation, ops wiring, sniper build

**Files:** modify `shim/src/s2script_mm.h`, `shim/src/s2script_mm.cpp`.

**Interfaces:**
- Consumes (Task 1): the C ops `voice_set_muted`/`voice_get_muted` (fills them) + the existing extern `s2script_core_dispatch_client_event` (dispatches `"voice"`).
- Produces: `Hook_SetClientListening` (PRE, param-rewrite), `Hook_ClientVoice` (POST, throttled notify), `MaybeValidateVoiceListening()` (one-shot round-trip), voice statics.

### Steps

- [ ] **Step 0: Submodules.** `git submodule update --init --recursive` (the worktree's `third_party/` is empty; the shim build fails confusingly without this).

- [ ] **Step 1: SH_DECLs.**

In `shim/src/s2script_mm.cpp`, after the six lifecycle SH_DECLs (:95-100), add:

```cpp
// Voice-control slice. ClientVoice (eiface.h:619 "TERROR: A player sent a voice packet") = the 7th
// sibling notify hook on m_gameClients — fires PER VOICE PACKET, throttled in the handler before the
// core dispatch. SetClientListening (eiface.h:330) = the CSSharp/Swiftly voice-mute mechanism: a PRE
// hook on s_pEngine that rewrites bListen->false for a muted sender. CAUTION: it sits in a
// HAND-PATCHED eiface.h region ('#if 0 Don't really match the binary' + unk301/302) — behaviorally
// validated at runtime (first-fire sanity + a Get/Set round-trip), named-degrade on mismatch.
SH_DECL_HOOK1_void(ISource2GameClients, ClientVoice, SH_NOATTRIB, 0, CPlayerSlot);                       // :619
SH_DECL_HOOK3(IVEngineServer2, SetClientListening, SH_NOATTRIB, 0, bool, CPlayerSlot, CPlayerSlot, bool); // :330
```

- [ ] **Step 2: Hook member decls in `shim/src/s2script_mm.h`.**

After `void Hook_ClientSettingsChanged(CPlayerSlot slot);` (:63):

```cpp
    // Voice-control slice: throttled voice-packet notify (dispatches client event "voice") + the
    // listen-matrix rewrite that enforces the per-slot mute (shim-resident flag array, zero FFI).
    void Hook_ClientVoice(CPlayerSlot slot);
    bool Hook_SetClientListening(CPlayerSlot receiver, CPlayerSlot sender, bool bListen);
```

- [ ] **Step 3: Voice statics + ops + validation helper.**

In `shim/src/s2script_mm.cpp`, after the `s2_client_kick` op (~:1056; must be after `s_pEngine` (:594) and `s2_client_valid` (:639) are in scope), add:

```cpp
// ---------------------------------------------------------------------------
// Voice-control slice. The mute is FRAMEWORK state (CSSharp keeps CPlayer::m_voiceFlag the same way —
// no engine/schema mute bit exists): a shim-resident flag array consulted by the SetClientListening
// PRE hook, which rewrites bListen->false whenever the SENDER is muted. The hook fires per
// (receiver, sender) pair per game voice refresh (up to O(n^2)) — everything here is plain array
// reads, no FFI/JS/allocations. Doctrine: the vtable slots come from a hand-patched eiface.h region,
// so enforcement is gated on runtime validation (first-fire arg sanity + a one-shot Get/Set
// round-trip once two clients are active); any failure -> named degrade, ops return 0/-1.
// ---------------------------------------------------------------------------
static uint8_t s_voiceMuted[kMaxClientSlots] = {0};        // 1 = sender muted for all receivers
static time_t  s_voiceLastNotify[kMaxClientSlots] = {0};   // per-slot ClientVoice throttle (<=1/s)
static bool    s_voiceNotifyHookInstalled = false;         // ClientVoice POST hook on m_gameClients
static bool    s_voiceListenHookInstalled = false;         // SetClientListening PRE hook on s_pEngine
static bool    s_voiceListenSeen = false;                  // first engine call observed (sanity-checked)
static bool    s_voiceListenValidated = false;             // Get/Set round-trip passed
static bool    s_voiceListenDegraded = false;              // NAMED degrade: rewrite + ops disabled

// One-shot behavioral validation of the hand-patched Get/SetClientListening vtable slots (the
// ChangeTeam 102-vs-101 drift lesson): flip one (receiver, sender) listen bit both ways and read it
// back through the ADJACENT virtual. Runs from Hook_ClientActive once two clients (bots count) are
// active; retried on every activation until it can run. Skips muted slots so our own pre-hook's
// rewrite can't fake a mismatch. Pass -> proactive-apply enabled; fail -> named degrade.
static void MaybeValidateVoiceListening() {
    if (s_voiceListenValidated || s_voiceListenDegraded || !s_voiceListenHookInstalled || !s_pEngine) return;
    int a = -1, b = -1;
    for (int i = 0; i < kMaxClientSlots; i++) {
        if (!s2_client_valid(i) || s_voiceMuted[i]) continue;
        if (a < 0) a = i; else { b = i; break; }
    }
    if (b < 0) return;   // need two un-muted occupied slots; try again on the next ClientActive
    bool orig     = s_pEngine->GetClientListening(CPlayerSlot(a), CPlayerSlot(b));
    s_pEngine->SetClientListening(CPlayerSlot(a), CPlayerSlot(b), !orig);
    bool flipped  = s_pEngine->GetClientListening(CPlayerSlot(a), CPlayerSlot(b));
    s_pEngine->SetClientListening(CPlayerSlot(a), CPlayerSlot(b), orig);
    bool restored = s_pEngine->GetClientListening(CPlayerSlot(a), CPlayerSlot(b));
    if (flipped == !orig && restored == orig) {
        s_voiceListenValidated = true;
        META_CONPRINTF("[s2script] VOICE VALIDATION: Get/SetClientListening round-trip OK (slots %d,%d)\n", a, b);
    } else {
        s_voiceListenDegraded = true;
        META_CONPRINTF("[s2script] VOICE VALIDATION FAILED: SetClientListening round-trip mismatch "
                       "(orig=%d flipped=%d restored=%d) — hand-patched eiface vtable region drifted; "
                       "voice mute DISABLED (voiceMuted is inert)\n", (int)orig, (int)flipped, (int)restored);
    }
}

// voice_set_muted op. Records the flag, then (mute only, only once the round-trip PROVED the vtable
// slots) proactively forces listen=false for every current receiver so the mute doesn't wait for the
// engine's next voice refresh. Our own PRE hook sees these calls harmlessly (param already false).
// Unmute is engine-paced: the game's next refresh restores its own truth (a laggy unmute is benign).
static int s2_voice_set_muted(int slot, int muted) {
    if (slot < 0 || slot >= kMaxClientSlots) return 0;
    s_voiceMuted[slot] = muted ? 1 : 0;
    if (!s_voiceListenHookInstalled || s_voiceListenDegraded) return 0;   // recorded but inert
    if (muted && s_voiceListenValidated && s_pEngine) {
        for (int r = 0; r < kMaxClientSlots; r++) {
            if (r == slot || !s2_client_valid(r)) continue;
            s_pEngine->SetClientListening(CPlayerSlot(r), CPlayerSlot(slot), false);
        }
    }
    return 1;
}
static int s2_voice_get_muted(int slot) {
    if (slot < 0 || slot >= kMaxClientSlots) return -1;
    return s_voiceMuted[slot] ? 1 : 0;
}
```

*(Note: `<ctime>` is almost certainly already transitively included; add `#include <ctime>` at the top of the file if the build complains about `time_t`/`time`.)*

- [ ] **Step 4: Hook bodies.**

After `Hook_ClientSettingsChanged`'s body (~:3755):

```cpp
// Voice-control: ClientVoice fires per RECEIVED voice packet (tens/sec while a client talks — never
// for bots). Throttle per-slot to <=1 core dispatch per wall-clock second; the first packet of a
// transmission always dispatches, so a lazy mute-on-talk (the TTT PlayerMuter pattern) lands
// immediately. Notify-only (POST, MRES_IGNORED); the core side is the existing try_borrow_mut-guarded
// dispatch_client_event under the name "voice".
void S2ScriptPlugin::Hook_ClientVoice(CPlayerSlot slot) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) {
        time_t now = time(nullptr);
        if (now != s_voiceLastNotify[s]) {
            s_voiceLastNotify[s] = now;
            s2script_core_dispatch_client_event("voice", s);
        }
    }
    RETURN_META(MRES_IGNORED);
}

// Voice-control: the enforcement hook (CSSharp voice_manager.cpp:60-63 shape). PRE hook; when the
// SENDER is muted and the game is about to store listen=true, swap the param to false with
// MRES_IGNORED + NEWPARAMS — the engine's own implementation still runs and stores our value. HOT
// PATH: plain array reads only. First fire performs the arg-sanity half of the doctrine validation
// (out-of-range slots = vtable drift -> named degrade, rewrite disabled) and logs once — that log
// line is also the live evidence for the engine's refresh cadence.
bool S2ScriptPlugin::Hook_SetClientListening(CPlayerSlot receiver, CPlayerSlot sender, bool bListen) {
    int r = receiver.Get(), s = sender.Get();
    if (!s_voiceListenSeen) {
        s_voiceListenSeen = true;
        if (r < -1 || r >= kMaxClientSlots || s < -1 || s >= kMaxClientSlots) {
            s_voiceListenDegraded = true;
            META_CONPRINTF("[s2script] VOICE VALIDATION FAILED: SetClientListening first fire has "
                           "out-of-range slots (r=%d s=%d) — vtable drift; voice mute DISABLED\n", r, s);
        } else {
            META_CONPRINTF("[s2script] voice: SetClientListening first fire (r=%d s=%d listen=%d)\n",
                           r, s, (int)bListen);
        }
    }
    if (!s_voiceListenDegraded && bListen && s >= 0 && s < kMaxClientSlots && s_voiceMuted[s]) {
        RETURN_META_VALUE_NEWPARAMS(MRES_IGNORED, bListen, &IVEngineServer2::SetClientListening,
                                    (receiver, sender, false));
    }
    RETURN_META_VALUE(MRES_IGNORED, bListen);
}
```

- [ ] **Step 5: Install both hooks in `Load()`.**

(a) In the `m_gameClients` success block, after the six `SH_ADD_HOOK`s and the `m_clientLifecycleHooksInstalled = true;` line (~:2723):

```cpp
                // Voice-control slice: the 7th sibling — throttled voice-packet notify.
                SH_ADD_HOOK(ISource2GameClients, ClientVoice, m_gameClients,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientVoice), true);   // POST, like CSSharp
                s_voiceNotifyHookInstalled = true;
                META_CONPRINTF("[s2script] voice: ClientVoice hook installed (throttled notify)\n");
```

(b) In the `s_pEngine` success block, after its `interface OK` META_CONPRINTF (~:2756):

```cpp
                // Voice-control slice: the mute-enforcement rewrite hook. Enforcement stays gated on
                // the runtime validation (first-fire sanity here, Get/Set round-trip at 2nd
                // ClientActive) because the eiface vtable region is hand-patched.
                SH_ADD_HOOK(IVEngineServer2, SetClientListening, s_pEngine,
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_SetClientListening), false);  // PRE
                s_voiceListenHookInstalled = true;
                META_CONPRINTF("[s2script] voice: SetClientListening hook installed (mute enforcement)\n");
```

- [ ] **Step 6: Validation trigger + disconnect hygiene.**

In `Hook_ClientActive` (~:3735), after the `s_trackedSignon[s] = kSignonFull;` line and BEFORE the dispatch:

```cpp
    MaybeValidateVoiceListening();   // one-shot Get/Set round-trip once two clients are active
```

In `Hook_ClientDisconnect` (~:3748), after `s_trackedSignon[s] = kSignonNone;`:

```cpp
    if (s >= 0 && s < kMaxClientSlots) { s_voiceMuted[s] = 0; s_voiceLastNotify[s] = 0; }  // slot-reuse hygiene
```

- [ ] **Step 7: Unload removal.**

In `Unload()`, after the six-lifecycle `SH_REMOVE_HOOK` block (~:3575):

```cpp
    // Voice-control slice: remove both voice hooks. Any forced-false listen values already stored in
    // the engine are restored by the game's own next voice refresh (engine-paced; see live-gate note).
    if (s_voiceNotifyHookInstalled && m_gameClients) {
        SH_REMOVE_HOOK(ISource2GameClients, ClientVoice, m_gameClients,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_ClientVoice), true);
        s_voiceNotifyHookInstalled = false;
    }
    if (s_voiceListenHookInstalled && s_pEngine) {
        SH_REMOVE_HOOK(IVEngineServer2, SetClientListening, s_pEngine,
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_SetClientListening), false);
        s_voiceListenHookInstalled = false;
    }
```

- [ ] **Step 8: Ops wiring.**

After `ops.usercmd_clear_subtick = &s2_usercmd_clear_subtick;` (:3476):

```cpp
    // Voice-control slice — APPENDED after usercmd_clear_subtick; order MUST match S2EngineOps.
    ops.voice_set_muted = &s2_voice_set_muted;
    ops.voice_get_muted = &s2_voice_get_muted;
```

- [ ] **Step 9: Sniper build + gates.**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
# expected: BOTH libs2script_core.so and s2script.so rebuild; GLIBC checks pass; no compile error
make check-boundary && bash scripts/test-boundary-nameleak.sh   # expected: PASS (all voice C++ is shim-side)
```

- [ ] **Step 10: Commit (PR2).**

```bash
git add shim/src/s2script_mm.cpp shim/src/s2script_mm.h
gt create -am "feat(voice): shim SetClientListening rewrite + ClientVoice notify + runtime validation

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
```

---

## Task 3 (PR3): Demo + basecomm migration + live gate

**Files:** create `examples/voice-demo/{package.json,tsconfig.json,src/plugin.ts}`; modify `plugins/basecomm/src/plugin.ts`.

**Interfaces:**
- Consumes: `Clients.onVoice`/`client.voiceMuted` (`@s2script/sdk/clients`), `Commands.register` (`@s2script/sdk/commands`), `Events.on("player_spawn"/"round_end")` (typed: `ev.getPlayerSlot("userid")`), `Player.fromSlot`/`pawn.health` (`@s2script/cs2`).

### Steps

- [ ] **Step 1: Demo plugin (TTT `PlayerMuter`-shaped).**

`examples/voice-demo/package.json`:

```json
{
  "name": "@demo/voice-demo",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }
}
```

`examples/voice-demo/tsconfig.json`: `cp examples/entity-listeners-demo/tsconfig.json examples/voice-demo/tsconfig.json` (any sibling demo's tsconfig is identical).

`examples/voice-demo/src/plugin.ts`:

```ts
// Live-gate demo for the voice-control slice — shaped exactly like TTT's PlayerMuter:
// lazy mute-on-talk for DEAD players (+ one-time reminder), unmute on spawn, unmute-all on round end,
// plus sm_voicetest for the bot-provable tier (flag set/read without any human voice).
import { Clients } from "@s2script/sdk/clients";
import { Commands } from "@s2script/sdk/commands";
import { Events } from "@s2script/sdk/events";
import { Player } from "@s2script/cs2";

Clients.onVoice((c) => {
  console.log("[voice-demo] onVoice slot=" + c.slot + " name=" + c.name + " muted=" + c.voiceMuted);
  const p = Player.fromSlot(c.slot);
  const pawn = p ? p.pawn : null;
  const dead = !pawn || (pawn.health ?? 0) <= 0;
  if (dead && !c.voiceMuted) {                       // TTT PlayerMuter.cs:39-53, lazily on the talk attempt
    c.voiceMuted = true;
    c.chat("[voice-demo] Dead players are muted until you respawn.");
    console.log("[voice-demo] lazy-muted dead talker slot=" + c.slot);
  }
});

Events.on("player_spawn", (ev) => {                  // TTT :57-62 — clear on respawn
  const slot = ev.getPlayerSlot("userid");
  const c = Clients.fromSlot(slot);
  if (c && c.voiceMuted) { c.voiceMuted = false; console.log("[voice-demo] unmuted slot " + slot + " on spawn"); }
});

Events.on("round_end", () => {                       // TTT :66-70 — clear all at round end
  for (const c of Clients.all()) if (c.voiceMuted) c.voiceMuted = false;
  console.log("[voice-demo] round_end — unmuted all");
});

// Bot-provable gate hook: sm_voicetest <slot> <0|1> — set/read the flag without needing voice traffic.
Commands.register("sm_voicetest", (ctx) => {
  const slot = parseInt(ctx.arg(0), 10);
  const on = ctx.arg(1) !== "0";
  const c = Clients.fromSlot(isNaN(slot) ? -1 : slot);
  if (!c) { ctx.reply("[voice-demo] no client in slot '" + ctx.arg(0) + "'"); return; }
  c.voiceMuted = on;
  ctx.reply("[voice-demo] slot " + slot + " (" + c.name + ") voiceMuted=" + c.voiceMuted);
});

export function onLoad(): void {
  console.log("[voice-demo] onLoad — onVoice armed; sm_voicetest registered");
}
export function onUnload(): void {}
```

- [ ] **Step 2: basecomm migration.**

In `plugins/basecomm/src/plugin.ts`:

(a) Replace the header MUTE bullet (:6-10) with:

```
//  - MUTE (voice): REAL. Flips Client.voiceMuted — the shim's SetClientListening rewrite silences the
//    sender's outgoing voice for every receiver (the CSSharp/Swiftly mechanism; supersedes the old
//    best-effort m_bHasCommunicationAbuseMute plan). The schema flag is still written as a cosmetic
//    scoreboard indicator only. Keyed by SteamID and re-asserted on putinserver so a mute survives a
//    reconnect. sm_silence = gag + mute.
```

(b) Add the import next to the other `@s2script/sdk/*` imports:

```ts
import { Clients } from "@s2script/sdk/clients";
```

(c) Replace `setMute` (:38-43) with:

```ts
function setMute(p: Player, on: boolean): void {
  const c = Clients.fromSlot(p.slot);
  if (c) c.voiceMuted = on;                 // REAL server-side voice mute (voice-control slice)
  p.hasCommunicationAbuseMute = on;         // cosmetic scoreboard indicator (best-effort, kept)
  const sid = p.steamId;
  if (!sid) return;
  if (on) muted.add(sid); else muted.delete(sid);
}
```

(d) In `onLoad()`, after the `Chat.onMessage` registration, add the reconnect re-assert:

```ts
  // A muted player who reconnects gets a fresh slot with a cleared flag (shim slot hygiene) — re-assert
  // the SteamID-keyed admin mute once their controller exists.
  Clients.onPutInServer((c) => {
    if (muted.has(c.steamId)) c.voiceMuted = true;
  });
```

- [ ] **Step 3: Build + typecheck gates.**

```bash
( cd packages/cli && node build.mjs )
node packages/cli/dist/cli.js build examples/voice-demo    # expected: dist/*.s2sp, strict typecheck PASS
bash scripts/build-base-plugins.sh                          # expected: basecomm rebuilds clean
bash scripts/check-plugins-typecheck.sh                     # expected: PASS
```

- [ ] **Step 4: Deploy + live gate (Tier 0 — bot-provable, blocks the PR).**

```bash
bash scripts/package-addon.sh    # note: recreate a writable dist/addons/s2script/configs afterwards (admin cache etc.)
cp examples/voice-demo/dist/*.s2sp dist/addons/s2script/plugins/
cd docker && docker compose restart cs2    # NOT --force-recreate; re-run /patch-gameinfo.sh first if CS2 updated
```

Verify, in order:
1. `docker logs s2script-cs2 --tail 300 | grep -i "voice"` →
   `voice: ClientVoice hook installed (throttled notify)` AND `voice: SetClientListening hook installed (mute enforcement)` AND `[voice-demo] onLoad`.
2. `python3 scripts/rcon.py "bot_add"` ×2 → the 2nd activation logs `VOICE VALIDATION: Get/SetClientListening round-trip OK (slots a,b)`. **`VOICE VALIDATION FAILED` → STOP** (spec §8): do not merge enforcement; file the drift finding.
3. `python3 scripts/rcon.py "sm_voicetest 1 1"` → reply `voiceMuted=true`; `sm_voicetest 1 0` → `false`. Record whether/when `voice: SetClientListening first fire` appeared and roughly how often the engine calls it (the recurrence evidence — grep count over a minute if the first-fire line appeared).
4. `docker logs s2script-cs2 | grep -i "RestartCount\|panic\|SIGSEGV"` → `RestartCount=0`, none — proves the round-trip + proactive-apply direct calls (incl. bot-slot receivers) are crash-safe.

**Tier 1 (ONE human, run if available):** connect, hold push-to-talk → `[voice-demo] onVoice slot=…` at ≤1/s; `sm_slay @me` (funcommands) then talk → `lazy-muted dead talker` + the reminder chat once; respawn → `unmuted … on spawn`.

**Tier 2 (TWO humans — audible suppression): DEFER.** Requires `sv_full_alltalk 1`, an audible baseline, then mute → the second human confirms silence → unmute → audible again. Update the `deferred-live-tests` memory entry ("basecomm voice-mute effect") to point at `voiceMuted` + this gate script. State this deferral explicitly in the PR body.

- [ ] **Step 5: Commit + submit the stack.**

```bash
git add examples/voice-demo plugins/basecomm/src/plugin.ts
gt create -am "feat(voice): TTT-shaped demo + basecomm real mute migration; live-gate proof

Claude-Session: https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3"
gt submit --stack
```

PR bodies must include: the reuse-vs-new-fact verdict (hooks, zero sigs), the validation evidence (round-trip log line, first-fire line + observed cadence), the Tier-2 human-audio deferral, the ABI-tail collision note vs the transmit/round-control stacks, and `https://claude.ai/code/session_014QKNSYyxzQfv7t1pcEjjW3`.

---

## Self-Review

**1. Spec coverage.** §2 mechanism (SetClientListening rewrite + shim flags + ClientVoice notify) → Task 2 Steps 1-5. §2 doctrine validation (first-fire + round-trip + named degrade) → Task 2 Steps 3-4, gated live in Task 3 Step 4.2. §2 recurrence hedge (proactive apply, post-validation only) → Task 2 Step 3 (`s2_voice_set_muted`). §3 API (`voiceMuted`, `onVoice`, degrade contract) → Task 1 Steps 3-5. §4 architecture (shim-side state, ops tail, five ABI places, collision note) → Task 1 Steps 1-2 + Global Constraints. §5 boundary → Global Constraints + gate runs in Tasks 1/2. §6 deferred (matrix, flags, vban, proximity) → not built anywhere (verified: no `ListenOverride`/team read in any step). §7 live-gate tiers → Task 3 Step 4. §8 STOP conditions → Task 3 Step 4.2 + spec cross-reference. Slot hygiene → Task 2 Step 6; unload → Step 7; basecomm reconnect re-assert → Task 3 Step 2d.

**2. Placeholder scan.** No TBD/TODO/`<fill in>` except live-gate numbers captured at run time (intentional). Every code step is complete and paste-able; `~:` line anchors each carry a grep or a named neighbor.

**3. Type consistency.** Ops: C `int(*)(int,int)` / `int(*)(int)` ↔ Rust `extern "C" fn(c_int, c_int) -> c_int` / `fn(c_int) -> c_int` ↔ shim `static int s2_voice_set_muted(int,int)` / `(int)`. Consistent. Natives `__s2_voice_set_muted(slot, bool)`/`__s2_voice_get_muted(slot)` ↔ prelude property (`get` maps `=== 1`, `set` passes `!!on`) ↔ `.d.ts` `voiceMuted: boolean`. Event name `"voice"` identical in shim dispatch, prelude `__s2_client_on("voice", h)`, and tests. Handler `(client: Client)` matches the existing `__s2_client_on` wrapper. Test captures mirror the exact op signatures. `s2_client_valid`/`kMaxClientSlots`/`s_trackedSignon` names verified against the current tree (s2script_mm.cpp:624-640). ABI tail (`usercmd_clear_subtick` at s2script_core.h:375 / v8host.rs:359 / wiring :3476 / test structs :11042+:11695) verified against the worktree at 538c5c5.
