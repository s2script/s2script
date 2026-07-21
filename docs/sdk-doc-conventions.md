# SDK doc conventions (author-facing `.d.ts` stubs)

The `@s2script/sdk/*` and `@s2script/cs2` `.d.ts` stubs ARE the intellisense a
plugin author sees on hover. These conventions keep that experience rich and
consistent. Coverage is enforced per-PR by `scripts/check-doc-coverage.mjs`
(a dev tool, not a CI gate).

## What must be documented
Every **exported** symbol (function, const, class, interface, type, enum) **and
every** interface/class member, const-object method, and enum member gets a
`/** */` block. `import` lines and `export * from` / `export { … }` re-exports
do not.

## Shape of a file
1. Keep the existing `/** @s2script/x — … */` banner as line 1.
2. Blank line.
3. Per-symbol docs. (The analyzer treats the offset-0 banner as the module
   comment — it does NOT satisfy the first symbol, so give that symbol its own.)

## Voice
Terse, factual, present-tense — match `trace.d.ts` / `math.d.ts`. The first
sentence is a self-contained summary (autocomplete shows it before selection).
Never merely restate the type. Preserve every RE/engine caveat already present.

## Tags
- `@param name -` for non-obvious params (skip when the name says it all).
- `@returns` when semantics aren't obvious from the type (e.g. `fetch` resolves —
  not rejects — on 4xx/5xx with `ok=false`).
- `@throws` / rejection semantics for anything that can throw or reject.
- `@example` on **major entry points** (top-level functions + a namespace/object's
  primary methods), drawn from real `plugins/`/`examples/` usage, with accurate
  imports. Not required on trivial members.
- `{@link Symbol}` to cross-reference related types (editors render it clickable).
- `@defaultValue` to standardize the existing "Default X" prose on optional fields.

## Out of scope
The GENERATED cs2 fields (`packages/cs2/*.generated.d.ts`) are not covered here —
they are a separate future effort. "SDK fully documented" means the hand-authored
stubs only.
