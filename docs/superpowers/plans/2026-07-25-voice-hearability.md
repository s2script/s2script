# Voice Hearability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a plugin control who can hear whom, per (receiver, sender) pair, without putting JavaScript on the engine's voice hot path.

**Architecture:** The `SetClientListening` PRE hook already exists and already receives both slots — it currently collapses them to one per-sender mute flag. This slice keeps the receiver dimension. Core owns policy as `owner -> (sender -> u64 receiver-mask)`, AND-merged across owners, and pushes a single merged mask per sender to the shim. The shim's hot path gains two shifts and two tests; no FFI, no JS, no allocation. Everything mirrors the `checktransmit` slice, which solved the identical problem for entity visibility.

**Tech Stack:** Rust (`cargo test -p s2script-core`, in-isolate V8 tests), C++17 (SourceHook PRE hook), TypeScript `.d.ts` stubs, `.test.mjs` for the SDK.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-25-voice-hearability-design.md`. Every requirement there applies.
- **`S2EngineOps` is ABI-ordered.** New op fields **append only**, never insert. The current tail is `engine_call_resolve`, `engine_call_invoke` (`core/src/v8host.rs:463-464`); the C header's matching tail is at `shim/include/s2script_core.h:493-494`. Rust field order and C struct order must stay identical.
- **Hot-path contract (spec §3):** `Hook_SetClientListening` stays plain array reads — no FFI, no JS, no heap, no logging past first-fire. It fires up to O(n²) per voice refresh.
- **Layering (spec §4):** `voiceMuted` is checked FIRST and wins. Hearability is consulted only when the sender is not muted. `plugins/basecomm` must not be modified.
- **Merge is AND** across owners (most-restrictive-wins). No owner can widen another's restriction.
- **Slot cap is 64**, and a receiver mask is a `uint64`. Pin the coupling with a `static_assert`.
- **Degradation:** all ops return `0`/`false` when `s_voiceListenDegraded` is set. Never a silent no-op.
- **Boundary:** no game identifier may enter `core/`. `make check-boundary` stays green.

## File Structure

| File | Responsibility |
|---|---|
| `shim/src/s2script_mm.cpp` | `s_voiceAudible[64]` + `s_voiceHasRule` state, the two exported ops, the hot-path test, the `static_assert` |
| `shim/include/s2script_core.h` | Two appended `s2_voice_audible_*_fn` typedefs + struct fields |
| `core/src/v8host.rs` | Two appended op fields, `VOICE_RULES` + merge/push, three natives, `__s2pkg_voice` prelude, owner-store registration |
| `packages/sdk/voice.d.ts` | `Voice` / `VoiceStats` author contract |
| `packages/sdk/package.json` | `./voice` exports subpath |
| `examples/cookbook/src/recipes/voice.ts` | The consumer `check-examples-coverage.sh` requires |

---

### Task 1: Shim — hearability state, ops, and the hot-path test

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (voice-control block ~line 1350; `Hook_SetClientListening` ~line 4815)
- Modify: `shim/include/s2script_core.h` (typedefs ~line 320; struct tail ~line 494)

**Interfaces:**
- Consumes: the existing `s_voiceMuted[]`, `s_voiceListenDegraded`, `kMaxClientSlots` (= 64).
- Produces, for Task 2 to bind:

```c
typedef int (*s2_voice_audible_set_fn)(int sender, uint64_t mask);
typedef int (*s2_voice_audible_clear_fn)(int sender);
typedef int (*s2_voice_audible_stats_fn)(uint64_t* out);   /* out[3] = {calls, entries, rewrites} */
```

- [ ] **Step 1: Add state next to the existing voice block**

In `shim/src/s2script_mm.cpp`, immediately after `static bool s_voiceListenDegraded = false;`:

```cpp
// Hearability (spec §5): per-SENDER bitmask of receivers allowed to hear them. `s_voiceHasRule`
// distinguishes "audible to nobody" (mask 0 WITH the bit set) from "no rule, leave the engine
// alone" (bit clear) — without it those two collapse and every sender would be silenced.
static_assert(kMaxClientSlots <= 64, "s_voiceAudible packs receivers into a uint64 mask");
static uint64_t s_voiceAudible[kMaxClientSlots] = {0};
static uint64_t s_voiceHasRule = 0;
static uint64_t s_voiceCalls = 0, s_voiceRewrites = 0;   // hot-path counters (plain increments)
```

- [ ] **Step 2: Extend the hot path**

Replace the rewrite branch in `Hook_SetClientListening` (currently the single `s_voiceMuted[s]` test):

```cpp
    s_voiceCalls++;
    if (!s_voiceListenDegraded && bListen && s >= 0 && s < kMaxClientSlots) {
        bool deny = false;
        if (s_voiceMuted[s]) {
            deny = true;                                   // layer 1: moderation mute wins
        } else if ((s_voiceHasRule >> s) & 1) {
            // layer 2: hearability. r < 0 is the engine's broadcast/console pseudo-receiver — no
            // bit to test, so a rule cannot deny it.
            if (r >= 0 && r < kMaxClientSlots && !((s_voiceAudible[s] >> r) & 1)) deny = true;
        }
        if (deny) {
            s_voiceRewrites++;
            RETURN_META_VALUE_NEWPARAMS(MRES_IGNORED, bListen, &IVEngineServer2::SetClientListening,
                                        (receiver, sender, false));
        }
    }
    RETURN_META_VALUE(MRES_IGNORED, bListen);
```

- [ ] **Step 3: Add the three exported ops**

Next to the existing `Shim_VoiceSetMuted`:

```cpp
extern "C" int Shim_VoiceAudibleSet(int sender, uint64_t mask) {
    if (s_voiceListenDegraded) return 0;
    if (sender < 0 || sender >= kMaxClientSlots) return 0;
    s_voiceAudible[sender] = mask;
    s_voiceHasRule |= (1ull << sender);
    return 1;
}
extern "C" int Shim_VoiceAudibleClear(int sender) {
    if (s_voiceListenDegraded) return 0;                   // spec §7: clear collapses absent+degraded
    if (sender < 0 || sender >= kMaxClientSlots) return 0;
    int had = (s_voiceHasRule >> sender) & 1;
    s_voiceAudible[sender] = 0;
    s_voiceHasRule &= ~(1ull << sender);
    return had;
}
extern "C" int Shim_VoiceAudibleStats(uint64_t* out) {
    if (!out) return 0;
    out[0] = s_voiceCalls;
    out[1] = (uint64_t)__builtin_popcountll(s_voiceHasRule);
    out[2] = s_voiceRewrites;
    return 1;
}
```

- [ ] **Step 4: Append the typedefs and struct fields**

In `shim/include/s2script_core.h`, after the `s2_engine_call_invoke_fn` typedef:

```c
/* --- voice hearability slice (APPENDED after engine_call_invoke; order is the ABI) --- */
typedef int (*s2_voice_audible_set_fn)(int sender, uint64_t mask);
typedef int (*s2_voice_audible_clear_fn)(int sender);
typedef int (*s2_voice_audible_stats_fn)(uint64_t* out);
```

and at the **end** of the ops struct, after `engine_call_invoke`:

```c
    s2_voice_audible_set_fn   voice_audible_set;
    s2_voice_audible_clear_fn voice_audible_clear;
    s2_voice_audible_stats_fn voice_audible_stats;
```

- [ ] **Step 5: Populate them at the matching end of the initializer**

Find where `engine_call_invoke` is assigned in `s2script_mm.cpp` and add, immediately after:

```cpp
    ops.voice_audible_set   = &Shim_VoiceAudibleSet;
    ops.voice_audible_clear = &Shim_VoiceAudibleClear;
    ops.voice_audible_stats = &Shim_VoiceAudibleStats;
```

- [ ] **Step 6: Build**

Run: `make shim`
Expected: links clean. Only the pre-existing `utlbuffer.h -Wint-to-pointer-cast` warning.

- [ ] **Step 7: Commit**

```bash
git add shim/src/s2script_mm.cpp shim/include/s2script_core.h
git commit -m "Add shim hearability state, ops, and the hot-path test"
```

---

### Task 2: Core — op bindings, the AND-merged rule store, and natives

**Files:**
- Modify: `core/src/v8host.rs`
- Test: in-file `#[cfg(test)]` in `core/src/v8host.rs`

**Interfaces:**
- Consumes: Task 1's three C ops; `crate::owner_stores::register`; `current_plugin(scope)`.
- Produces natives `__s2_voice_audible_set(sender, receiversArray) -> bool`, `__s2_voice_audible_clear(sender) -> bool`, `__s2_voice_audible_stats() -> {calls, entries, rewrites} | null`.

**ABI:** append the three fields at the very END of `S2EngineOps`, after `engine_call_invoke` (line ~464). Do not insert. Both test op-structs in this file must gain the fields too, or they will not compile.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)]` module in `core/src/v8host.rs`:

```rust
#[test]
fn voice_rules_and_merge_across_owners() {
    // Two owners restricting the same sender -> the shim sees the INTERSECTION.
    voice_rules_clear_for_test();
    voice_set_rule_for_test("@a/one", 3, 0b0111);
    voice_set_rule_for_test("@b/two", 3, 0b0110);
    assert_eq!(voice_merged_for_test(3), Some(0b0110));
}

#[test]
fn voice_owner_teardown_recomputes() {
    voice_rules_clear_for_test();
    voice_set_rule_for_test("@a/one", 3, 0b0111);
    voice_set_rule_for_test("@b/two", 3, 0b0110);
    voice_remove_owner("@b/two");
    assert_eq!(voice_merged_for_test(3), Some(0b0111), "the survivor's rule stands alone");
    voice_remove_owner("@a/one");
    assert_eq!(voice_merged_for_test(3), None, "no owners -> no rule at all");
}

#[test]
fn voice_empty_receiver_list_is_a_rule_not_an_absence() {
    // mask 0 WITH a rule = audible to nobody. Distinct from None = engine decides.
    voice_rules_clear_for_test();
    voice_set_rule_for_test("@a/one", 5, 0);
    assert_eq!(voice_merged_for_test(5), Some(0));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p s2script-core voice_`
Expected: FAIL — `voice_set_rule_for_test` / `voice_merged_for_test` / `voice_remove_owner` not found.

- [ ] **Step 3: Add the op types and fields**

Near the other `type ...Fn` aliases:

```rust
// --- voice hearability slice (APPENDED after engine_call_invoke; order is the ABI) ---
type VoiceAudibleSetFn   = extern "C" fn(c_int, u64) -> c_int;
type VoiceAudibleClearFn = extern "C" fn(c_int) -> c_int;
type VoiceAudibleStatsFn = extern "C" fn(*mut u64) -> c_int;
```

At the **end** of `S2EngineOps`:

```rust
    // --- voice hearability slice (APPENDED after engine_call_invoke; do not reorder above) ---
    pub voice_audible_set:   Option<VoiceAudibleSetFn>,
    pub voice_audible_clear: Option<VoiceAudibleClearFn>,
    pub voice_audible_stats: Option<VoiceAudibleStatsFn>,
```

Add `voice_audible_set: None, voice_audible_clear: None, voice_audible_stats: None,` to every test op-struct literal in this file (search for `entity_subobj_vcall: None` to find them).

- [ ] **Step 4: Add the rule store, merge, and helpers**

Modelled on `TRANSMIT_RULES` / `transmit_recompute_and_push`:

```rust
/// voice hearability: per-plugin rules. owner -> (senderSlot -> receiver mask).
/// The shim holds only the AND-merged mask per sender; this map is the policy source of truth so
/// unload/reset can recompute. AND means no owner can WIDEN what another restricted.
thread_local! {
    static VOICE_RULES: std::cell::RefCell<std::collections::HashMap<String, std::collections::HashMap<i32, u64>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// AND-merge every owner's rule for `sender`. None when no owner has one.
fn voice_merged(sender: i32) -> Option<u64> {
    VOICE_RULES.with(|r| {
        let map = r.borrow();
        let mut acc: Option<u64> = None;
        for rules in map.values() {
            if let Some(m) = rules.get(&sender) {
                acc = Some(match acc { None => *m, Some(a) => a & *m });
            }
        }
        acc
    })
}

/// Recompute and push to the shim: set the merged mask, or clear when no rule remains.
fn voice_recompute_and_push(sender: i32) {
    let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
    match voice_merged(sender) {
        Some(mask) => { if let Some(f) = ops.voice_audible_set { f(sender, mask); } }
        None => { if let Some(f) = ops.voice_audible_clear { f(sender); } }
    }
}

/// Drop every rule an owner holds and re-push each sender it touched.
fn voice_remove_owner(owner: &str) {
    let touched: Vec<i32> = VOICE_RULES.with(|r| {
        let mut map = r.borrow_mut();
        match map.remove(owner) { Some(rules) => rules.keys().copied().collect(), None => Vec::new() }
    });
    for s in touched { voice_recompute_and_push(s); }
}

#[cfg(test)]
fn voice_rules_clear_for_test() { VOICE_RULES.with(|r| r.borrow_mut().clear()); }
#[cfg(test)]
fn voice_set_rule_for_test(owner: &str, sender: i32, mask: u64) {
    VOICE_RULES.with(|r| { r.borrow_mut().entry(owner.to_string()).or_default().insert(sender, mask); });
}
#[cfg(test)]
fn voice_merged_for_test(sender: i32) -> Option<u64> { voice_merged(sender) }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p s2script-core voice_`
Expected: PASS (3 tests).

- [ ] **Step 6: Add the three natives**

Follow the `s2_transmit_set` style exactly — `catch_unwind`, `rv` defaulted first:

```rust
/// Native `__s2_voice_audible_set(sender, receiversArray) -> boolean`.
fn s2_voice_audible_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let sender = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if !(0..64).contains(&sender) { return; }
        let Ok(arr) = v8::Local::<v8::Array>::try_from(args.get(1)) else { return };
        let mut mask: u64 = 0;
        for i in 0..arr.length() {
            let Some(v) = arr.get_index(scope, i) else { return };
            let slot = v.integer_value(scope).unwrap_or(-1);
            if !(0..64).contains(&slot) { return; }
            mask |= 1u64 << (slot as u32);
        }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        // Candidate merged mask: AND this rule with every OTHER owner's rule for the same sender.
        let merged = VOICE_RULES.with(|r| {
            let map = r.borrow();
            let mut acc = mask;
            for (o, rules) in map.iter() {
                if o == &owner { continue; }
                if let Some(m) = rules.get(&sender) { acc &= *m; }
            }
            acc
        });
        // PUSH FIRST, PERSIST ONLY ON SUCCESS — the s2_transmit_set ordering. Inserting before the
        // push would leave core holding a rule the shim rejected (e.g. voice degraded), so the two
        // would disagree and a later unrelated recompute would silently apply a phantom rule.
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_audible_set else { return };
        if f(sender, merged) == 0 { return; }
        VOICE_RULES.with(|r| { r.borrow_mut().entry(owner).or_default().insert(sender, mask); });
        rv.set_bool(true);
    }));
}

/// Native `__s2_voice_reset_all()` — remove all of the calling plugin's rules.
/// A dedicated native, not a 64-iteration loop in JS: mirrors `__s2_transmit_reset_all`, and
/// `voice_remove_owner` already recomputes exactly the senders that were touched.
fn s2_voice_reset_all(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        voice_remove_owner(&owner);
    }));
}

/// Native `__s2_voice_audible_clear(sender) -> boolean` — drops only the CALLER's rule.
fn s2_voice_audible_clear(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        let sender = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        if !(0..64).contains(&sender) { return; }
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let removed = VOICE_RULES.with(|r| {
            let mut map = r.borrow_mut();
            map.get_mut(&owner).map(|rules| rules.remove(&sender).is_some()).unwrap_or(false)
        });
        if removed { voice_recompute_and_push(sender); }
        rv.set_bool(removed);
    }));
}

/// Native `__s2_voice_audible_stats() -> {calls, entries, rewrites} | null`.
/// Null when the op is unassigned (old shim) — the capability is ABSENT, not zero.
fn s2_voice_audible_stats(scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(f) = ops.voice_audible_stats else { return };
        let mut out = [0u64; 3];
        if f(out.as_mut_ptr()) == 0 { return; }
        let obj = v8::Object::new(scope);
        for (k, v) in [("calls", out[0]), ("entries", out[1]), ("rewrites", out[2])] {
            let key = v8::String::new(scope, k).unwrap();
            let val = v8::Number::new(scope, v as f64);
            obj.set(scope, key.into(), val.into());
        }
        rv.set(obj.into());
    }));
}
```

Register all three in the `set_native` block, next to the transmit natives.

- [ ] **Step 7: Register the owner store**

Next to the `"TRANSMIT"` registration:

```rust
// voice hearability: drop the plugin's rules + re-push each affected sender. Not a scope surface.
crate::owner_stores::register(
    "VOICE",
    Box::new(|owner| { voice_remove_owner(owner); }),
    Box::new(|_ids| {}),
);
```

- [ ] **Step 8: Add the prelude**

Next to `globalThis.__s2pkg_transmit`:

```js
  var Voice = {
    setAudibleTo: function (sender, receivers) {
      if (!Array.isArray(receivers)) throw new TypeError("Voice.setAudibleTo: receivers must be an array");
      return __s2_voice_audible_set(sender, receivers);
    },
    reset: function (sender) { return __s2_voice_audible_clear(sender); },
    resetAll: function () { __s2_voice_reset_all(); },
    stats: function () { return __s2_voice_audible_stats(); },
  };
  globalThis.__s2pkg_voice = { Voice: Voice };
```

- [ ] **Step 9: Verify**

Run: `cargo test -p s2script-core` then `make core && make shim` then `make check-boundary`
Expected: all pass; boundary green (no game identifiers added to core).

- [ ] **Step 10: Commit**

```bash
git add core/src/v8host.rs
git commit -m "Add the core voice rule store, natives, and owner teardown"
```

---

### Task 3: SDK contract + cookbook recipe

**Files:**
- Create: `packages/sdk/voice.d.ts`
- Modify: `packages/sdk/package.json` (add `"./voice"` to `exports`)
- Create: `examples/cookbook/src/recipes/voice.ts`
- Modify: `examples/cookbook/src/recipes/index.ts`
- Test: `packages/sdk/test/voice-exports.test.mjs`

**Interfaces:**
- Consumes: Task 2's `__s2pkg_voice`.
- Produces: the `@s2script/sdk/voice` subpath.

**Why the recipe is in this task:** `scripts/check-examples-coverage.sh` fails the build when a shipped `packages/sdk/*.d.ts` has no consumer in `examples/`, `plugins/`, or `tools/`. Adding the subpath without the recipe turns `make ci-js` red.

- [ ] **Step 1: Write the failing test**

```js
// packages/sdk/test/voice-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./voice", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./voice"], "expected a ./voice export subpath");
});

test("voice.d.ts declares Voice and VoiceStats", () => {
  const dts = readFileSync(join(root, "voice.d.ts"), "utf8");
  assert.match(dts, /export interface VoiceStats/);
  assert.match(dts, /export declare const Voice/);
  assert.match(dts, /setAudibleTo\(/);
  assert.match(dts, /rewrites/);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/voice-exports.test.mjs`
Expected: FAIL — `voice.d.ts` does not exist.

- [ ] **Step 3: Write the contract**

```ts
// packages/sdk/voice.d.ts
/**
 * @s2script/sdk/voice — who can hear whom. NO runtime code: the engine injects the implementation
 * at load (`__s2pkg_voice`).
 *
 * Layered UNDER `Client.voiceMuted`: a muted sender is inaudible to everyone regardless of any rule
 * here, so admin moderation always beats a gameplay rule.
 *
 * Rules are declarative state, not a callback. The engine's listen matrix is re-asserted
 * continuously and the underlying hook fires per (receiver, sender) pair, so a per-pair JS callback
 * would run up to 64x64 times per refresh. Set a rule; the shim enforces it.
 */

export interface VoiceStats {
  /** Listen-matrix decisions the engine has asked about. */
  calls: number;
  /** Senders currently carrying a merged rule. */
  entries: number;
  /** Times a rule actually silenced a pair. THE effect counter — a rule that never rewrites is not working. */
  rewrites: number;
}

export declare const Voice: {
  /**
   * This sender is audible ONLY to `receivers` (0-based slots). An empty array means audible to
   * nobody — distinct from {@link Voice.reset}, which removes the rule and lets the engine decide.
   * Rules from multiple plugins AND-merge, so another plugin can only ever narrow yours, never widen it.
   * Returns false when voice control is degraded on this build.
   */
  setAudibleTo(sender: number, receivers: readonly number[]): boolean;
  /** Drop THIS plugin's rule for `sender`. False when no rule of yours was present. */
  reset(sender: number): boolean;
  /** Drop every rule this plugin owns. Unload does this automatically. */
  resetAll(): void;
  /** Hot-path counters, or null when the running shim predates this capability. */
  stats(): VoiceStats | null;
};
```

- [ ] **Step 4: Add the export subpath**

In `packages/sdk/package.json`, alphabetically between `"./usermessages"` and `"./votes"`, mirroring the exact shape of its siblings:

```json
    "./voice": { "types": "./voice.d.ts" },
```

- [ ] **Step 5: Write the cookbook recipe**

```ts
// examples/cookbook/src/recipes/voice.ts
import type { Recipe } from "../recipe.ts";
import { Voice } from "@s2script/sdk/voice";
import { Player } from "@s2script/cs2";

/**
 * Voice hearability — who can hear whom, per (receiver, sender) pair.
 *
 * The rule is declarative: you set it, the shim enforces it on the engine's listen matrix. There is
 * deliberately no per-pair callback, because that path runs up to 64x64 times per voice refresh.
 *
 *   cb_voice_solo <slot>   only that slot is audible; everyone else is silenced
 *   cb_voice_reset         drop this plugin's rules
 *   cb_voice_stats         show the hot-path counters (rewrites = the effect)
 */
export const voiceRecipe: Recipe = {
  name: "voice",
  describe: "per-pair voice hearability (cb_voice_solo / _reset / _stats)",
  register(ctx) {
    ctx.commands.register("cb_voice_solo", (cmd) => {
      const keep = cmd.argInt(0, -1);
      if (keep < 0) { cmd.reply("[cookbook] usage: cb_voice_solo <slot>"); return; }
      // Everyone except `keep` becomes audible to nobody. `keep` gets its rule dropped so the
      // engine decides for them again.
      let silenced = 0;
      for (const p of Player.allConnected()) {
        if (p.slot === keep) { Voice.reset(p.slot); continue; }
        if (Voice.setAudibleTo(p.slot, [])) silenced++;
      }
      cmd.reply(`[cookbook] voice: silenced ${silenced} sender(s); slot ${keep} still audible`);
    });

    ctx.commands.register("cb_voice_reset", (cmd) => {
      Voice.resetAll();
      cmd.reply("[cookbook] voice: dropped this plugin's hearability rules");
    });

    ctx.commands.register("cb_voice_stats", (cmd) => {
      const s = Voice.stats();
      if (!s) { cmd.reply("[cookbook] voice: stats unavailable (shim predates this capability)"); return; }
      cmd.reply(
        `[cookbook] voice: calls=${s.calls} entries=${s.entries} rewrites=${s.rewrites}` +
          (s.rewrites > 0 ? " (rules ARE taking effect)" : " (no rewrites yet — nobody has spoken)")
      );
    });
  },
};
```

- [ ] **Step 6: Wire the recipe in**

In `examples/cookbook/src/recipes/index.ts`, add the import alphabetically and the entry to `RECIPES`:

```ts
import { voiceRecipe } from './voice.ts';
```
```ts
  voiceRecipe,
```

- [ ] **Step 7: Verify**

Run:
```bash
cd packages/sdk && node --experimental-strip-types --no-warnings --test test/voice-exports.test.mjs
cd ../.. && bash scripts/check-plugins-typecheck.sh
bash scripts/check-examples-coverage.sh
```
Expected: 2 tests pass; typecheck passes; coverage reports one MORE module than before and PASSES.

- [ ] **Step 8: Commit**

```bash
git add packages/sdk/voice.d.ts packages/sdk/package.json packages/sdk/test/voice-exports.test.mjs \
        examples/cookbook/src/recipes/voice.ts examples/cookbook/src/recipes/index.ts
git commit -m "Add the @s2script/sdk/voice contract and cookbook recipe"
```

---

### Task 4: In-isolate native tests + changeset

**Files:**
- Modify: `core/src/v8host.rs` (in-isolate `#[cfg(test)]`)
- Create: `.changeset/voice-hearability.md`

**Interfaces:**
- Consumes: Tasks 1–3.

**Pattern:** `transmit_set_folds_viewer_slots_into_mask` (`core/src/v8host.rs`, ~line 14259) is the
template. The real helpers are `init(dummy_logger())`, `set_engine_ops(Some(...))`,
`create_plugin_context(name)`, and `eval_in_context_string(name, js)`, with a recording `static
Mutex<Vec<...>>` and a `*_test_ops()` builder. Read that test before writing these and mirror it.

- [ ] **Step 1: Write the failing tests**

```rust
static VOICE_SET_CALLS: std::sync::Mutex<Vec<(i32, u64)>> = std::sync::Mutex::new(Vec::new());

extern "C" fn voice_fake_set(sender: c_int, mask: u64) -> c_int {
    VOICE_SET_CALLS.lock().unwrap().push((sender, mask));
    1
}

/// Ops with ONLY voice_audible_set wired — everything else stays None, which is also what proves
/// the stats native reports ABSENT rather than zero.
fn voice_test_ops() -> S2EngineOps {
    let mut ops = empty_test_ops();
    ops.voice_audible_set = Some(voice_fake_set);
    ops
}

/// setAudibleTo folds the receiver-slot array into a u64 mask and pushes (sender, mask).
#[test]
fn voice_set_audible_to_folds_receiver_slots_into_mask() {
    let _ = init(dummy_logger());
    VOICE_SET_CALLS.lock().unwrap().clear();
    voice_rules_clear_for_test();
    set_engine_ops(Some(voice_test_ops()));
    create_plugin_context("vc1");
    let out = eval_in_context_string("vc1",
        "String(__s2pkg_voice.Voice.setAudibleTo(3, [0, 5, 63]))");
    assert_eq!(out, "true");
    let calls = VOICE_SET_CALLS.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], (3, 1u64 | (1u64 << 5) | (1u64 << 63)));
}

/// An old shim must report ABSENT, not zero — zero would read as "working, nothing happened".
#[test]
fn voice_stats_is_null_without_the_op() {
    let _ = init(dummy_logger());
    set_engine_ops(Some(empty_test_ops()));      // voice_audible_stats stays None
    create_plugin_context("vc2");
    let out = eval_in_context_string("vc2", "String(__s2pkg_voice.Voice.stats())");
    assert_eq!(out, "null");
}

/// The JS wrapper rejects a non-array before it can reach the native.
#[test]
fn voice_set_audible_to_rejects_a_non_array() {
    let _ = init(dummy_logger());
    set_engine_ops(Some(voice_test_ops()));
    create_plugin_context("vc3");
    let out = eval_in_context_string("vc3",
        "(function(){ try { __s2pkg_voice.Voice.setAudibleTo(0, 5); return 'no-throw'; } \
          catch (e) { return e instanceof TypeError ? 'TypeError' : 'other'; } })()");
    assert_eq!(out, "TypeError");
}
```

`empty_test_ops()` is whatever the file's all-`None` ops constructor is actually called — the transmit
test builds its ops via a local `transmit_test_ops()`; find the equivalent base and reuse it rather
than hand-writing a struct literal, so appending future ops does not break these tests.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p s2script-core voice_`
Expected: FAIL — the prelude/ops are exercised for the first time.

- [ ] **Step 3: Fix whatever the tests surface**

No new production code should be needed; these tests exercise Tasks 1–2. If one fails, the bug is real — fix it in the source, not the test.

- [ ] **Step 4: Run the full suite**

Run: `cargo test -p s2script-core`
Expected: all pass, 0 failed.

- [ ] **Step 5: Add the changeset**

`packages/sdk` gains a public subpath, and a push to `main` auto-publishes:

```markdown
---
'@s2script/sdk': minor
---

Add `@s2script/sdk/voice` — per-(receiver, sender) voice hearability.

`Voice.setAudibleTo(sender, receivers)` restricts who can hear a speaker; rules from multiple plugins
AND-merge so one plugin can only narrow another's, never widen it. Layered under `Client.voiceMuted`,
which still wins, so admin moderation always beats a gameplay rule.

Declarative rather than a callback: the engine's listen matrix is re-asserted continuously and the
underlying hook fires per pair, so a per-pair JS callback would run up to 64x64 times per refresh.
`Voice.stats().rewrites` is the effect counter — a rule that never rewrites is not taking effect.
```

- [ ] **Step 6: Commit**

```bash
git add core/src/v8host.rs .changeset/voice-hearability.md
git commit -m "Add in-isolate voice tests and the changeset"
```

---

### Task 5: Live gate

**Files:**
- Modify: `docs/PROGRESS.md` (append the finished-slice entry)

**Interfaces:**
- Consumes: Tasks 1–4.

**Prerequisites:** the sniper build is the ONLY deployable binary (host glibc is too new). The Docker container mounts the MAIN checkout's `dist/addons/s2script`, so build here and copy `bin/` there — leave `configs/` and `data/` alone, they hold live operator state. Back up first.

- [ ] **Step 1: Build the deployable binaries**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: `DONE`, with core GLIBC ≤ 2.31.

- [ ] **Step 2: Build the cookbook and deploy**

```bash
cd examples/cookbook && npx @s2script/sdk build && cd ../..
LIVE=/home/gkh/projects/s2script/dist/addons/s2script
tar czf /tmp/dist-backup-voice.tgz -C /home/gkh/projects/s2script dist/addons/s2script
cp dist/addons/s2script/bin/linuxsteamrt64/*.so "$LIVE/bin/linuxsteamrt64/"
cp dist/addons/s2script/js/pawn.js "$LIVE/js/"
cp examples/cookbook/dist/_example_cookbook.s2sp "$LIVE/plugins/"
```

- [ ] **Step 3: Restart (stop+start, not restart)**

```bash
cd /home/gkh/projects/s2script
docker compose -f docker/docker-compose.yml stop cs2
docker compose -f docker/docker-compose.yml start cs2
sleep 45
```
`docker restart` can keep the stale `.so`. Confirm the new build is live by asserting a log line only this build emits, not by md5.

- [ ] **Step 4: Gate it**

```bash
python3 scripts/rcon.py "sm_voice_stats"              # baseline: calls climbing, entries 0
python3 scripts/rcon.py "sm_voice_only 0 1"           # slot 0 audible ONLY to slot 1
python3 scripts/rcon.py "sm_voice_stats"              # entries MUST be 1
python3 scripts/rcon.py "sm_voice_reset"
python3 scripts/rcon.py "sm_voice_stats"              # entries back to 0
python3 scripts/rcon.py "sm_voice_solo 63"            # empty mask on every connected sender
python3 scripts/rcon.py "sm_voice_stats"              # entries == connected player count
touch <live>/plugins/_example_cookbook.s2sp           # hot-reload -> owner-store teardown
sleep 12
python3 scripts/rcon.py "sm_voice_stats"              # entries MUST be 0 again
```

**`entries` is the gate, NOT `rewrites`.** The hot path only considers denying when the engine passes
`bListen == true`, and it only does that when a client is actually TRANSMITTING voice. Bots never
transmit, so `rewrites` stays at 0 no matter how many rules are in force — measured: 12 senders with
empty masks and ~160k hook `calls` produced exactly 0 rewrites. Do not treat that as a failure, and do
not try to force it with `sv_alltalk`; it does not help.

What `entries` does prove: the rule crossed core→shim, the AND-merge ran, the has-rule bit was set,
and — on the hot-reload — the `"VOICE"` owner-store teardown reached the shim. That is spec criterion
4, live.

**Not provable on this server:** disconnect hygiene. It needs a client to actually leave, and neither
`bot_kick` nor `bot_quota 0` removes bots on this configuration. It is covered by
`voice_disconnect_clears_the_slot_across_owners` in the core suite.

**Deferred to a two-human session:** the denial itself (`rewrites > 0`) and audible silence — the same
Tier-2 deferral the voice-control slice carries for mute.

- [ ] **Step 5: Restore the server**

```bash
rm -rf /home/gkh/projects/s2script/dist/addons/s2script
tar xzf /tmp/dist-backup-voice.tgz -C /home/gkh/projects/s2script
cd /home/gkh/projects/s2script
docker compose -f docker/docker-compose.yml stop cs2 && docker compose -f docker/docker-compose.yml start cs2
```

- [ ] **Step 6: Full gate + PROGRESS**

Run: `make ci` and `CI=1 make ci-js`
Expected: green.

Append a finished-slice entry to `docs/PROGRESS.md` in the house style: what shipped, the hot-path
contract, the AND-merge rationale, and the live-gate result including the `rewrites` numbers.

- [ ] **Step 7: Commit and open the PR**

```bash
git add docs/PROGRESS.md
git commit -m "Log the voice hearability slice"
```

Write the PR body to a file and use `gh pr edit N --body-file` — never a heredoc, it mangles tables.
The body must include **Why**.

---

## Self-Review

**Spec coverage:** §3 hot-path contract → Task 1 Step 2. §4 layering → Task 1 Step 2 (mute checked
first). §5 data model → Task 1 Step 1 (`s_voiceHasRule`, `static_assert`) + Task 2 Step 4
(`VOICE_RULES`). §6 hot path → Task 1 Step 2. §7 ops + return convention → Task 1 Steps 3–5, Task 2
Step 3. §8 degradation → Task 1 Step 3 (`s_voiceListenDegraded` guard) + Task 4 Step 1 (null stats).
§9 API → Task 3. §10 teardown → Task 2 Step 7. §11 testing → Tasks 2, 3, 4, 5. §13 criteria 1–7 →
Task 5 covers 1/2/7, Task 2 covers 3/4, Task 1+4 cover 5, Task 1 covers 6.

**Placeholder scan:** clean — no TBD/TODO, every code step carries real code.

**Type consistency:** the three C typedefs in Task 1 Step 4 match the Rust aliases in Task 2 Step 3
field-for-field (`int/uint64_t` ↔ `c_int/u64`, `uint64_t*` ↔ `*mut u64`). `voice_merged` returns
`Option<u64>` in both its definition and its test helper. `VoiceStats` keys (`calls`, `entries`,
`rewrites`) are identical across the native (Task 2 Step 6), the `.d.ts` (Task 3 Step 3), and the
recipe (Task 3 Step 5). `stats()` is `VoiceStats | null` in the `.d.ts` and the recipe null-checks it.

**Fixed during review** (three findings, each verified against the codebase rather than inferred):

1. **Push-then-persist ordering.** The plan originally inserted into `VOICE_RULES` and *then* pushed
   to the shim. `s2_transmit_set` does the opposite — `if f(...) == 0 { return; }` guards the insert.
   The original order would leave core holding a rule the shim rejected (voice degraded, slot out of
   range), so the two would silently disagree and a later recompute could apply a phantom rule.
   Task 2 Step 6 now matches the precedent.
2. **`resetAll` looped 64 native calls.** There is a dedicated `__s2_transmit_reset_all` native that
   delegates to `transmit_remove_owner`; the plan now adds the symmetric `__s2_voice_reset_all`, which
   also recomputes only the senders actually touched.
3. **The in-isolate test helper was invented.** `eval_in_isolate_with_ops` does not exist. The real
   pattern is `init(dummy_logger())` / `set_engine_ops` / `create_plugin_context` /
   `eval_in_context_string` with a recording `static Mutex`. Task 4 now uses it.

**Remaining unknown, flagged not guessed:** `empty_test_ops()` is a placeholder for the file's
all-`None` ops constructor. The transmit test builds ops via a local `transmit_test_ops()`; the
implementer must find the equivalent base rather than hand-writing a struct literal, since a literal
breaks every time an op is appended.
