# SourceMod-shaped TypeScript authoring — implementation plan

> **For agentic workers:** GitHub-native stacked PRs (not Graphite). Each PR is independently mergeable and gate-green. Child PR `base_branch` = parent branch.

**Goal:** Land the SourceMod-shaped TypeScript authoring paradigm on `s2script/s2script` main (HUD / `ctx.ui` already present). Four PRs; ESM deferred.

**Architecture:** Free load-window APIs bind to the current factory/`OnPluginStart` ctx via `globalThis.__s2_load_ctx` in `core/js/prelude.js`. Types live on existing `@s2script/sdk/commands` and `@s2script/sdk/plugin` subpaths. Host publics are named exports found on `module.exports`. esbuild stays CJS / `platform: "neutral"`.

**Tech stack:** TypeScript `.d.ts` + `packages/sdk` CLI, `core/js/prelude.js` + `core/src/v8host.rs` load path, cargo isolate tests, node `--test` SDK tests.

## Global Constraints

- Origin is `github.com/s2script/s2script`. Do not push to `GabeHirakawa/s2script`.
- Do not invent a new language. Do not revive the `.s2s` compiler.
- Do not clobber HUD / `ctx.ui`: keep the `__s2pkg_game_ctx` merge last in `__s2_make_ctx`.
- `plugin((ctx) => …)` stays valid. Do not rewrite the plugin suite except `basecommands` in PR 4.
- Keep esbuild; `format: "cjs"`; `platform: "neutral"`. ESM is later.
- Command handler `HookResult` is a JS return only; do not implement engine SUPERCEDE-on-Continue.
- Chat `!` vs `/` suppress rules unchanged (`dispatch_chat` silent trigger).
- Inter-plugin stays producer-as-import (B) + `use()` for optional / explicit load-window form.
- CLI is `packages/sdk` (`s2s` bin), not `packages/cli`.
- Isolate tests: `cargo +stable test -p s2script-core <filter>` — do not pass `--exact` unless using the full rustc path; do not pass `--test-threads`.
- SDK tests: `cd packages/sdk && npm test`. Do not “fix” pre-existing schema-runtime / player-identity Weapon failures.
- `bash scripts/check-plugins-typecheck.sh` per PR.
- No live CS2 in the cloud VM.
- Changesets: `@s2script/sdk` minor (PR 1, PR 2), patch (PR 3). PR 4 is a private plugin — no changeset.

---

### Task 1: PR 1 — ctx-free load-window APIs

**Files:**
- Modify: `packages/sdk/commands.d.ts`, `packages/sdk/plugin.d.ts`
- Modify: `core/js/prelude.js` (`__s2_load_ctx`, `command` / `hook` / `publish` / `use` / `tryUse`, command wrappers return handler result)
- Modify: `packages/sdk/src/publish-scan.ts`, `packages/sdk/src/build.ts`
- Modify: `core/src/v8host.rs` (`command_api_registers_during_factory_and_throws_after`)
- Create: `packages/sdk/test/fixtures/authoring-command/`, `packages/sdk/test/fixtures/consumer-import/`
- Modify: `packages/sdk/test/publish-scan.test.mjs`, `packages/sdk/test/build.test.mjs`
- Create: `.changeset/authoring-load-window.md`
- Create: spec + plan under `docs/superpowers/{specs,plans}/`

**Interfaces:**
- Produces: `CommandHandler`, callable `command` + `.admin` / `.server`; `hook.damage`; free `publish` / `use` / `tryUse`; `PublishScan.importNames`

- [ ] Types on `commands.d.ts` / `plugin.d.ts`; `CtxCommands.register*` handlers become `CommandHandler`
- [ ] Prelude: set/clear `__s2_load_ctx` around the factory; bind free APIs; command wrappers `return handler(...)`
- [ ] `publish-scan`: free `publish`/`use`/`tryUse` (symbol from `plugin.d.ts`) + `importNames`
- [ ] `build.ts` advisories: `useNames ∪ importNames`
- [ ] Fixtures + SDK tests + isolate test
- [ ] `s2s create` still scaffolds `plugin()` (PR 2 switches it)
- [ ] Verify: `cd packages/sdk && npm test` (ignore pre-existing Weapon failures); `cargo +stable test -p s2script-core command_api_registers`; `bash scripts/check-plugins-typecheck.sh`

---

### Task 2: PR 2 — OnPluginStart publics (base = PR 1)

**Files:**
- Modify: `core/src/v8host.rs` `load_plugin_js` + `__s2_run_factory(def, exports)`
- Modify: `core/js/prelude.js` (factory then publics; `topmenu` / `translations` on `__s2pkg_plugin`)
- Modify: `packages/sdk/plugin.d.ts` (`topmenu`, `translations`)
- Modify: `packages/sdk/src/create/create.ts` `pluginSource`
- Modify: `packages/sdk/test/create-resolve.test.mjs` generic-scaffold assertion
- Create: `packages/sdk/test/fixtures/authoring-publics/`
- Modify: `scripts/sync-phrase-types.mjs` so `translations.load(...)` (identifier) is collected, not only `ctx.translations.load`
- Isolate tests: `on_plugin_start_public_is_a_valid_artifact`, `missing_plugin_and_on_plugin_start_refused`
- Changeset: `@s2script/sdk` minor

**Load rules:**
- Accept `plugin()` OR `export function OnPluginStart` (or both: factory first, then publics)
- Missing both → refuse naming `OnPluginStart`
- Legacy `onLoad` still refused
- Order: factory → subscribe `OnGameFrame`/`OnMapStart` → `OnPluginStart()` → `OnPluginEnd` → `hooks.onUnload`

**Create scaffolds:**
- CS2: `OnPluginStart` + `command("hello", …)` — no `export default plugin`
- generic: `OnPluginStart` + `OnGameFrame` + `void delay(...)`

---

### Task 3: PR 3 — ES2024 target (base = PR 2)

**Files:**
- Modify: `packages/sdk/src/tsconfig-shared.ts` `target`/`lib` ES2024; `sharedProgramOptions` `ScriptTarget.ES2024`
- Modify: `tsconfig.base.json` (parity test must stay green)
- Modify: `packages/sdk/src/build.ts` esbuild `target: "es2024"`
- Modify: `packages/sdk/test/fixtures/authoring-publics/` to use `Object.groupBy` as lib proof
- Do **not** change `packages/sdk/tsconfig.json` or historical specs that mention es2020
- Changeset: `@s2script/sdk` patch

---

### Task 4: PR 4 — dogfood basecommands (base = PR 3)

**Files:**
- Modify: `plugins/basecommands/src/plugin.ts` only

Rewrite to `export function OnPluginStart` + `command.admin` / `command("sm", …)` + `hook.damage` + `topmenu.addItem` + `translations.load`. Extract named local command functions. Behavior identical: kick/map/who/rcon/exec/cvar/sm/damage-halve/admin diag/map menu. Typecheck + `s2s build` the plugin. Do not rewrite other plugins.
