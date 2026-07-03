# Slice 5C.4 — Pointer-chain field navigation → `pawn.origin` / `pawn.angles` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a generic engine-generic pointer-chain read primitive in core, and use it (hand-written in `pawn.js`) to ship `pawn.origin` (world position) + `pawn.angles` (body rotation) — fields that live behind a two-pointer chain the direct-field path can't reach.

**Architecture:** A new `__s2_ent_ref_read_floats_chain(idx, serial, ptrOffs[], finalOff, count)` native follows a chain of pointer derefs ENTIRELY in-core (the raw `CBodyComponent*`/`CGameSceneNode*` never cross to JS), serial-gated at the root entity, null-checking each hop, and returns a copied float triple. The CS2-specific chain (`m_CBodyComponent → m_pSceneNode → m_vecAbsOrigin`/`m_angAbsRotation`, offsets live-resolved) lives hand-written in `pawn.js` (mirrors the 5C.2 player nav). Touches core → one sniper rebuild.

**Tech Stack:** Rust `cdylib` core (rusty_v8 149.4.0), the injected JS prelude, `games/cs2/js/pawn.js`, `node:test` vm-compose, the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §10):

- **Core stays engine-generic.** The pointer-chain native follows a *generic* offset list; the CS2 chain knowledge (`m_CBodyComponent`/`m_pSceneNode`/`m_vecAbsOrigin`/`m_angAbsRotation`) lives ONLY in `pawn.js` + `packages/cs2`. NO CS2 identifiers in `core/src`. Both gates green: `bash scripts/check-core-boundary.sh`, `bash scripts/test-boundary-nameleak.sh`.
- **Never expose a raw pointer across time.** The intermediate pointers are followed + read within ONE synchronous native and never cross to JS; only the copied `{x,y,z}` value returns. Each hop null-checked; the root entity serial-gated → `T | null`.
- **Layout is data.** Every offset (the chain + the final field) resolves live via `__s2_schema_offset`; nothing baked.
- **cdylib:** core in-isolate tests inline `#[cfg(test)] mod`.
- **Naming:** PascalCase types (`Vector`, `QAngle`), camelCase methods/props (`readFloatsChain`, `origin`, `angles`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5c4-origin-ptr-nav`; do NOT push.

**Deferred — do NOT build:** codegen auto-generation of embedded/ptr accessors; the quantized `m_vecOrigin` wrapper; Vector/origin **writes** (teleport = an engine `Teleport()`); a generic scalar-behind-pointer read (this native reads floats only); the engine-identity follow; the game-event system; the `tsc` gate; the registry (5.5); the base suite (6).

**Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs` (scoped glob).

---

## Task 1: The pointer-chain native + `EntityRef.readFloatsChain` + `.d.ts` (cargo-in-isolate)

**Files:**
- Modify: `core/src/v8host.rs` (the native + install + the `EntityRef` prelude method + an in-isolate test), `packages/entity/index.d.ts`

**Interfaces:**
- Consumes: `entity_resolve_ptr` (serial-gated root), `crate::entity::read_ptr(base,off)->*const u8` (deref, guarded), `crate::entity::read_f32` (final read), `set_native`, the `v8::Local::<v8::Array>::try_from` pattern (v8host.rs:1366).
- Produces (for Task 2): `__s2_ent_ref_read_floats_chain(index, serial, ptrOffs, finalOff, count) → number[] | null`; `EntityRef.readFloatsChain(ptrOffs, finalOff, count) → number[] | null`.

- [ ] **Step 1: Write the failing test** (add to `#[cfg(test)] mod frame_tests`):

```rust
    #[test]
    fn read_floats_chain_degrades_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);
        create_plugin_context("p");
        // the native degrades to null (no engine ops → entity_resolve_ptr null, before any deref):
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,[48,8],200,3))"), "null");
        // guards: a non-array chain, a negative finalOff, and a bad count all → null:
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,42,200,3))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,[48,8],-1,3))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_ent_ref_read_floats_chain(1,7,[48,8],200,9))"), "null");
        // the EntityRef method degrades to null:
        assert_eq!(eval_in_context_string("p", r#"var {EntityRef}=__s2require("@s2script/entity"); String(new EntityRef(1,7).readFloatsChain([48,8],200,3))"#), "null");
        shutdown();
    }
```
(Mirror the exact harness of the neighboring `read_floats_native_and_method_degrade_without_ops` test if `eval_in_context_string`/`create_plugin_context` differ.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p s2script-core frame_tests::read_floats_chain_degrades_without_ops -- --test-threads=1` → FAIL (`__s2_ent_ref_read_floats_chain` undefined → throws).

- [ ] **Step 3: Add the native** (near `s2_ent_ref_read_floats` in `v8host.rs`):

```rust
/// Native `__s2_ent_ref_read_floats_chain(index, serial, ptrOffs, finalOff, count) -> number[] | null`.
/// Follows a chain of pointer derefs (each i32 offset in the `ptrOffs` JS array), then reads `count` (1..=4)
/// contiguous f32s at `finalOff` into a COPIED JS array. Serial-gated at the root entity; each hop null-checked;
/// the raw intermediate pointers never cross to JS. null on a stale root / a null hop / a bad chain/offset/count.
fn s2_ent_ref_read_floats_chain(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        let index = args.get(0).integer_value(scope).unwrap_or(-1) as i32;
        let serial = args.get(1).integer_value(scope).unwrap_or(-1) as i32;
        let final_off = args.get(3).integer_value(scope).unwrap_or(-1) as i32;
        let count = args.get(4).integer_value(scope).unwrap_or(0) as i32;
        if count <= 0 || count > 4 || final_off < 0 { return; }
        // args[2] must be an array of pointer offsets:
        let Ok(chain) = v8::Local::<v8::Array>::try_from(args.get(2)) else { return; };
        let ent = entity_resolve_ptr(index, serial);
        if ent.is_null() { return; }                     // stale/invalid root → null (already set)
        let mut p = ent as *const u8;
        for i in 0..chain.length() {
            let off = chain.get_index(scope, i).and_then(|v| v.integer_value(scope)).unwrap_or(-1) as i32;
            if off < 0 { return; }                       // bad offset in the chain → null
            p = crate::entity::read_ptr(p, off);
            if p.is_null() { return; }                   // a null hop (broken chain) → null
        }
        let out = v8::Array::new(scope, count);
        for i in 0..count {
            let v = crate::entity::read_f32(p, final_off + i * 4) as f64;
            let num = v8::Number::new(scope, v);
            out.set_index(scope, i as u32, num.into());
        }
        rv.set(out.into());
    }));
}
```
(Note: `read_ptr` returns `*const u8` and is null/negative-offset guarded; `entity_resolve_ptr` is the serial-gated root resolve. If the exact `try_from`/`get_index` signatures differ in the pinned rusty_v8, adapt to the working form used at v8host.rs:1366 for `try_from` and the existing `v8::Array` usage for indexing.)

- [ ] **Step 4: Register the native.** In `install_natives`, next to `__s2_ent_ref_read_floats`:
`set_native(scope, global_obj, "__s2_ent_ref_read_floats_chain", s2_ent_ref_read_floats_chain);`

- [ ] **Step 5: Add the `EntityRef` prelude method.** In `INJECTED_STD_PRELUDE`, on the `EntityRef.prototype` object (next to `readFloats`):

```js
    readFloatsChain: function (chain, finalOff, count) { return __s2_ent_ref_read_floats_chain(this.index, this.serial, chain, finalOff, count); },
```

- [ ] **Step 6: Update `packages/entity/index.d.ts`** — add to the `EntityRef` class:

```ts
  /** Follow a chain of pointer derefs (each an offset into the current target), then read `count` (1..4) floats
   *  at `finalOff` into a number[]. All in-core (raw pointers never cross); null if the root is stale or any hop
   *  is null. */
  readFloatsChain(ptrOffs: number[], finalOff: number, count: number): number[] | null;
```

- [ ] **Step 7: Run + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs packages/entity/index.d.ts
git commit -m "feat(slice5c4): __s2_ent_ref_read_floats_chain native + EntityRef.readFloatsChain (in-core pointer-chain read)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: Hand-written `pawn.origin` + `pawn.angles` + types + vm test (node:test)

**Files:**
- Modify: `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`, `packages/cli/test/schema-runtime.test.mjs`

**Interfaces:**
- Consumes: Task-1 `EntityRef.readFloatsChain`; the 5C.3 `@s2script/math` `Vector`/`QAngle`; `__s2_schema_offset`.
- Produces: `pawn.origin → Vector | null`, `pawn.angles → QAngle | null`.

- [ ] **Step 1: Write the failing vm-compose test** (append to `packages/cli/test/schema-runtime.test.mjs`):

```js
test("pawn.origin / pawn.angles: pointer-chain accessors read a value, degrade to null (offline vm)", () => {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  let chainRet = [64, 128, 256];
  EntityRef.prototype.readFloatsChain = function () { return chainRet; };   // toggled to null below
  function Vector(x, y, z) { this.x = x; this.y = y; this.z = z; }
  function QAngle(x, y, z) { this.x = x; this.y = y; this.z = z; }
  const math = { Vector, QAngle };
  let offRet = 8;                                        // schema-offset stub; toggled to -1 below
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : n === "@s2script/math" ? math : null),
    __s2_schema_offset: () => offRet,
    __s2_ent_current_serial: () => 7, __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  const { Pawn } = ctx.__s2pkg_cs2;
  const p = new Pawn(new EntityRef(5, 9));
  assert.ok(p.origin instanceof Vector, "origin is a Vector");
  assert.deepEqual([p.origin.x, p.origin.y, p.origin.z], [64, 128, 256]);
  assert.ok(p.angles instanceof QAngle, "angles is a QAngle");
  chainRet = null;                                      // stale/broken chain → readFloatsChain null
  assert.equal(p.origin, null, "a null readFloatsChain → the accessor returns null");
  chainRet = [1, 2, 3]; offRet = -1;                    // a schema-offset miss → null (before the chain read)
  assert.equal(p.origin, null, "a missing offset → the accessor returns null");
});
```

- [ ] **Step 2: Run to verify failure** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schema-runtime.test.mjs` → FAIL (`p.origin` is `undefined`, not a `Vector`).

- [ ] **Step 3: Add the math require + the accessors to `pawn.js`.** Near the top (after `var EntityRef = __s2require("@s2script/entity").EntityRef;`):

```js
  var math = __s2require("@s2script/math");
  var Vector = math.Vector, QAngle = math.QAngle;
```
Then add the two accessors alongside the other `Pawn.prototype` nav blocks (after the `pawn.controller` `defineProperty`):

```js
  // pawn.origin -> world-space position: entity -> m_CBodyComponent(ptr) -> m_pSceneNode(ptr) -> m_vecAbsOrigin.
  // The pointer chain is followed in-core (readFloatsChain); the raw component/node pointers never reach JS.
  Object.defineProperty(Pawn.prototype, "origin", {
    get: function () {
      var bodyOff = __s2_schema_offset("CBaseEntity", "m_CBodyComponent");
      var sceneOff = __s2_schema_offset("CBodyComponent", "m_pSceneNode");
      var off = __s2_schema_offset("CGameSceneNode", "m_vecAbsOrigin");
      if (bodyOff < 0 || sceneOff < 0 || off < 0) return null;
      var a = this.ref.readFloatsChain([bodyOff, sceneOff], off, 3);
      return a === null ? null : new Vector(a[0], a[1], a[2]);
    }, enumerable: true, configurable: true,
  });
  // pawn.angles -> body world rotation via the same chain -> m_angAbsRotation (distinct from eyeAngles = view/aim).
  Object.defineProperty(Pawn.prototype, "angles", {
    get: function () {
      var bodyOff = __s2_schema_offset("CBaseEntity", "m_CBodyComponent");
      var sceneOff = __s2_schema_offset("CBodyComponent", "m_pSceneNode");
      var off = __s2_schema_offset("CGameSceneNode", "m_angAbsRotation");
      if (bodyOff < 0 || sceneOff < 0 || off < 0) return null;
      var a = this.ref.readFloatsChain([bodyOff, sceneOff], off, 3);
      return a === null ? null : new QAngle(a[0], a[1], a[2]);
    }, enumerable: true, configurable: true,
  });
```

- [ ] **Step 4: Run to verify pass** — same command → PASS.

- [ ] **Step 5: Add the types** to `packages/cs2/index.d.ts`. Add the import near the top (after the `EntityRef` import):
`import type { Vector, QAngle } from "@s2script/math";`
and add to the `Pawn` interface (alongside `controller`):

```ts
  /** World-space position (via the CGameSceneNode pointer chain), or null if stale. */
  readonly origin: Vector | null;
  /** Body world rotation (via the CGameSceneNode pointer chain); distinct from the view/aim `eyeAngles`. */
  readonly angles: QAngle | null;
```

- [ ] **Step 6: Full CLI suite + gates + commit**

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add games/cs2/js/pawn.js packages/cs2/index.d.ts packages/cli/test/schema-runtime.test.mjs
git commit -m "feat(slice5c4): hand-written pawn.origin + pawn.angles (pointer-chain nav) + types + vm test

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```
(No `gen-schema` regen — this task is hand-written JS + types only; `check-schema-generated.sh` should stay green untouched. If it fails, the generated files were unexpectedly modified — investigate.)

---

## Task 3: Sniper build + spike/live gate + README/CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`; **create** a dated spike-findings doc.

**Interfaces:**
- Consumes: the Task-2 generated `pawn.origin` + `pawn.angles`.

**Needs ONE sniper rebuild** (Task 1 added a core native + prelude method).

- [ ] **Step 1: Sniper build + package.**

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```
Confirm GLIBC ≤ 2.31.

- [ ] **Step 2: SPIKE + gate demo.** Rewrite `examples/demo-plugin/src/plugin.ts` to read the pointer-chain accessors:

```ts
import { OnGameFrame } from "@s2script/frame";
import { Player } from "@s2script/cs2";

// Slice 5C.4 — pointer-chain nav. Every ~256 frames, read each in-game player's pawn world position +
// rotation via the generated pointer-chain accessors (entity -> CBodyComponent -> CGameSceneNode).
let ticks = 0;
export function onLoad(): void {
  console.log("[demo] onLoad (origin/angles pointer-chain)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    const players = Player.all();
    console.log("[demo] tick " + ticks + " players=" + players.length);
    for (const p of players) {
      const body = p.pawn;
      if (!body) { console.log("  slot=" + p.slot + " (no pawn)"); continue; }
      const o = body.origin;                              // Vector | null (world position)
      const a = body.angles;                              // QAngle | null (body rotation)
      console.log("  slot=" + p.slot
        + " origin=" + (o ? o.toString() : "null")
        + " angles=" + (a ? a.toString() : "null"));
    }
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```
Build the `.s2sp` (`node packages/cli/dist/cli.js build examples/demo-plugin`), deploy (`mkdir -p dist/addons/s2script/plugins && rm -f dist/addons/s2script/plugins/*.s2sp && cp examples/demo-plugin/dist/*.s2sp dist/addons/s2script/plugins/`), restart (`docker restart s2script-cs2`), wait past the boot window, `bot_quota 2` via `python3 scripts/rcon.py`, read `docker logs s2script-cs2 | grep '[demo]'`.
**SPIKE verdict:** `origin` must be a **plausible de_inferno map coordinate** (map extents are order ±1000..±3000; a spawn point is a specific nonzero `{x,y,z}`), NOT `(0,0,0)`, NOT garbage (e.g. `1e30`), NOT `null`. `angles` a plausible rotation. If `origin` is `(0,0,0)`/garbage/null, the chain is wrong — HALT and add a raw diagnostic (`p.pawn.ref.readFloatsChain([bodyOff, sceneOff], originOff, 3)` with the offsets logged; check each `__s2_schema_offset` is ≥ 0 and whether an intermediate deref is null by testing a one-hop chain) to isolate whether it's an offset, a wrong intermediate class name, or the native. Record the observed offsets + values in `docs/superpowers/specs/2026-07-02-slice-5c4-spike-findings.md`.

- [ ] **Step 3: Degrade test.** `bot_kick` → `players=0`; the accessors are unreached (no players), server ticking, no crash. (A stored `Pawn`'s `origin` would read `null` on the stale ref — the serial gate in the native; the occupancy filter already drops the players.) Capture the log. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with commands/errors.

- [ ] **Step 4: README + CLAUDE.**
  - `README.md`: add a `## Pointer-chain fields — origin (Slice 5C.4)` section — the `readFloatsChain` in-core pointer-follow primitive (raw pointers never cross to JS), the two-pointer chain to `origin`, that `pawn.origin` (Vector) + `pawn.angles` (QAngle) are hand-written (CS2 chain in `pawn.js`, offsets live-resolved), and the captured live log. Note codegen generalization, the quantized wrapper, and writes/teleport are deferred.
  - `CLAUDE.md` "## Current state": Slice 5C.4 done (pointer-chain field navigation — a generic `__s2_ent_ref_read_floats_chain` native follows a deref chain in-core [raw pointers never cross], serial-gated; hand-written `pawn.origin`/`pawn.angles` via `m_CBodyComponent → m_pSceneNode → m_vecAbsOrigin`/`m_angAbsRotation`; the embedded/ptr codegen generalization stays deferred). "Current focus" → next (engine-identity, the game-event system, or codegen generalization). Do NOT alter the standing conventions.

- [ ] **Step 5: Final verification + commit** (no build artifacts):

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md docs/superpowers/specs/2026-07-02-slice-5c4-spike-findings.md
git commit -m "feat(slice5c4): live gate PASSED — pawn.origin + pawn.angles (pointer-chain nav); spike + README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §8)

1. `cargo test -p s2script-core` green (new in-isolate test); the CLI `node:test` suite green (the vm-compose origin/angles test); both boundary gates + `check-schema-generated.sh` green; sniper build clean.
2. `pawn.origin` (Vector) + `pawn.angles` (QAngle) read via the in-core pointer chain; a stale root / null hop / missing offset → `null`.
3. Spike: `origin` reads a plausible de_inferno map coordinate live (not zero/garbage). Live gate: `players=0` on `bot_kick`, server ticking, no crash.
4. README + CLAUDE updated.
