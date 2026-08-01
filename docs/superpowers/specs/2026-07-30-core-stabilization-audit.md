# Core stabilization audit — why core grows per-feature, and the smallest set of changes that stops it

**Date:** 2026-07-30
**Status:** audit + proposed program (A1–A7). Not a slice spec — each item below becomes its own
slice spec when it is picked up.

## Provenance and how much to trust it

Produced by a 12-agent audit: one orchestrator pass (framing + model assignment), four parallel
investigation dimensions, one adversarial reviewer per dimension, one orchestrator synthesis.

Two caveats that matter when reading the findings:

- **The adversarial stage under-performed.** All 22 findings were marked "survives." An adversarial
  pass that kills nothing has not done its job. The synthesis pass then overrode four of them on its
  own (§3), which is the only reason the kill list below is non-empty. Treat "survived adversarial
  review" as weak evidence here; treat §3 and §4's ordering as the real filter.
- **Line numbers are as of the tree the audit ran against**, which was ~20 commits behind
  `origin/main` at `517b71b`. The citations driving A1–A3 were re-verified against `517b71b` and are
  corrected inline below; the rest may be off by tens of lines. The file/symbol names are reliable;
  treat the numbers as hints.

Claims re-verified by hand before this doc was written: the v8host.rs line count, the
`INJECTED_STD_PRELUDE` span, the `Mirrors … verbatim` comments, the engine-call `pid` handling, the
`owner_stores` struct, and `git show --stat 5ff62b4`/`6fe8647` for the zero-core-lines movement slice.

---

## 1. Was the thesis right?

The thesis under test: *"every time I try to implement a new feature I'm having to add something to
core to handle it."*

**Yes — and it is now quantified, with an existence proof.**

- `core/src/v8host.rs`: 6,550 → 19,028 lines in 24 days (+190%). Of those 19,028: **1,805 lines are
  JavaScript inside a Rust string literal** (`INJECTED_STD_PRELUDE`), ~7,082 are the test module,
  ~10,142 are actual Rust.
- Roughly **60% of feature-slice core growth since 2026-07-05 is a copy of an existing pattern** —
  a new mux + a new dispatch fn + new subscribe natives + an ops append + a reset line — not a new
  engine mechanism.
- The per-hook cost today is **7–9 coordinated edit sites in core alone**, verified against
  `git show 625878b` (`Server.onCvarChange`): thread_local, prelude JS, dispatch fn,
  subscribe/unsubscribe natives, `install_natives`, shutdown reset, `owner_stores` — plus `ffi.rs`
  and two shim files.
- **The existence proof that a stable core is reachable is already in the repo:** movement-control
  (`5ff62b4` + `6fe8647`) shipped 14 writable movement fields with **zero `core/src` lines** —
  curated `writable` data in `games/cs2/nav-targets.json` riding the existing `write*Via` chain, plus
  gamedata and shim only. That slice is the model.

One refinement to the thesis: the growth is not primarily "missing primitives in the abstract." The
repo *keeps inventing* the right primitive — `multiplexer.rs` → `event_mux.rs` → `owner_stores.rs` →
`gamedata_calls.rs` → `liveness.rs` — **one slice after it was needed, and never retrofits.** Every
primitive in that list was extracted after 2+ hand-copies existed, and the old copies stayed. The fix
is one deliberate retrofit pass, not new invention.

---

## 2. The cross-dimension headline

Three dimensions independently converged on the same object from different directions:

| Dimension | What it saw |
|---|---|
| why-core-changed | 8 edit sites per hook; ~1,100 retroactive lines of copied mux/dispatch |
| dispatch-mux-unification | 18 dispatchers, ~1,138 lines, 3 actual axes of variation |
| missing-primitives | 17 dispatchers + 19 subscribe natives; "hooks remain core-owned" deferred in the plugin-gamedata spec §14 |

> **Core has a declarative OUTBOUND path (`Engine.call` over gamedata) but no declarative INBOUND
> path — every engine→JS notification is still a hand-rolled vertical slice — and the JS half of the
> API surface is compiled into the Rust.**

Everything else in this audit is a corollary. The single primitive that collapses the most findings
is a generic fan-out dispatcher over a keyed `Descriptor` store with self-registered lifecycle:
dispatch dedup, auto-disable, crash-breadcrumb coverage, priority plumbing, and shutdown reset all
become structural instead of per-slice checklist items.

---

## 3. Kills, downgrades, and re-scopes

The adversarial pass marked everything "survives." Four are overridden here.

**RE-SCOPED — "declared hook-points, the inbound `Engine.call`": the gamedata-declared-detour version
is mostly dead.** Only ~4 of the 14 shim hooks are `s2detour::Install` inline detours reachable by a
generic max-arity handler. The rest are `SH_DECL_HOOK*` SourceHook declarations requiring complete
compile-time C++ types (`CBitVec<16384>&`, `const CNetMessage*` — `s2script_mm.cpp:86-152`) and
providing cooperative chaining with other Metamod extensions, which a data-driven detour would
destroy. Three of the four slices the finding claimed it would absorb (checktransmit,
usermessage-hook, cvar-change) are not expressible in it.
**The correct split:** shim-side hook *acquisition* stays hand-written C++ (genuinely irreducible,
~30–50 lines per hook); everything from the ffi boundary inward becomes one generic table row plus
one generic `s2script_core_dispatch_hook(id, key, argc, argv)` entry point. ~90% of the win at ~10%
of the risk, without touching SourceHook. Plugin-gamedata-declared `hooks` can come later, scoped to
inline-detour targets only, and is **not** on the critical path.

**KILLED — routing the std prelude through `register_injected_package`.** The prelude defines ambient
globals every plugin needs unconditionally (`HookResult`, `Priority`); a deploy-time file load turns
one corrupt file into total plugin failure, where `@s2script/cs2` degrades to `null` by design.
Endorse **only** the `include_str!` split (`core/js/*.js`, compile-time, byte-identical, precedent
`9ed1db0`). Same win, zero new failure modes. Severity correction: the in-isolate tests already
smoke-eval the prelude on every `make ci-native`, so the gap is lint/type coverage, not "never tested."

**KILLED — the `HashMap<&str,(ptr,sig)>` replacement for `S2EngineOps`.** Type erasure plus unsafe
casts at every call site plus a string lookup on hot paths (`schema_offset` is called per property
access) is the wrong trade. Endorse instead: **generate** the positional struct, the C header, and
the two exhaustive test fixtures from one declarative list. The append-only positional ABI discipline
is correct and stays; what goes away is 7 hand-synchronized edits and the ordering merge conflicts.

**DOWNGRADED — the generic struct-view primitive.** The claimed core cost does not exist: adding a
usercmd field is shim + prelude only (`usercmd_read(field:i32)` is already generic). This is shim
ergonomics plus retrofitting the already-generic `s2_usermsg_walk` resolver onto the older hardcoded
`UsercmdFieldCache`. Worth doing eventually; not core growth; not now.

Two findings deserve promotion above their dimension's framing:

- **The engine-call `pid` impersonation is a security bug, not bloat.** See A1.
- **`dispatch_damage`'s `break` at `Handled` is a behavior bug** — it skips other plugins' `onDamage`
  handlers, contradicting `ARCHITECTURE.md` and `multiplexer.rs`'s own
  `handled_does_not_short_circuit` test. The "locked decision #8" citation does not support it (the
  north-star spec only grants block power). No Monitor tier exists to truncate; the defect is that
  one plugin's `Handled` silently disables every other plugin's damage observer for that hit.

---

## 4. The smallest set of changes that stops core growing

Ordered by leverage/risk. Prerequisites explicit.

### (a) Do now — unblocks everything

**A1. Fix the engine-call `pid` impersonation.** `__s2_engine_call_ready` / `_receiverless` /
`_status` / `_invoke` each take the plugin id as **JS argument 0** and key `gamedata_calls`
authorization on it, while the raw natives sit on every plugin's global object. Because
`gamedata_calls::prepare` gates the `engine:calls` permission once at *registration*, a descriptor's
existence in the registry is its authorization — so any plugin can drive another plugin's
operator-allow-listed engine calls by passing that plugin's id as a string.
Citations **corrected to `517b71b`**: the four `let pid = args.get(0)` sites are
`core/src/v8host.rs:7586`, `:7598`, `:7612`, `:7632`; the permission gate is
`core/src/gamedata_calls.rs:257`. The fix is `current_plugin(scope)` (`core/src/v8host.rs:8929`),
already used by 58 other natives. ~20 lines. No prerequisites.

**A2. Externalize the prelude:** `INJECTED_STD_PRELUDE` → `core/js/<module>.js` + `include_str!`.
Byte-identical at runtime, precedent `9ed1db0`, deletes 1,805 lines from `v8host.rs` (the largest
single mechanical reduction available), and — the part that matters for the thesis — **every JS-only
API change stops being a core Rust edit.** Add ESLint over `core/js/` to `ci-js.sh`. No prerequisites;
do it before A4 so the dispatch refactor is not diffing against a 19k-line file.

**A3. Complete the lifecycle registry: third verb + second bucket.**
- Add `reset: Box<dyn Fn()>` to `OwnerScopedStore` + `sweep_reset()`; replace the ~21 owner-scoped
  lines of the shutdown litany. Preserve the current ordering split around `HOST.take()` — place the
  sweep where the mux clears sit today, do not consolidate to one insertion point blindly.
- Add a **disjoint** `process_singletons` registry (register + `reset_all`) for the ~27 host-global
  statics with no registration at all (`ADMIN_*`, `BAN_*`, `SCHEMA_OFFSETS`, cookies,
  `PLUGIN_PUBLISHES`…). This bug class has already shipped three times (`98cf483`/`8a06b4a`,
  `e40492d`, `7e62119`) and all three were in this unregistered bucket. Keep the two registries
  structurally separate — see (d)6.
Prerequisite for A4 (hook registration auto-registers both).

**A4. THE HEADLINE: one generic fan-out dispatcher + keyed channel store.**
- `core/src/channels.rs`: `Channels<H>` = keyed `Descriptor` (`multiplexer.rs`'s, unchanged — priority,
  phase, enabled, error_count come along for free). Retire `event_mux.rs`; its charter ("events don't
  collapse") is falsified by five of its own instances.
- `core/src/dispatch.rs`: `fan_out(snapshot, label, reentry, build_args) -> DispatchOutcome` owning,
  once: the `try_borrow_mut` re-entrancy guard, per-subscriber `is_live`, context clone,
  HandleScope/ContextScope/TryCatch, WARN + `report_js_error`, `run_chain` collapse, `apply_errors`,
  and `enter_dispatch` breadcrumbs. 18 dispatchers (~1,138 lines) become 3–15-line callers; the four
  pending-drain shells keep their ~10-line loop and terminal-close prunes; per-hook pre/post side
  effects (`voice_clear_slot`, `note_tick`, `refresh_detour`) stay at call sites.
- One generic `__s2_hook_on`/`__s2_hook_off` pair replaces the 19 subscribe natives; one generic ffi
  dispatch entry replaces the 14 bespoke `s2script_core_dispatch_*` exports for scalar-payload hooks
  (view-backed hooks — damage/usercmd/usermsg — keep their block-scoped views but route through
  `fan_out`).
- Structurally delivers three things currently wired for ~1 of 15–20 channels each: auto-disable
  (`apply_errors`: one caller today), crash-breadcrumb coverage (4 of 20 sites — the B2 live-gate
  failure was exactly a missed site), and priority/phase capability.
Prerequisites: A2, A3. After this, **a new engine notification = one table row + one shim call site +
a JS file edit.**

**A5. `defer: "nextFrame"` (+ ordered call chains) on `Engine.call` descriptors; migrate the ~13
CS2-signature ops to `@s2script/cs2` gamedata.** The shim-side next-frame drain already exists twice
(`terminate_round`, `respawn`) — this generalizes written code. Kills the single largest recurring
class of core edit ("Valve has a member function, wrap it"), retires
`player_respawn`/`switch_team`/`change_team`/`terminate_round`/`give_named_item` as ops, and moves CS2
signatures out of `gamedata/core.gamedata.jsonc` to where the boundary doctrine says they belong.
Independent of A4; can run in parallel. After A5 the switchteam-class slice (+40 core lines, zero
novel information) is a game-package gamedata entry with **zero core diff**.

**A6. Extract `FoldTable` (`merged_rules.rs`) from `TRANSMIT_RULES`/`VOICE_RULES`.** ~230 duplicated
lines, third instance free; the `liveness.rs` extraction (`5190980` → `4aafc42`) is the proven
behavior-preserving template. Keep the AND-fold direction and transmit's serial gate — see (d)7.
Depends on A3 for registration; otherwise independent.

**A7. Generate `S2EngineOps` struct/header/fixtures from one declarative list** (the corrected
version, not a runtime name-keyed table). Lowest urgency of the (a) set because A4+A5 shrink the rate
of new ops to near zero anyway.

### (b) Before freezing a 1.0 API — contract-breaking, gets harder later

- **B1.** Repair the one post-L1 drift: `Server.onCvarChange` → `ctx.server.onCvarChange` returning
  void; deprecate the ambient `{dispose()}` form for one minor. It breaks both locked L1 rules five
  days after they locked, with no recorded exception — unlike `UserMessages.onPre`, whose ambient
  status *is* locked (#9). Then freeze `PluginContext`/`Scope`/`PluginHooks`/`PluginFactory`; they are
  otherwise sound.
- **B2.** Fix `dispatch_damage`'s `Handled` short-circuit (falls out of A4 for free: post-fold
  `zero_current_damage()`), and either plumb priority through the collapsing subscribe surfaces or
  amend `events.d.ts`'s `Stop` wording — today the `.d.ts` promises ordering semantics the runtime
  cannot deliver. Changing ordering guarantees post-1.0 is breaking; do it now.
- **B3.** Decide `Scope` semantics for transmit/voice rules (real ids so `scope.clear()` works, or a
  documented exemption like locked #9). The current `remove_by_ids` no-ops are commented but
  unjustified.
- **B4. Do NOT freeze** `@s2script/sdk/unsafe`, `/usermessages`, `/transmit`, `/voice`, `/usercmd` at
  1.0 — they are the plugin-facing shadow of exactly the bespoke shapes A4–A6 replace.

### (c) Genuine bloat, cosmetic, low priority

- The `__s2_user_message_*` vs `__s2_usermsg_*` prefix split (internal only; rename whenever).
- `console.log`'s pre-convention registration.
- Retrofit `UsercmdFieldCache` onto the generic `s2_usermsg_walk`-style resolver (shim ergonomics).
- The per-access `__s2_schema_offset` memo hoist (`packages/sdk/src/schemagen/emit-js.ts`,
  `resolveExpr`) — offsets are already immutable once resolved (`OffsetCache` only ever inserts into
  `hits`; the sole reset is on core re-init, which rebuilds every context), so the per-access call and
  its two `to_rust_string_lossy` allocations are pure waste. A perf win, not a growth fix; land
  opportunistically. Prerequisite for making `__s2_ent_ref_read` a V8 Fast API candidate.
- The census's 8 cs2-typed natives (pawn/team/round): the doc-comment engine-genericity arguments are
  defensible, and A5 dissolves most of them anyway. Do not relitigate separately.

### (d) Looked like bloat — load-bearing. Leave alone, and why.

1. **The CJS `(function(require,module,exports){})` wrapper + esbuild-emits-CJS.** Deliberate: it is
   what makes plugin load synchronous (slice-4 spike findings). Plugins are authored as pure ESM; CJS
   is only the bundle output format. Do not "modernize" to ESM module loading — `Module::evaluate`
   returns a promise, which would make load asynchronous and admit top-level await.
2. **The `try_borrow_mut` graceful-skip, snapshot-before-invoke, per-handler TryCatch.** The isolate
   re-entrancy defense (`Events.fire` from a handler double-borrows). A4 centralizes them; nothing may
   remove them. The refactor's acceptance test is that these guards survive byte-for-byte in one place.
3. **SourceHook (`SH_DECL_HOOK*`) for virtual interface methods.** Cannot be data-driven (compile-time
   struct types) and provides cooperative chaining with other Metamod extensions. The per-hook shim
   C++ is the genuinely irreducible cost. Do not resurrect "gamedata-declared hooks for everything."
4. **Client-command NOT routed through `Engine.call`.** Documented kill: variadic `ClientCommand`,
   struct-by-ref `DispatchConCommand` — outside any closed marshalling vocabulary. Do not re-propose
   interface receivers for `Engine.call`.
5. **The next-frame deferral for respawn/terminateRound.** A real framework constraint (isolate borrow
   re-entrancy), not bloat — A5 makes it a descriptor flag; the mechanism stays.
6. **Host-global caches (`ADMIN_*`, `BAN_*`, `SCHEMA_OFFSETS`, cookies, `USERMSG_RESOLVE`) are NOT
   missing from `owner_stores` by oversight.** They are designed to survive any single plugin's
   unload. Registering them owner-scoped would wipe shared admin/ban state on an unrelated plugin
   reload. Hence the two structurally disjoint registries in A3.
7. **The AND-fold (most-restrictive-wins) in transmit/voice rules.** A safety choice — "no owner can
   WIDEN what another owner restricted" — and transmit's per-rule entity serial guards index reuse.
   `FoldTable` must carry both.
8. **The append-only ABI discipline on `S2EngineOps`.** Correct for two separately-built `.so` files;
   A7 generates it, never abandons it.
9. **Numeric field enums on the usercmd hot path.** Documented: no CS2 identifier crosses the C ABI,
   no string lookup per tick. Any future view primitive must keep the resolve-once/read-by-token shape.
10. **The per-descriptor `-1` sentinels, books-first entity liveness, block-scoped raw views.**
    Reconfirmed untouchable; no finding in this audit proposes otherwise.

---

## 5. The arithmetic, stated once

Today: a new hook ≈ 7–9 core edit sites + ffi + shim; a new engine call ≈ 40–170 core lines.

After A2+A3+A4: a new hook = 1 table row + 1 shim call site + 1 JS file edit (core Rust delta ≈ one
declarative line). After A5: a new engine call = a gamedata entry **in the game package**, core delta
zero — which is what movement-control already proved is possible.

The remaining genuine core work is then only: new detour/SourceHook categories, new marshal kinds,
new safety primitives. That is a stable core.
