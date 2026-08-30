# SourceMod-shaped TypeScript authoring — design spec

**Status:** per-slice design (final surface). Lands as four GitHub-native stacked PRs on `s2script/s2script` main.
**Date:** 2026-08-30.
**Scope:** keep TypeScript as the language; give plugins SourceMod's *registration discipline* (`OnPluginStart`, named publics, `RegAdminCmd`-shaped `command.admin`) without inventing a new language or reviving the abandoned `.s2s` compiler.
**Verified against:** org `main` @ `50d19d5` (HUD / `ctx.ui` already landed in `@s2script/cs2`; CLI lives in `packages/sdk`).

---

## Locked product decisions

1. **Language is TypeScript (`.ts`).** Not `.s2s`. Not SourcePawn highlighting.
2. **`export` is only SM-style publics the host finds by name.** Commands are NOT exports.
3. **Commands register in the load window:** `command.admin("sm_kick", flags, kickFn)`. Name the function anything. Same as `RegAdminCmd`.
4. **Command handlers return `HookResult` (`Continue` / `Changed` / `Handled` / `Stop`).** Omit return = `void` allowed. Chat `!` vs `/` suppress rules unchanged. Engine SUPERCEDE-on-Continue is **not** this slice.
5. **File scope is not live.** `import` / `const` / `function` defs only. Live work is `OnPluginStart` (or the existing `plugin()` factory).
6. **`AskPluginLoad2` = `package.json`** (`pluginDependencies` / `publishes` / derived `apiVersion`).
7. **Inter-plugin stays producer-as-import (B):** `s2s add` → `.s2script/types/<pkg>/`, esbuild `external`, `__s2_require` → `makeIfaceProxy`. `import { greet } from "@demo/greeter"` is the contract. `use()` stays for optional deps / the explicit load-window form. Do not invent a second interop story.
8. **Keep esbuild.** One `plugin.js`, externals, `platform: "neutral"`.
9. **`plugin((ctx) => …)` and `ctx.*` stay valid** until a later cutover. Do not rewrite the whole plugin suite in this stack.
10. **GitHub-native stacked PRs.** Not Graphite / not `gt`. Each PR must pass gates alone. Child PR `base_branch` = parent branch.
11. **Dogfood last:** rewrite `plugins/basecommands` only after the surface exists.
12. **ESM load (`esbuild format:"esm"` + `v8::Module`) is a later knob.** Do not block “API complete” on it. If done later, the resolver must still map plugin package names to the host proxy.

## Explicit non-goals

- ESM host rewrite.
- Engine SUPERCEDE-on-Continue for ConCommands.
- Repo-wide plugin rewrite (only `basecommands` dogfoods).
- Anything on `GabeHirakawa/s2script`.
- Live CS2 in the cloud VM.

---

## Target authoring shape

```ts
import { command } from "@s2script/sdk/commands";
import { hook, topmenu } from "@s2script/sdk/plugin";
import { ADMFLAG } from "@s2script/sdk/admin";

export function OnPluginStart(): void {
  command.admin("sm_kick", ADMFLAG.KICK, kick);
  hook.damage(halve);
}

function kick(cmd: CommandInvocation): HookResultValue | void { /* … */ }
function halve(info: DamageInfo): void { info.damage = info.damage / 2; }
```

Until PR 2, `export default plugin((ctx) => { … })` remains **required**. The free functions bind to the factory's load ctx. PR 2 accepts `export function OnPluginStart` as an alternative (or in addition).

---

## Architecture (org main)

The CLI is `@s2script/sdk` (`s2s` bin in `packages/sdk`). There is no `packages/cli`.

The engine prelude is **`core/js/prelude.js`**, baked into the isolate by `include_str!` in `core/src/v8host.rs` (`INJECTED_STD_PRELUDE`). Do **not** inline a new prelude string in Rust. HUD / `ctx.ui` lives in `games/cs2/js/ui.js` and is merged onto ctx via `__s2pkg_game_ctx` **after** built-in ctx members — that merge must stay last and untouched.

`__s2require("@s2script/sdk/<cap>")` already maps to `globalThis.__s2pkg_<cap>`. Adding named exports (`command` on `__s2pkg_commands`, `hook` / `publish` / `use` / `tryUse` / later `topmenu` / `translations` on `__s2pkg_plugin`) is how `import { command } from "@s2script/sdk/commands"` resolves at runtime. esbuild `format: "cjs"` + `external: ["@s2script/*", …deps]` emits `require(...)`; the CJS wrapper binds `require = globalThis.__s2_require`.

### Load-window binding

There is no native `__s2_load_ctx`. The prelude sets `globalThis.__s2_load_ctx` to the load-scoped ctx **for the duration of the factory (and, in PR 2, `OnPluginStart`)**. After settle it is cleared to `null`. Free `command` / `hook` throw `"s2script: <api> outside the load window"` when it is null. Buffered `ctxReg` thunks still arm at Active as today.

### Command `HookResult`

`Commands.register` / `registerAdmin` / `registerServer` wrappers **return** the handler's value. `void` is allowed. Rust `dispatch_concommand` continues to ignore the return (no engine SUPERCEDE). Chat `!` vs `/` suppress remains `dispatch_chat`'s silent-trigger rule, unchanged.

### Publish scan

`scanPluginProgram` today only sees `ctx.publish` / `ctx.use` / `ctx.tryUse` (receiver type `PluginContext`). PR 1 also collects:

- free `publish` / `use` / `tryUse` whose identifier aliases a symbol declared in `plugin.d.ts`
- `importNames`: static `import` / `export … from` specifiers that are not `@s2script/*` and not relative

`build.ts` advisories use `useNames ∪ importNames`, so `import { greet } from "@demo/greeter"` satisfies a declared `pluginDependencies` entry (producer-as-import) and does not warn “never `ctx.use()`d”.

### Host publics (PR 2)

`load_plugin_js` accepts `plugin()` **or** `export function OnPluginStart` (or both: factory first, then publics). Missing both → refuse, naming `OnPluginStart`. Legacy `onLoad` still refused.

`__s2_run_factory(def, exports)`:

1. factory if present
2. subscribe exported `OnGameFrame` / `OnMapStart`
3. call `OnPluginStart()`
4. `OnPluginEnd` → `hooks.onUnload`

`s2s create` scaffolds publics only (no `export default plugin`).

### ES2024 (PR 3)

Plugin typecheck + esbuild target move to ES2024 so `Object.groupBy` typechecks. Do **not** change `packages/sdk/tsconfig.json` (CLI compile) or historical specs that mention es2020.

### Dogfood (PR 4)

Rewrite **only** `plugins/basecommands/src/plugin.ts` to the new shape. Behavior identical. Phrase-file discovery (`scripts/sync-phrase-types.mjs`) must still see a `.translations.load(...)` call — PR 2 therefore also ships a load-window `translations` on `@s2script/sdk/plugin` (needed to drop `ctx`, same reason as `topmenu`).

---

## Stack

| PR | Branch (this run) | What |
|----|-------------------|------|
| 1 | `cursor/authoring-lifecycle-apis-a8c9` | ctx-free load-window APIs; `plugin()` still required |
| 2 | `cursor/authoring-publics-a8c9` | `OnPluginStart` publics; `topmenu` / `translations`; `s2s create` |
| 3 | `cursor/es2024-target-a8c9` | ES2024 target/lib + esbuild |
| 4 | `cursor/basecommands-dogfood-a8c9` | rewrite `plugins/basecommands/src/plugin.ts` |
| 5 | `cursor/sm-lifecycle-publics-a8c9` | remaining SM publics; `hook.event` / `hook.output` / `hook.topmenu`; `createScope`; `Command` alias |
