# Slice 5C.1 — The Module Split

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5c1-module-split` (off `main`, which has Slices 0–5A + entref-wire + 5B merged).
**Parent:** Slice 5C (std breadth + module split + player model). 5C.1 is the **module split**, done first so the
package taxonomy is locked before breadth accretes; 5C.2 = the player model; 5C.3+ = std breadth.

---

## 1. Goal

Dissolve the monolithic engine-generic `@s2script/std` package into first-class **per-capability module
packages** under the reserved `@s2script/*` scope, so plugin authors write
`import { EntityRef } from "@s2script/entity"` (SourceMod-include feel — each capability is its own thing).
Do it now — while std is ~9 exports and ~7 consumers — because establishing the taxonomy is trivial today
and a painful retrofit once breadth (player model, timers, events, vector, string) has accreted. `@s2script/std`
is **retired** (no umbrella): one unambiguous import path per capability.

## 2. What exists (merged)

- **`@s2script/std`** — the one engine-generic package. **Runtime:** a core-embedded JS prelude (a Rust
  string literal in `core/src/v8host.rs`) evaluated per context, which builds a `std` object and stashes it
  at `globalThis.__s2pkg_std`. **Types:** `packages/std/index.d.ts` (types-only; `package.json` `"types"`).
  Exports: `EntityRef` (+ typed read/write/`readHandle`), `OnGameFrame` + `SubscribeOptions`, `delay`/
  `nextTick`/`nextFrame`/`threadSleep`, `console`, `publishInterface` + `PublishHandle`, and the
  `HookResult`/`Priority`/`Phase` frame-hook enums.
- **`s2require` native** (`core/src/v8host.rs`) — the injected-package resolver: a hardcoded
  `match name { "@s2script/std" => "__s2pkg_std", "@s2script/cs2" => "__s2pkg_cs2", _ => return }`, then
  returns `globalThis[key]`. **Inter-plugin `require`** (`@demo/greeter` → producer proxy) is a SEPARATE JS
  path (not `s2require`), so `s2require` handles ONLY first-party injected packages.
- **The CLI** (`packages/cli/src/build.ts`) — esbuild marks `@s2script/std`, `@s2script/cs2`, + inter-plugin
  deps as `external`, so `import … from "@s2script/std"` stays as a runtime `require("@s2script/std")`.
- **`@s2script/cs2`** — the game package (Pawn + 5B.3 generated accessors); its runtime prelude sets
  `globalThis.__s2pkg_cs2`, and `pawn.js` resolves `EntityRef` via `__s2require("@s2script/std").EntityRef`.

## 3. Decisions locked during brainstorming

1. **Top-level module packages, not `@s2script/std/*` subpaths.** `@s2script/entity`, `@s2script/timers`,
   etc. — siblings of `@s2script/cs2`. No subpath-resolution machinery.
2. **Retire `@s2script/std`** — no umbrella/barrel. One import path per capability. (A convenience umbrella is
   additive + trivial to add later if the base-plugin suite wants one — YAGNI, don't ship it now; if ever
   added, name it for the framework, not "std".)
3. **The taxonomy (5 modules)** — see §5.
4. **Generalize `s2require`** — any `@s2script/<name>` → `globalThis.__s2pkg_<name>` (one rule; core never
   hardcodes the module list; an unknown/retired name → `null`). This subsumes the existing `cs2` mapping.
5. **CLI externalizes `@s2script/*`** by wildcard (all first-party module imports stay external).
6. **Each module is a types-only package** (`packages/<mod>/{package.json,index.d.ts}`, split from today's
   `packages/std/index.d.ts`). The single core-embedded prelude sets all the `__s2pkg_<module>` globals.

## 4. Architecture — three thin layers

- **Core (runtime resolution + prelude).** `s2require` is generalized: if the specifier starts with
  `@s2script/`, strip that prefix and look up `globalThis["__s2pkg_" + rest]` (so `@s2script/entity` →
  `__s2pkg_entity`, `@s2script/cs2` → `__s2pkg_cs2`); anything else → `null` (unchanged — inter-plugin deps
  are resolved on the separate JS path). The core-embedded engine-generic prelude is reorganized to build
  the five module objects and set `globalThis.__s2pkg_entity/frame/timers/console/interfaces` — and to NO
  LONGER set `__s2pkg_std`. (The prelude stays one injected source; only its output globals change. The
  `HookResult`/`Priority`/`Phase` values already live as raw-context runtime globals with NO current `.d.ts`
  and are NOT `@s2script/std` exports — unaffected by this split; typing them into `@s2script/frame` is future
  frame-breadth, out of scope here.)
- **Build (CLI).** `packages/cli/src/build.ts` replaces the explicit `@s2script/std`/`@s2script/cs2` external
  entries with the `@s2script/*` wildcard (esbuild supports `*` in `external`), so every first-party module
  import externalizes to a `require("@s2script/<mod>")` in the bundled plugin. Inter-plugin external entries
  (the plugin's declared deps) are unchanged.
- **Types (packages).** `packages/std/` is replaced by `packages/entity/`, `packages/frame/`,
  `packages/timers/`, `packages/console/`, `packages/interfaces/` — each a types-only package
  (`{ "name": "@s2script/<mod>", "types": "index.d.ts" }`) whose `index.d.ts` holds that module's slice of
  today's `packages/std/index.d.ts`. `@s2script/frame`'s `.d.ts` imports `EntityRef` from `@s2script/entity`
  only if it references it (it doesn't today).

## 5. The taxonomy — module ← today's exports

| Package | Gets (from today's `@s2script/std`) |
|---|---|
| `@s2script/entity` | `EntityRef` + all its typed read/write/`readHandle` methods |
| `@s2script/frame` | `OnGameFrame`, `SubscribeOptions` (`HookResult`/`Priority`/`Phase` stay untyped raw-context globals — future frame-breadth) |
| `@s2script/timers` | `delay`, `nextTick`, `nextFrame`, `threadSleep` |
| `@s2script/console` | `console` |
| `@s2script/interfaces` | `publishInterface`, `PublishHandle` |

Every current export lands in exactly one module (no export dropped, none duplicated). Future breadth slots
into new or existing modules (`vector`, `string`, `math`, `events`, `commands`, `convars`) — not built here.

## 6. Migration (all consumers of `@s2script/std`, repointed now)

- **`games/cs2/js/pawn.js`** — `__s2require("@s2script/std").EntityRef` → `__s2require("@s2script/entity").EntityRef`.
- **The core-embedded prelude(s)** in `core/src/v8host.rs` — set the module globals; drop `__s2pkg_std`; and
  any in-isolate test that does `__s2require("@s2script/std")` (e.g. the `OnGameFrame`/`delay` destructure
  test) → the specific module(s).
- **`packages/cli/src/schemagen/emit-dts.ts`** — it emits `import type { EntityRef } from "@s2script/std";`
  into the GENERATED `.d.ts`; change to `@s2script/entity`, then **regenerate** the committed
  `packages/cs2/schema.generated.d.ts` (the `check-schema-generated.sh` freshness gate enforces this).
- **The example plugins** — `examples/{demo-plugin,greeter-plugin,greeter-consumer,entref-producer,entref-consumer}`
  (+ `entref-consumer/src/entref-iface.d.ts`) — repoint each `@s2script/std` import to the specific module
  (`OnGameFrame`→`@s2script/frame`, `EntityRef`→`@s2script/entity`, `publishInterface`→`@s2script/interfaces`,
  `delay`→`@s2script/timers`).
- **`packages/cli/test/fixtures/producer/src/plugin.ts`** — repoint (used by the CLI build test).
- **`packages/cli/src/build.ts`** — the external list → the `@s2script/*` wildcard (mechanism, not a consumer).
- **Delete `packages/std/`** and the `@s2script/std` name everywhere.
- **`CLAUDE.md`** — update the "npm scope taxonomy" convention line: the engine-generic std lib is now the
  per-capability `@s2script/<module>` packages, not a single `@s2script/std`.

## 7. Data flow (require resolution, after)

`import { EntityRef } from "@s2script/entity"` → esbuild (external `@s2script/*`) → bundled
`require("@s2script/entity")` → the CJS require shim → `__s2require("@s2script/entity")` → `s2require`
strips `@s2script/` → `globalThis.__s2pkg_entity` (set by the injected prelude) → the entity module object.
`require("@s2script/std")` → `s2require` → `globalThis.__s2pkg_std` (never set) → `undefined` → `null`.

## 8. Error handling / degrade

Unchanged posture. An unknown or retired `@s2script/<name>` → the global is undefined → `s2require` returns
`null` (a hard-dep `require` throwing `InterfaceUnavailable` is the separate inter-plugin path, not this).
`s2require` stays `catch_unwind`-wrapped. A subpath specifier (`@s2script/a/b`, not used) degrades to `null`
(no such global), not a crash.

## 9. Testing & acceptance

**In-isolate (`frame_tests`, core):**
- `s2require` generalization: in a plugin context, `require("@s2script/entity").EntityRef` is a function;
  `require("@s2script/frame").OnGameFrame` is present; `require("@s2script/timers").delay` is a function;
  `require("@s2script/std")` is `null` (retired); `require("@s2script/nope")` is `null`.
- The existing 5A/4.5 in-isolate tests that used `@s2script/std` are repointed to the module packages and
  still pass (EntityRef degrade, publishInterface, the registered-package-reaches-EntityRef guard).

**Build (CLI, node:test):** a fixture plugin importing from a module (e.g. `@s2script/entity`) builds and the
bundled `plugin.js` keeps it external (`require("@s2script/entity")`), not bundled. The freshness gate
(`check-schema-generated.sh`) passes after the `emit-dts.ts` repoint + regenerate.

**Live-only (Docker CS2):** the demo (repointed to `@s2script/frame` + `@s2script/cs2`) loads and reads a
generated field live; `require("@s2script/entity")` resolves in the running server. Needs ONE sniper rebuild
(the `s2require` core change).

**Acceptance:**
1. `cargo test -p s2script-core` green (repointed + new `s2require` tests); the CLI `node:test` suite green;
   both boundary gates green; `check-schema-generated.sh` green (regenerated `.d.ts`).
2. `s2script build` externalizes `@s2script/*` module imports correctly.
3. Live gate: a plugin importing from the new module packages loads + reads a field, no crash.
4. No `@s2script/std` reference remains in the repo (grep-clean); README + CLAUDE updated.

## 10. File structure

- **Modify** `core/src/v8host.rs` — generalize `s2require`; reorganize the embedded prelude to set the
  module globals (drop `__s2pkg_std`); repoint the in-isolate tests.
- **Create** `packages/{entity,frame,timers,console,interfaces}/{package.json,index.d.ts}` (split from
  `packages/std/index.d.ts`). **Delete** `packages/std/`.
- **Modify** `packages/cli/src/build.ts` (wildcard external), `packages/cli/src/schemagen/emit-dts.ts`
  (`@s2script/entity`), `packages/cli/test/fixtures/producer/src/plugin.ts`.
- **Regenerate** `packages/cs2/schema.generated.d.ts` (via `s2script gen-schema`).
- **Modify** `games/cs2/js/pawn.js`, `examples/*/src/*.ts` + `entref-consumer/src/entref-iface.d.ts`,
  `README.md`, `CLAUDE.md`.

Core stays engine-generic (the module names + prelude are engine-generic; the generalized `s2require` rule
knows no specific module). Both boundary gates stay green.

## 11. Scope & deferrals

**Scope:** the split mechanism (generalized `s2require` + wildcard external + per-module types packages),
the 5-module taxonomy, retiring `@s2script/std`, migrating all consumers, the live gate.

**Deferred — do NOT build:** any NEW std breadth (Vector value type, timers/events/string/math/commands/convars
surface — 5C.3+); the player model (`@s2script/cs2` players — 5C.2); the `@s2script/cs2` game-package internal
split (this slice splits `@s2script/std` only; cs2 stays one package, though the generalized `s2require` makes
cs2 subpackages *possible* for 5C.2); a convenience umbrella package; the `tsc` typecheck gate; config/permissions;
the registry (5.5); the base-plugin suite (6). The 5B.3 post-merge codegen TODOs (global-vs-per-chain collision
scoping; `enumerable:false`) are NOT part of this slice.

## 12. Global constraints (bind every task)

- **Core stays engine-generic.** The generalized `s2require` + the module prelude know no game and no specific
  module list — they apply a `@s2script/<name>` → `__s2pkg_<name>` rule. No CS2 identifiers in `core/src`.
  Both gates green (`check-core-boundary.sh`, `test-boundary-nameleak.sh`).
- **Back-compat within the migration.** After the split, every prior capability is still reachable — from its
  new module. The injected preludes + `pawn.js` must be repointed IN THE SAME slice so nothing breaks at load.
- **Deterministic codegen stays green.** The `emit-dts.ts` change regenerates `schema.generated.d.ts`; the
  freshness gate must pass (regenerate, don't hand-edit).
- **Naming:** package names lowercase (`@s2script/entity`); PascalCase types, camelCase fns/props unchanged.
- **cdylib:** core unit/in-isolate tests inline in `#[cfg(test)] mod`.
- **Commit trailer** on every commit; commit only on `slice-5c1-module-split`; do NOT push.
