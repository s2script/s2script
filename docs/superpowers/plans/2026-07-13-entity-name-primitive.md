# `entity_name` Primitive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an engine-generic `EntityRef.name` getter that reads an entity's targetname (`CEntityIdentity::m_name`), unblocking name-based zone discovery for the surftimer port (and any framework consumer).

**Architecture:** A new `entity_name(index, serial) → const char*` shim op reads `id->m_name.String()` — a direct sibling of the `m_designerName.String()` read that `entity_find_by_class` already performs on the same `CEntityIdentity`. A `__s2_entity_name(index, serial) → string | null` native copies the C string into V8 (mirrors `__s2_client_name`), and an `EntityRef.name` prelude getter exposes it. Serial-gated: `null` on a stale/invalid ref, `""` for an entity with no targetname.

**Tech Stack:** C++ shim (Metamod), Rust core (rusty_v8), JS prelude, TypeScript `.d.ts`.

**Why this is a slice, not a port-folder change:** the surftimer port is scoped to `../s2s-surftimer-port` and consumes published `@s2script/*` packages; it cannot add engine primitives. This one primitive is the sanctioned prerequisite (approved during brainstorm). It ships through the s2script repo's normal cadence and gets republished so the port can depend on it.

## Global Constraints

- **ABI append-only:** `entity_name` is APPENDED after `entity_listener_install` (the CURRENT ABI tail at this base — PR #23 added it after `entity_set_model`) in BOTH the C header struct (`shim/include/s2script_core.h`) AND the Rust mirror (`core/src/v8host.rs`). Order IS the ABI — never reorder existing fields. It must ALSO be added as `entity_name: None,` to the two in-test `S2EngineOps` literals in `core/src/v8host.rs`, or the crate won't compile.
- **Engine-generic:** the op signature is `(index, serial) → string`; NO CS2 class names in `core/src` or the C ABI. Both boundary gates (`scripts/check-*` core/std-vs-game) must stay green.
- **Degrade-never-crash:** the native wraps its body in `catch_unwind`; returns `null` on stale ref / absent ops. Never panics into the engine.
- **GLIBC floors (sniper):** `libs2script_core.so` ≤ 2.30, `s2script.so` ≤ 2.14 — unchanged (no new deps).
- **Packages change → changeset:** `packages/entity/index.d.ts` gains a member → a `minor` changeset for the `@s2script/*` fixed family.
- **Cadence:** dedicated worktree + `feat/entity-name` branch; live CS2 gate on `surf_kitsune` (wsid `3076153623`); PR.

---

### Task 1: Implement the `entity_name` primitive end-to-end

> **BASE-DRIFT CORRECTIONS (authoritative — override any line number below; locate anchors by SYMBOL NAME via grep, not the stale line numbers):**
> 1. The current ABI tail is **`entity_listener_install`** (PR #23 appended it after `entity_set_model`). Append `entity_name` AFTER `entity_listener_install` in ALL of: the C header typedef (after `s2_entity_listener_install_fn`), the C `S2EngineOps` struct field (after `s2_entity_listener_install_fn entity_listener_install;`, just before `} S2EngineOps;`), the Rust type alias (after `type EntityListenerInstallFn = ...`), the Rust `S2EngineOps` struct field (after `pub entity_listener_install: Option<EntityListenerInstallFn>,`), and the shim ops assignment (`ops.entity_name = &s2_entity_name;` after `ops.entity_listener_install = &Shim_EntityListenerInstall;`).
> 2. **Extra required edit not in the steps below:** `core/src/v8host.rs` has TWO in-test `S2EngineOps { … }` literals that list every field explicitly (each ends `…, entity_listener_install: None, }`). Add `entity_name: None,` after EACH `entity_listener_install: None,` (two sites) — otherwise the test build fails with "missing field `entity_name`". (This is Step 4b below.)
>
> Everywhere the steps say "after `entity_set_model`", read "after `entity_listener_install`".

**Files:**
- Modify: `shim/include/s2script_core.h` (typedef after :218; struct field after :316)
- Modify: `shim/src/s2script_mm.cpp` (impl near the `s2_entity_find_by_class` sibling ~:260; ops assignment after :2722)
- Modify: `core/src/v8host.rs` (type after :210; struct field after :312; native near the `s2_client_name` family; `set_native` registration near :5991; prelude getter after the `EntityRef.prototype` block ~:852; in-isolate test in the test module)
- Modify: `packages/entity/index.d.ts` (EntityRef member)

**Interfaces:**
- Produces: shim op `const char* entity_name(int index, int serial)`; native `__s2_entity_name(index, serial) → string | null`; `EntityRef.prototype.name` getter → `string | null`; `.d.ts` `readonly name: string | null`.

- [ ] **Step 1: C header — typedef + struct field.** In `shim/include/s2script_core.h`, after the `s2_entity_set_model_fn` typedef (~:218) add:

```c
/* entity_name: read an entity's targetname (CEntityIdentity::m_name, a CUtlSymbolLarge; String() is
 * inline). Serial-gated (index,serial). Returns the name ("" if the entity has no targetname), valid
 * during the call — the core copies immediately — or NULL if stale/invalid. ENGINE-GENERIC.
 * Zones/surftimer slice. */
typedef const char* (*s2_entity_name_fn)(int index, int serial);
```

and after the struct field `s2_entity_set_model_fn entity_set_model;` (:316) add:

```c
    /* entity_name slice — APPENDED after entity_listener_install; order is the ABI; do not reorder above. */
    s2_entity_name_fn entity_name;
```

- [ ] **Step 2: Shim impl.** In `shim/src/s2script_mm.cpp`, right after `s2_entity_find_by_class` (ends ~:260), add the sibling — same identity access, `m_name` instead of `m_designerName`, serial-gated:

```cpp
// Engine-op: read an entity's targetname (CEntityIdentity::m_name, a CUtlSymbolLarge; String() inline,
// utlsymbollarge.h). Serial-gated: resolves the identity at `index`, validates the captured `serial`
// via GetRefEHandle(), returns m_name.String() ("" if unnamed) or nullptr if stale/invalid/removed.
// Sibling of s2_entity_find_by_class (which reads m_designerName on the same identity). Engine-generic.
// C-ABI, called by the Rust core through the S2EngineOps table.
static const char* s2_entity_name(int index, int serial) {
    CGameEntitySystem* es = GetEntitySystem();
    if (!es) return nullptr;
    if (index < 0 || index >= MAX_TOTAL_ENTITIES) return nullptr;
    int chunk = index / MAX_ENTITIES_IN_LIST;
    int slot  = index % MAX_ENTITIES_IN_LIST;
    CEntityIdentity* chunk_base = es->m_EntityList.m_pIdentityChunks[chunk];
    if (!chunk_base) return nullptr;
    CEntityIdentity* id = &chunk_base[slot];
    if (id->m_flags & EF_IS_INVALID_EHANDLE) return nullptr;
    if (!id->m_pInstance) return nullptr;
    if (id->GetRefEHandle().GetSerialNumber() != serial) return nullptr;  // stale slot reuse
    return id->m_name.String();  // "" if the entity has no targetname
}
```

- [ ] **Step 3: Shim ops assignment.** In `shim/src/s2script_mm.cpp`, after `ops.entity_set_model = &Shim_EntitySetModel;` (~:2722) add:

```cpp
    ops.entity_name = &s2_entity_name;
```

- [ ] **Step 4: Rust type + struct field.** In `core/src/v8host.rs`, after `type EntitySetModelFn = ...` (:210) add:

```rust
type EntityNameFn = extern "C" fn(c_int, c_int) -> *const c_char;
```

and after `pub entity_set_model: Option<EntitySetModelFn>,` (:312, the current last field, just before the struct's closing `}`) add:

```rust
    // --- entity_name slice (APPENDED after entity_listener_install; order is the ABI; do not reorder above) ---
    pub entity_name: Option<EntityNameFn>,
```

- [ ] **Step 4b: Add `entity_name: None,` to the two in-test op-struct literals.** In `core/src/v8host.rs`, the test module builds `S2EngineOps { … }` twice, each listing every field (grep `entity_listener_install: None` — two hits). After EACH `entity_listener_install: None,` line add:

```rust
            entity_name: None,
```

  (Rust struct literals must list every field; without this the crate's test build fails with `missing field entity_name`.)

- [ ] **Step 5: Rust native.** In `core/src/v8host.rs`, beside the `s2_client_name` native (~:5461), add (mirrors it exactly, two args):

```rust
/// Native `__s2_entity_name(index, serial) -> string | null`. Reads CEntityIdentity::m_name via the
/// `entity_name` op; copies the C string now. null = stale/invalid/no-ops; "" = entity has no targetname.
fn s2_entity_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 2 { return; }
        let index  = args.get(0).int32_value(scope).unwrap_or(-1);
        let serial = args.get(1).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.entity_name else { return };
        let ptr = func(index, serial);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}
```

- [ ] **Step 6: Register the native.** In `core/src/v8host.rs`, in the `set_native` block beside the other `__s2_ent_ref_*` registrations (~:5991), add:

```rust
    set_native(scope, global_obj, "__s2_entity_name", s2_entity_name);
```

- [ ] **Step 7: Prelude getter.** In `core/src/v8host.rs`, after the `EntityRef.prototype.setModel = ...` line (~:851) add:

```javascript
  // Targetname (CEntityIdentity::m_name) — e.g. a map trigger's "map_start". null if stale; "" if unnamed.
  Object.defineProperty(EntityRef.prototype, "name", {
    get: function () { var n = __s2_entity_name(this.index, this.serial); return n == null ? null : n; }
  });
```

- [ ] **Step 8: TypeScript type.** In `packages/entity/index.d.ts`, inside `class EntityRef`, after the `readonly serial: number;` field, add:

```typescript
  /** This entity's targetname (`CEntityIdentity::m_name`) — e.g. a map trigger's `"map_start"`. `""` if
   *  the entity has no targetname; `null` if the ref is stale/invalid. */
  readonly name: string | null;
```

- [ ] **Step 9: Write the in-isolate degrade test.** In `core/src/v8host.rs`, in the `#[cfg(test)]` module beside the other `eval_std` tests, add:

```rust
    #[test]
    fn entity_name_degrades_to_null_without_ops() {
        init(dummy_logger()).unwrap();
        // No ENGINE_OPS are installed in-isolate -> the op is absent -> both paths return null.
        let out = eval_std("en1", r#"
            var EntityRef = globalThis.__s2pkg_entity.EntityRef;
            var direct = __s2_entity_name(5, 7);
            var viaRef = new EntityRef(5, 7).name;
            JSON.stringify({ direct: direct, viaRef: viaRef });
        "#);
        assert_eq!(out, r#"{"direct":null,"viaRef":null}"#);
        shutdown();
    }
```

- [ ] **Step 10: Run the core test suite.** Run: `cargo test -p s2script-core entity_name` (then the full `cargo test` to confirm no regression; `.cargo/config.toml` forces `RUST_TEST_THREADS=1`).
  Expected: `entity_name_degrades_to_null_without_ops ... ok`; full suite green (prior count + 1).

- [ ] **Step 11: Boundary gates.** Run the repo's `scripts/check-*.sh` boundary + generated-freshness gates (the core/std-vs-game boundary check must stay green — `entity_name` adds no CS2 names to core/shim).
  Expected: all gates pass.

- [ ] **Step 12: Commit.**

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp core/src/v8host.rs packages/entity/index.d.ts
git commit -m "feat(entity): entity_name primitive — EntityRef.name reads CEntityIdentity::m_name

Sibling of entity_find_by_class (m_designerName -> m_name). Serial-gated
(index,serial) -> string|null. Unblocks name-based zone discovery.

Claude-Session: https://claude.ai/code/session_01QQ9hUs6JMM29DUR29REUps"
```

---

### Task 2: Build, deploy, live-gate on surf_kitsune, changeset, PR

**Files:**
- Create: `.changeset/entity-name.md`
- (Live gate only — reuse `examples/schema-dump` or a throwaway example that dumps trigger names; no committed demo required.)

**Interfaces:**
- Consumes: Task 1's `EntityRef.name`.

- [ ] **Step 1: Sniper rebuild.** Rebuild both artifacts via the repo's established sniper build (shim op → `s2script.so`; core native + prelude → `libs2script_core.so`). Confirm GLIBC floors: `s2script.so` ≤ 2.14, `libs2script_core.so` ≤ 2.30.
  Expected: both `.so` build clean; GLIBC check passes.

- [ ] **Step 2: Deploy + restart.** Deploy the rebuilt runtime to the CS2 Docker addon dir and `docker compose restart cs2` (NOT `--force-recreate`; re-run `/patch-gameinfo.sh` only if `gameinfo.gi` was reset).
  Expected: boot log `=== GAMEDATA VALIDATION: N ok, 0 FAILED ===` (N unchanged — no new signatures); base suite loads; `RestartCount=0`.

- [ ] **Step 3: Load surf_kitsune + dump trigger names (the live gate).** From rcon: `host_workshop_map 3076153623`. Run a throwaway example (or an existing demo command) that does:

```javascript
import { Entity } from "@s2script/entity";
for (const t of Entity.findByClass("trigger_multiple")) {
  console.log(`[entity-name] trigger #${t.index} name=${JSON.stringify(t.name)}`);
}
```

  Expected: the log lists the map's real targetnames — includes CS2Surf reserved names such as `"map_start"`, `"map_end"`, and any `"stageN_start"` / `"bonusN_start"` / `"surftimer_reset"` present on surf_kitsune (NOT all `""`, NOT `null`). This is the definitive proof the primitive reads real names. Server keeps ticking, no crash.

- [ ] **Step 4: Add the changeset.** Create `.changeset/entity-name.md`:

```markdown
---
"@s2script/entity": minor
---

Add `EntityRef.name` — read an entity's targetname (`CEntityIdentity::m_name`). Serial-gated `string | null` (`""` when the entity has no targetname). Unblocks name-based entity/zone discovery.
```

- [ ] **Step 5: Commit + open the PR.**

```bash
git add .changeset/entity-name.md
git commit -m "chore(changeset): entity_name minor bump

Claude-Session: https://claude.ai/code/session_01QQ9hUs6JMM29DUR29REUps"
gh pr create --fill --title "feat(entity): entity_name primitive (EntityRef.name)"
```

  PR body ends with: `https://claude.ai/code/session_01QQ9hUs6JMM29DUR29REUps`

- [ ] **Step 6: Publish note for the port.** After merge + the `@s2script/entity` version bump is published, the port bumps its `@s2script/entity` dependency to the new minor (or links the local package during dev) so `EntityRef.name` typechecks and runs. Record the published version in the port's `GAP-ANALYSIS.md`.

---

## Self-Review

- **Spec coverage:** implements the §3 "blocking gap" (`entity_name`) exactly; nothing else in this plan's scope.
- **ABI:** appended after `entity_set_model` in both the C struct and the Rust mirror; no reorder. ✓
- **Type consistency:** shim `entity_name` ↔ Rust `EntityNameFn = extern "C" fn(c_int, c_int) -> *const c_char` ↔ native `__s2_entity_name(index, serial)` ↔ getter `EntityRef.prototype.name` ↔ `.d.ts` `readonly name: string | null`. All aligned. ✓
- **No placeholders:** every step has concrete code/commands. ✓
