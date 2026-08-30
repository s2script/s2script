# `@s2script/sdk` root barrel — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/sm-lifecycle-publics-a8c9`.
**Date:** 2026-08-30.
**Scope:** `import { command, hook, HookResult, … } from "@s2script/sdk"` typechecks and resolves at runtime. Subpath imports stay valid. `Player` stays on `@s2script/cs2`.

---

## Why

The SourceMod-shaped authoring surface is one import, not one import per capability. `s2require` already maps bare `@s2script/sdk` to `globalThis.__s2pkg_sdk` (the `@s2script/` strip); that global was never populated, so the specifier resolved to `null`. Types had no `index.d.ts` either.

## Types

`packages/sdk/index.d.ts` re-exports every engine-generic capability module except:

- `globals` (ambient, not an importable module)
- `unsafe` (deliberate subpath; keep the opt-in)
- `console` (the engine `console` is already global; a named `console` export on the barrel would shadow it)

`Player` / `Pawn` / CS2 schema types are **not** on this barrel. They live on `@s2script/cs2`. Putting them here would make the SDK game-aware.

`package.json` `exports["."]` = `{ "types": "./index.d.ts" }`. `files` already includes `*.d.ts`.

Typecheck paths: `"@s2script/sdk": ["sdk/index.d.ts"]` plus existing `"@s2script/sdk/*"`. `isAlwaysResolved` includes `d === "@s2script/sdk"`. Editor `tsconfig.base.json` gets the same exact path (it already maps `@s2script/*` → `*/index.d.ts`, which would find `sdk/index.d.ts`; the exact entry makes the intent obvious).

## Runtime

After `command` / `hook` / `topmenu` / `translations` are bound onto their per-cap packages, prelude copies those packages' named exports onto `globalThis.__s2pkg_sdk` (first writer wins on a colliding key; `HookResult` on events beats the usercmd duplicate). `__s2require("@s2script/sdk")` then returns that object. esbuild already externalizes `@s2script/*`, so `require("@s2script/sdk")` is host-resolved.

Not copied: `__s2pkg_unsafe`, `__s2pkg_console`, `__s2pkg_frame` (no corresponding type exports on the barrel).

## Coverage gate

`scripts/check-examples-coverage.sh` excludes `index` the same way it excludes `globals`. Cookbook and existing plugins stay on subpaths so every `<cap>.d.ts` still has a consumer. The barrel is an additional spelling, not a replacement of the coverage corpus.

## Scaffold

`s2s create` CS2 / generic templates import from the barrel.

## ESLint

`findFactory` accepts `plugin` imported from `@s2script/sdk` as well as `@s2script/sdk/plugin`, so `no-ctx-escape` still sees `export default plugin((ctx) => …)` during cutover.

## Explicit non-goals

- `Player` on the SDK
- Dropping subpath imports
- Rewriting remaining plugins / examples (follow-up slices)
- Shipping a JS file in the npm package (still types-only; runtime is prelude-injected)
