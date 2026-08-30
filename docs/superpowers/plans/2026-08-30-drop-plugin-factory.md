# Drop `plugin()` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Remove `plugin()` from the public TypeScript authoring surface and fill remaining ctx-only gaps so every in-repo plugin/example/tool/fixture authors with `OnPluginStart` + free APIs.

**Architecture:** Prelude keeps an internal load ctx. New `hook.*` / `previous()` / `pluginId()` / `command.onClientCommand` bind through `__s2_load_ctx`. CS2 namespaces (`ui`, `gameRules`, `players`, `items`) are load-window proxies on `__s2pkg_cs2`. Host subscribes `OnPluginState`. Public `plugin()` / `PluginFactory` / `PluginDefinition` are deleted from `plugin.d.ts`. Isolate tests may still inject `{ __s2plugin: 1, factory }`.

**Tech Stack:** prelude.js (ES5), packages/sdk + packages/cs2 `.d.ts`, Rust isolate tests (`cargo +stable`), Node SDK/eslint tests, in-repo TypeScript plugins.

## Global Constraints

- 0.x **minor** changesets (`@s2script/sdk`, `@s2script/cs2`, `@s2script/eslint-plugin` as needed).
- `Player` stays on `@s2script/cs2`. `@s2script/sdk/unsafe` stays a subpath.
- Do not rustfmt all of `core/src/v8host.rs`. Isolate tests: `cargo +stable test -p s2script-core <substring>` — no `|` filter, no `--test-threads`.
- Do not “fix” pre-existing `packages/sdk && npm test` Weapon failures.
- Stacked PR base: `cursor/plugins-publics-cutover-a8c9`. Branch: `cursor/drop-plugin-factory-a8c9`.
- Live-gate tool behavior (log prefixes, command names, hook collapse) stays identical.

---

### Task 1: Prelude free APIs + OnPluginState + game-ns proxy

**Files:**
- Modify: `core/js/prelude.js` (hook object, command, previous/pluginId, OnPluginState, `__s2_game_ns`)
- Modify: `games/cs2/js/pawn.js` (attach gameRules/players/items proxies)
- Modify: `games/cs2/js/ui.js` (attach ui proxy)
- Modify: `core/src/v8host.rs` (loader refuse copy; isolate tests near existing OnPluginStart tests)

**Produces:**
- `hook.create/spawn/delete/precache/gameFrame/mapStart/runcmd/connect/putInServer/active/fullyConnect/disconnect/settingsChanged/voice/cookiesCached/say`
- `command.onClientCommand(name, handler)`
- `previous()`, `pluginId()`
- `globalThis.__s2_game_ns(name)` → Proxy onto `__s2_load_ctx[name]`
- `OnPluginState` → `hooks.state`

- [x] Expand `hook` and bind `previous` / `pluginId` / `command.onClientCommand` in prelude.js next to the existing free APIs. Wire `OnPluginState` after `OnPluginEnd` wrapping. Define `__s2_game_ns` using the existing Proxy pattern from `handleFor`.

- [x] After `__s2pkg_cs2` / `__s2pkg_game_ctx` are set in pawn.js, assign `gameRules`/`players`/`items` from `__s2_game_ns`. After ui.js assigns the ui factory, assign `__s2pkg_cs2.ui`.

- [x] Update the missing-artifact refuse string to name `OnPluginStart` only. Add isolate tests: `OnPluginState` + `previous()` roundtrip; `hook.create` + `hook.gameFrame` register during OnPluginStart and throw after settle.

- [x] Commit prelude + CS2 JS + isolate tests.

### Task 2: Public types — drop `plugin()`, add free APIs

**Files:**
- Modify: `packages/sdk/plugin.d.ts`, `packages/sdk/commands.d.ts`
- Modify: `packages/sdk/src/hookgen/emit-dts.ts`, `packages/cs2/hooks.generated.d.ts`
- Modify: `packages/cs2/ui.d.ts`, `packages/cs2/items.d.ts`, `packages/cs2/index.d.ts`
- Modify: `packages/sdk/test/hookgen.test.mjs`
- Modify: eslint `plugin-factory.ts` (`findOnPluginStart`) + `no-floating-promise-in-factory.ts`
- Create: `.changeset/drop-plugin-factory.md`

**Produces:**
- No `plugin` / `PluginFactory` / `PluginDefinition` in public `.d.ts`
- `export declare const gameRules` / `players` from hookgen; `ui` / `items` hand-written
- Changeset: sdk **minor**, cs2 **minor**, eslint-plugin **patch**

- [x] Delete `plugin()`, `PluginFactory`, `PluginDefinition`. Document `OnPluginStart` as the artifact. Expand `hook` and add `previous` / `pluginId`. Add `command.onClientCommand`.

- [x] hookgen: after each `Ctx*` interface, emit `export declare const ${ns}: ${ctxIface};`. Keep PluginContext augmentation. Update hookgen tests. Hand-write `export declare const ui: CtxUi` and `items: CtxItems`; re-export from `cs2/index.d.ts`.

- [x] `findOnPluginStart`: `export function OnPluginStart`. `no-floating-promise-in-factory` uses factory **or** OnPluginStart.

- [x] Changeset + commit types.

### Task 3: Migrate in-repo TS off `plugin()`

**Files:**
- All `examples/**/src/plugin.ts`, `examples/cookbook/src/**`, `tools/**/src/plugin.ts`
- `packages/sdk/test/fixtures/**/plugin.ts` (except eslint stub `plugin()` tests)
- SDK tests that write `export default plugin(...)` source strings
- `packages/sdk/test/fixtures/typecheck/game-ctx-only/src/plugin.ts`

**Produces:** every in-repo authoring file uses `OnPluginStart` (and publics / free APIs). Cookbook `Recipe.register(): void`.

- [x] Mechanical mapping in Task 1’s table. hud-lab: `ui` from `@s2script/cs2`; `DemoHud` calls `hook.gameFrame`; `bomb.install` uses `hook.event`. hello-plugin: `previous()` + `OnPluginState` + `OnPluginEnd`.

- [x] Fixtures that only need a valid artifact: `export function OnPluginStart() {}` plus `publish`/`command`/`use`/`tryUse` as they did via ctx.

- [x] game-ctx-only: keep `PluginContext` only (no named `@s2script/cs2` import) so `gamePackageDeclarationFiles` still proves the hooks augmentation.

- [x] Commit examples, tools, cookbook, fixtures.

### Task 4: Gates

- [x] `bash scripts/check-plugins-typecheck.sh`
- [x] `bash scripts/check-examples-coverage.sh`
- [x] `cd packages/sdk && npm test` (ignore pre-existing Weapon failures)
- [x] `cd packages/eslint-plugin && npm test` (or the repo’s eslint test command)
- [x] `cargo +stable test -p s2script-core previous` and `on_plugin_start` / `sdk_barrel` / `sm_publics` substrings
- [x] Push, open draft PR against `cursor/plugins-publics-cutover-a8c9`
