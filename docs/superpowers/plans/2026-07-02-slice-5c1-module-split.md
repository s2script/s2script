# Slice 5C.1 — The Module Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dissolve the monolithic `@s2script/std` into first-class per-capability engine-generic module packages (`@s2script/entity`, `/frame`, `/timers`, `/console`, `/interfaces`) and retire `@s2script/std`, so authors write `import { EntityRef } from "@s2script/entity"`.

**Architecture:** Generalize the core `s2require` native to a single rule (`@s2script/<name>` → `globalThis["__s2pkg_"+<name>]`); reorganize the core-embedded engine-generic prelude to set the five `__s2pkg_<module>` globals (dropping `__s2pkg_std`); split `packages/std/index.d.ts` into five types-only packages; externalize `@s2script/*` by wildcard in the CLI; migrate every consumer in the same slice. No new API surface.

**Tech Stack:** Rust `cdylib` core (rusty_v8), the injected JS prelude, the TypeScript/esbuild CLI, `node:test`, the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §12):

- **Core stays engine-generic.** The generalized `s2require` + the module prelude apply a `@s2script/<name>` → `__s2pkg_<name>` rule that knows no specific module or game. NO CS2 identifiers in `core/src`. Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Back-compat within the slice.** After the split every capability is reachable from its new module; `pawn.js`, the injected preludes, and all consumers are repointed IN THIS SLICE so nothing breaks at load.
- **Deterministic codegen stays green.** The `emit-dts.ts` change regenerates `packages/cs2/schema.generated.d.ts` (via `s2script gen-schema`); `bash scripts/check-schema-generated.sh` must pass. Do NOT hand-edit the generated file.
- **Naming:** package names lowercase `@s2script/<mod>`; PascalCase types, camelCase fns/props unchanged.
- **cdylib:** core tests inline in `#[cfg(test)] mod`.
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5c1-module-split`; do NOT push.

**Deferred — do NOT build:** any NEW std breadth (Vector value type, timers/events/string/math/commands/convars surface — 5C.3+), the player model (5C.2), the `@s2script/cs2` internal split, a convenience umbrella package, the `tsc` typecheck gate, config/permissions, the registry (5.5), the base-plugin suite (6), the 5B.3 codegen post-merge TODOs.

**Module ← export mapping (the taxonomy):**
| Package | Runtime global | Exports |
|---|---|---|
| `@s2script/entity` | `__s2pkg_entity` | `EntityRef` |
| `@s2script/frame` | `__s2pkg_frame` | `OnGameFrame`, `SubscribeOptions` (type) |
| `@s2script/timers` | `__s2pkg_timers` | `delay`, `nextTick`, `nextFrame`, `threadSleep` |
| `@s2script/console` | `__s2pkg_console` | `console` |
| `@s2script/interfaces` | `__s2pkg_interfaces` | `publishInterface`, `PublishHandle` (type) |

(`HookResult`/`Priority`/`Phase` stay ambient raw-context globals — NOT module exports.)

---

## Task 1: Core — generalize `s2require` + reorganize the prelude + repoint core tests

**Files:**
- Modify: `core/src/v8host.rs` (the `s2require` native ~L970–985; the `INJECTED_STD_PRELUDE` ~L263–355; the in-isolate tests ~L2389, ~L3086, ~L3111, ~L3134)

**Interfaces:**
- Consumes: the existing `__s2_*` natives (unchanged), `set_native`, the `frame_tests` helpers (`init`/`set_engine_ops`/`create_plugin_context`/`eval_in_context_string`/`load_plugin_js`/`read_global_string`/`shutdown`/`dummy_logger`).
- Produces: `require("@s2script/<mod>")` resolving to `globalThis.__s2pkg_<mod>`; the five module globals; `require("@s2script/std")` → `null`.

- [ ] **Step 1: Write the failing test** (add to `#[cfg(test)] mod frame_tests` in `v8host.rs`):

```rust
    #[test]
    fn require_resolves_module_packages_and_retires_std() {
        let _ = init(dummy_logger());
        // Use load_plugin_js (the CJS wrapper where `require` is defined + the prelude has run),
        // then read the results back — this exercises the full require→__s2require→module-global path.
        load_plugin_js("mods", r#"
            globalThis.__t_entity  = typeof require("@s2script/entity").EntityRef;            // "function"
            globalThis.__t_frame   = typeof require("@s2script/frame").OnGameFrame;            // "object"
            globalThis.__t_timers  = typeof require("@s2script/timers").delay;                 // "function"
            globalThis.__t_console = typeof require("@s2script/console").console;              // "object"
            globalThis.__t_iface   = typeof require("@s2script/interfaces").publishInterface;  // "function"
            globalThis.__t_std     = String(require("@s2script/std"));                         // "null" (retired)
            globalThis.__t_nope    = String(require("@s2script/nope"));                        // "null"
        "#);
        assert_eq!(read_global_string("mods", "__t_entity"), "function");
        assert_eq!(read_global_string("mods", "__t_frame"), "object");
        assert_eq!(read_global_string("mods", "__t_timers"), "function");
        assert_eq!(read_global_string("mods", "__t_console"), "object");
        assert_eq!(read_global_string("mods", "__t_iface"), "function");
        assert_eq!(read_global_string("mods", "__t_std"), "null");
        assert_eq!(read_global_string("mods", "__t_nope"), "null");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p s2script-core frame_tests::require_resolves_module_packages_and_retires_std -- --test-threads=1`
Expected: FAIL — `require("@s2script/entity")` resolves to `null` today (only `std`/`cs2` are mapped), so `require("@s2script/entity").EntityRef` throws a TypeError (reading a property of null) and the globals are never set → the assertions fail.

- [ ] **Step 3: Generalize `s2require`.** In `core/src/v8host.rs`, replace the hardcoded `match` in `fn s2require` with the prefix rule:

```rust
        let name = args.get(0).to_rust_string_lossy(scope);
        // First-party rule: @s2script/<name> → globalThis.__s2pkg_<name> (engine-generic; no module list
        // hardcoded; @s2script/cs2 → __s2pkg_cs2 subsumed). Non-@s2script specifiers → null (the JS
        // `__s2_require` shim resolves those as inter-plugin deps). A retired/unknown name → the global is
        // undefined → null.
        let Some(rest) = name.strip_prefix("@s2script/") else { return };
        let key = format!("__s2pkg_{}", rest);
        let global = scope.get_current_context().global(scope);
        let Some(k) = v8::String::new(scope, &key) else { return };
        if let Some(v) = global.get(scope, k.into()) {
            if !v.is_undefined() {
                rv.set(v);
            }
        }
```

(Delete the old `let key = match name.as_str() { … };` block. Keep the surrounding `catch_unwind` + `rv.set_null()` + `args.length()` guard unchanged.)

- [ ] **Step 4: Reorganize the prelude to set the five module globals.** In `INJECTED_STD_PRELUDE`:
  - REPLACE the `const std = { OnGameFrame, delay: …, nextTick: …, nextFrame: …, threadSleep: …, console };` object with a `timers` object (the async fns only):

```js
  const timers = {
    delay: (ms) => __s2_delay(ms || 0),
    nextTick: () => __s2_next_tick(),
    nextFrame: () => __s2_next_frame(),
    threadSleep: (ms) => __s2_thread_sleep(ms || 0),
  };
```

  - REPLACE `std.publishInterface = function (name, version, impl) { … };` with an `interfaces` object:

```js
  const interfaces = {
    publishInterface: function (name, version, impl) {
      __s2_iface_publish(name, version, impl);
      return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
    },
  };
```

  - DELETE `std.EntityRef = EntityRef;` (the `EntityRef` function/prototype definition stays — it is still used by `__s2_entref_replacer`/`reviver`).
  - REPLACE the final `globalThis.__s2pkg_std = std;` with the five module globals (place AFTER `EntityRef` is defined):

```js
  globalThis.__s2pkg_entity     = { EntityRef: EntityRef };
  globalThis.__s2pkg_frame      = { OnGameFrame: OnGameFrame };
  globalThis.__s2pkg_timers     = timers;
  globalThis.__s2pkg_console    = { console: console };
  globalThis.__s2pkg_interfaces = interfaces;
```

  - Update the `__s2_require` shim comment `// @s2script/std | @s2script/cs2` → `// first-party @s2script/* module or game package`.

- [ ] **Step 5: Repoint the existing in-isolate tests** (they used `@s2script/std`; the retired package now resolves null, so they must target the modules):
  - The `~L2389` async-boot test destructure:
    `const {{ OnGameFrame, delay, nextTick, nextFrame, threadSleep }} = __s2require(\"@s2script/std\");`
    → split into two requires:
    `const {{ OnGameFrame }} = __s2require(\"@s2script/frame\");\nconst {{ delay, nextTick, nextFrame, threadSleep }} = __s2require(\"@s2script/timers\");`
  - `~L3086` `const { EntityRef } = require("@s2script/std");` → `require("@s2script/entity")`.
  - `~L3111` `__s2require("@s2script/std").EntityRef` → `__s2require("@s2script/entity").EntityRef`.
  - `~L3134` `const { publishInterface, EntityRef } = require("@s2script/std");` → split:
    `const { publishInterface } = require("@s2script/interfaces");\nconst { EntityRef } = require("@s2script/entity");`
  - Grep `core/src/v8host.rs` for any remaining `@s2script/std` and repoint it.

- [ ] **Step 6: Run the new test + full suite**

Run: `cargo test -p s2script-core -- --test-threads=1`
Expected: green — the new `require_resolves_module_packages_and_retires_std` plus every repointed test (the async-boot, EntityRef-degrade, registered-package, publishInterface tests). `grep -n '@s2script/std' core/src/v8host.rs` returns nothing.

- [ ] **Step 7: Gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs
git commit -m "feat(slice5c1): generalize s2require (@s2script/<name>->__s2pkg_<name>); prelude sets 5 module globals

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: The five module packages + the CLI wildcard external

**Files:**
- Create: `packages/{entity,frame,timers,console,interfaces}/{package.json,index.d.ts}`
- Modify: `packages/cli/src/build.ts` (the `external` array)
- (packages/std/ is NOT deleted here — that happens in Task 3 after consumers are repointed)

**Interfaces:**
- Consumes: `packages/std/index.d.ts` (the source of the type slices).
- Produces: the five `@s2script/<mod>` type packages; esbuild externalizes `@s2script/*`.

- [ ] **Step 1: Create the five package.json files.** Each mirrors `packages/cs2/package.json`:

`packages/entity/package.json`:
```json
{
  "name": "@s2script/entity",
  "version": "0.1.0",
  "types": "index.d.ts",
  "description": "Type stubs for the @s2script/entity injected API (serial-gated EntityRef). No runtime code."
}
```
Repeat for `frame`, `timers`, `console`, `interfaces` (same shape; adjust `name` + `description`: frame = "OnGameFrame + frame subscription"; timers = "delay/nextTick/nextFrame/threadSleep"; console = "engine console"; interfaces = "typed inter-plugin interfaces (publishInterface)").

- [ ] **Step 2: Create the five index.d.ts (split from `packages/std/index.d.ts`).**

`packages/entity/index.d.ts` — the `EntityRef` class (verbatim lines 55–92 of `packages/std/index.d.ts`) under a fresh header:
```ts
/**
 * @s2script/entity — author-time type stubs for the injected entity API.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * A serial-gated handle to a live entity. Wraps the `__s2_ent_ref_*` natives; the raw
 * entity pointer never crosses to JS. All accessors degrade safely (return null/false)
 * when the entity slot has been reused or the ops table is absent.
 */
export declare class EntityRef {
  readonly index: number;
  readonly serial: number;
  constructor(index: number, serial: number);
  isValid(): boolean;
  readInt32(offset: number): number | null;
  writeInt32(offset: number, value: number): boolean;
  readFloat32(offset: number): number | null;
  writeFloat32(offset: number, value: number): boolean;
  readBool(offset: number): boolean | null;
  writeBool(offset: number, value: boolean): boolean;
  readInt8(offset: number): number | null;
  readInt16(offset: number): number | null;
  readUInt8(offset: number): number | null;
  readUInt16(offset: number): number | null;
  readUInt32(offset: number): number | null;
  readHandle(offset: number): EntityRef | null;
  notifyStateChanged(offset: number): void;
}
```
(Preserve the per-method doc comments from the original — copy them across.)

`packages/frame/index.d.ts`:
```ts
/**
 * @s2script/frame — author-time type stubs for the per-frame subscription API.
 * NO runtime code: the engine injects the implementation at load time.
 */

export interface SubscribeOptions {
  priority?: number;
}

export declare const OnGameFrame: {
  /** Register a callback that fires every game frame. */
  subscribe(fn: () => void, opts?: SubscribeOptions): void;
};
```

`packages/timers/index.d.ts`:
```ts
/**
 * @s2script/timers — author-time type stubs for the async timing API.
 * NO runtime code: the engine injects the implementation at load time.
 */

/** Await a delay of `ms` milliseconds before continuing. */
export declare function delay(ms: number): Promise<void>;
/** Yield to the next microtick. */
export declare function nextTick(): Promise<void>;
/** Yield until the next game frame. */
export declare function nextFrame(): Promise<void>;
/**
 * Block the current thread (fiber) for `ms` milliseconds.
 * Only valid inside a threadSleep-capable fiber context.
 */
export declare function threadSleep(ms: number): void;
```

`packages/console/index.d.ts`:
```ts
/**
 * @s2script/console — author-time type stubs for the engine console.
 * NO runtime code: the engine injects the implementation at load time.
 */

/** Engine-provided console (same interface as globalThis.console). */
export declare const console: typeof globalThis.console;
```

`packages/interfaces/index.d.ts` — the `PublishHandle` + `publishInterface` slice (verbatim lines 34–53 of the original) under a fresh header:
```ts
/**
 * @s2script/interfaces — author-time type stubs for typed inter-plugin interfaces.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * Handle returned by {@link publishInterface}: lets the producer emit forwarded
 * events to every plugin subscribed to this interface via its `on(event, …)`.
 */
export interface PublishHandle {
  /** Emit a forwarded event to all consumers subscribed via `interface.on(event, …)`. */
  emit(event: string, payload: unknown): void;
}

/**
 * Publish a typed inter-plugin interface under `name`@`version`. `impl`'s methods
 * become the natives consumers call (`interface.method(...)`); the returned handle's
 * `emit` fans forwarded events out to consumers' `on(event, …)` subscriptions.
 * Auto-ledgered: the interface is withdrawn (and hard-dep consumers degraded) on unload.
 */
export declare function publishInterface(
  name: string,
  version: string,
  impl: Record<string, (...args: any[]) => any>,
): PublishHandle;
```

- [ ] **Step 3: Externalize `@s2script/*` by wildcard.** In `packages/cli/src/build.ts`, replace the two explicit entries in the `external` array:

```ts
  const external = Array.from(new Set([
    "@s2script/*",
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]));
```

(esbuild supports the `*` wildcard in `external`; `@s2script/*` matches every first-party module AND `@s2script/cs2` AND the still-present `@s2script/std`, so nothing breaks before Task 3 migrates the fixtures.)

- [ ] **Step 4: Verify the CLI suite still builds (wildcard keeps existing fixtures external)**

Run: `cd packages/cli && node build.mjs && node --experimental-strip-types --no-warnings --test test/*.test.mjs`
Expected: green — the existing `build.test.mjs` still passes (the `@s2script/*` wildcard keeps the fixtures' current `@s2script/std` import external → `require("@s2script/std")` still appears in the bundle). Both boundary gates unaffected.

- [ ] **Step 5: Commit**

```bash
cd /home/gkh/projects/s2script
git add packages/entity packages/frame packages/timers packages/console packages/interfaces packages/cli/src/build.ts
git commit -m "feat(slice5c1): 5 @s2script/<module> type packages + CLI @s2script/* wildcard external

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: Migrate every consumer + regenerate codegen + delete `packages/std`

**Files:**
- Modify: `games/cs2/js/pawn.js`; `packages/cli/src/schemagen/emit-dts.ts`; `packages/cs2/schema.generated.d.ts` (regenerated); `examples/demo-plugin/src/plugin.ts`, `examples/schema-dump/src/plugin.ts`, `examples/greeter-plugin/src/plugin.ts`, `examples/greeter-consumer/src/plugin.ts`, `examples/entref-producer/src/plugin.ts`, `examples/entref-consumer/src/plugin.ts`, `examples/entref-consumer/src/entref-iface.d.ts`; `packages/cli/test/fixtures/producer/src/plugin.ts` (+ `packages/cli/test/fixtures/hello/src/plugin.ts` if it imports `@s2script/std`); `packages/cli/test/build.test.mjs` (assertions); `CLAUDE.md`
- Delete: `packages/std/`

**Interfaces:**
- Consumes: the Task-2 module packages; the Task-1 runtime resolution.

- [ ] **Step 1: Migrate `pawn.js`.** In `games/cs2/js/pawn.js`, `var EntityRef = __s2require("@s2script/std").EntityRef;` → `var EntityRef = __s2require("@s2script/entity").EntityRef;`.

- [ ] **Step 2: Migrate the codegen + regenerate.** In `packages/cli/src/schemagen/emit-dts.ts`, the header array's `'import type { EntityRef } from "@s2script/std";'` → `'import type { EntityRef } from "@s2script/entity";'`. Then regenerate the committed generated `.d.ts`:

```bash
cd /home/gkh/projects/s2script/packages/cli && node build.mjs
cd /home/gkh/projects/s2script && node packages/cli/dist/cli.js gen-schema
bash scripts/check-schema-generated.sh   # PASS (regenerated .d.ts now imports @s2script/entity)
```

- [ ] **Step 3: Migrate the example plugins + fixtures** (each import repointed to the specific module):
  - `OnGameFrame` importers → `@s2script/frame`: `examples/demo-plugin/src/plugin.ts`, `examples/schema-dump/src/plugin.ts`, `examples/greeter-consumer/src/plugin.ts`.
  - `publishInterface` importers → `@s2script/interfaces`: `examples/entref-producer/src/plugin.ts`, `packages/cli/test/fixtures/producer/src/plugin.ts`.
  - `EntityRef` importer → `@s2script/entity`: `examples/entref-consumer/src/entref-iface.d.ts` (`import { EntityRef } from "@s2script/std"` → `"@s2script/entity"`); check `examples/entref-consumer/src/plugin.ts`'s `OnGameFrame` import → `@s2script/frame`.
  - `examples/greeter-plugin/src/plugin.ts` — SPLIT the combined import into two lines:
    ```ts
    import { OnGameFrame } from "@s2script/frame";
    import { publishInterface, PublishHandle } from "@s2script/interfaces";
    ```
  - `packages/cli/test/fixtures/hello/src/plugin.ts` — if it imports `@s2script/std`, repoint (likely `OnGameFrame` → `@s2script/frame`).

- [ ] **Step 4: Update the CLI build-test assertions.** In `packages/cli/test/build.test.mjs`, the assertions that a built fixture's `plugin.js` contains `require("@s2script/std")` now expect the migrated module name (e.g. the `hello` fixture → `require("@s2script/frame")`; the `producer` fixture → `require("@s2script/interfaces")`). Update each `assert` string to the module the fixture now imports.

- [ ] **Step 5: Delete `packages/std` + update CLAUDE.md.**

```bash
git rm -r packages/std
```
In `CLAUDE.md`, update the "npm scope taxonomy" convention line: the engine-generic std lib is now the per-capability `@s2script/<module>` packages (`@s2script/entity`/`frame`/`timers`/`console`/`interfaces`), not a single `@s2script/std`. (Do NOT alter the standing conventions structure otherwise.)

- [ ] **Step 6: Grep the repo clean + verify**

```bash
grep -rn "@s2script/std" --include=*.ts --include=*.js --include=*.mjs --include=*.d.ts --include=*.rs --include=*.json . | grep -v node_modules
# Expected: ZERO hits (only historical mentions in docs/superpowers specs/plans are acceptable — exclude docs).
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
All green; grep clean (docs excepted).

- [ ] **Step 7: Commit** (do NOT commit `dist/` build artifacts):

```bash
git add games/cs2/js/pawn.js packages/cli/src/schemagen/emit-dts.ts packages/cs2/schema.generated.d.ts \
        examples packages/cli/test/fixtures packages/cli/test/build.test.mjs CLAUDE.md
git add -A packages/std
git commit -m "feat(slice5c1): migrate all consumers off @s2script/std to the module packages; retire packages/std

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: Live gate (Docker CS2) + README + CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `README.md`, `CLAUDE.md` (Current state)

**Interfaces:**
- Consumes: the migrated demo (imports from the new modules); the generated `Pawn` accessors.

**Needs ONE sniper rebuild** — the `s2require` core change (Task 1) alters the runtime.

- [ ] **Step 1: Sniper build + package.** From the repo root:

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
node packages/cli/dist/cli.js build examples/demo-plugin      # the demo now imports @s2script/frame + @s2script/cs2
```
`build-sniper.sh` runs `package-addon.sh` (concatenating `schema.generated.js` + `pawn.js` into the addon). Confirm `s2script.so` needs GLIBC ≤ 2.31.

- [ ] **Step 2: Run the live gate on Docker CS2.** Drop the demo `.s2sp` into the mounted `plugins/`; restart the container to load the freshly-built `.so` + injected JS. Arm: `python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1"`; wait past the boot window. Expect:
  - The demo (importing `OnGameFrame` from `@s2script/frame`, `Pawn` from `@s2script/cs2`) loads and logs `[demo] tick … health=100 friction=…` — i.e. `require("@s2script/frame")` and the module resolution work live, and a generated `Pawn` accessor reads.
  - `bot_kick` → the reads go `null`, server keeps ticking, no crash.
  Capture the log. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with the exact commands/errors.

- [ ] **Step 3: README + CLAUDE.**
  - `README.md`: add a short `## Module packages (Slice 5C.1)` note — the engine-generic std lib is now per-capability packages (`@s2script/entity`/`frame`/`timers`/`console`/`interfaces`); show an example import; note `@s2script/std` is retired; include the captured live-gate log.
  - `CLAUDE.md` "## Current state": Slice 5C.1 done (the module split — `@s2script/std` dissolved into the five `@s2script/<module>` packages via a generalized `s2require` rule; all consumers migrated); "Current focus: Slice 5C.2 next" (the player model — controller-as-client + `.pawn` + iteration). Do NOT alter the standing conventions above it.

- [ ] **Step 4: Final verification + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add README.md CLAUDE.md
git commit -m "feat(slice5c1): live gate PASSED — modules resolve live; README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §9)

1. `cargo test -p s2script-core` green (repointed + new `s2require` tests); the CLI `node:test` suite green; both boundary gates green; `check-schema-generated.sh` green (regenerated `.d.ts`).
2. `s2script build` externalizes `@s2script/*` module imports correctly.
3. Live gate: a plugin importing from the new module packages loads + reads a field, no crash.
4. No `@s2script/std` reference remains in code (grep-clean; docs excepted); README + CLAUDE updated.
