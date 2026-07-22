# Rich TSDoc for the author-facing SDK stubs — design

**Status:** Approved — ready for planning.
**Audience:** SDK maintainers + every plugin author (the people who import `@s2script/sdk/*` and `@s2script/cs2`).
**Builds on:** the shipped `packages/sdk/*.d.ts` capability stubs (the "includes"), the `@s2script/cs2` hand-authored entry points, and the existing informal doc voice in `trace.d.ts`/`math.d.ts`/`entity.d.ts`.

---

## 1. The problem (uneven intellisense)

The SDK's author-facing surface is a set of **hand-authored `.d.ts` capability stubs**. They *are* the intellisense: when a plugin author hovers `fetch`, `pawn.aimTrace`, or a `Response` member in their editor, what they see is whatever TSDoc lives on that symbol in the stub. Nothing else renders.

Today that coverage is **uneven**:

- **Well-documented** (the target quality): `trace.d.ts`, `math.d.ts`, `entity.d.ts`, `commands.d.ts` — every symbol *and* member carries a terse, factual doc line, RE/engine caveats included.
- **Bare**: `http.d.ts` gives `fetch` a doc but leaves every `Response` member (`status`, `ok`, `statusText`, `headers`, `text()`, `json()`) and every `FetchOptions` field undocumented. `ws.d.ts`, `console.d.ts`, `globals.d.ts`, `net.d.ts`, and many interface members across otherwise-good files are the same — a file-header banner and then bare declarations.

Rough surface size across the in-scope files: **~130 top-level exports + ~480 members ≈ ~600 documentable symbols**, of which perhaps a third are already documented. The rest is the gap.

There is **no doc tooling or convention** in the repo — no TSDoc config, no TypeDoc, no doc-coverage check, no written house style. So the gap has no objective measure and no guardrail against regressing.

## 2. Goal & non-goals

**Goal:** every author-facing exported symbol and member in the hand-authored `.d.ts` stubs carries a TSDoc comment good enough that hovering it in an editor is self-explanatory — a summary line, the relevant `@param`/`@returns`/`@throws`, and an `@example` on the major entry points.

**Non-goals** (so "documented" has a hard boundary):

- **No hosted API-reference site.** The deliverable is in-editor intellisense, not a web docs product.
- **No permanent CI enforcement gate.** A coverage script exists as a dev tool but is not wired into the gate suite.
- **No docs for the *generated* cs2 fields** (`schema.generated.d.ts`, `nav.generated.d.ts`, `events.generated.d.ts`, `csitem.generated.d.ts`). Those are a fundamentally different, larger effort (codegen/overrides layer) and are deferred.
- **No type or runtime changes.** This pass is **comments only** — the stubs' shapes are frozen. No exported type may change.
- **The `s2s` CLI implementation (`packages/sdk/src/**`) is out of scope** — it's internal contributor code, not the author-facing surface that produces plugin-author intellisense.

## 3. Scope — the file set

**In scope (33 hand-authored files):**

- The **31** `packages/sdk/*.d.ts` capability stubs:
  `admin`, `bans`, `chat`, `clients`, `commands`, `config`, `console`, `cookies`, `damage`, `db`, `entity`, `events`, `globals`, `http`, `interfaces`, `math`, `menu`, `net`, `plugin`, `plugins`, `server`, `sound`, `timers`, `topmenu`, `trace`, `translations`, `transmit`, `usercmd`, `usermessages`, `votes`, `ws`.
- The **2** hand-authored cs2 stubs: `packages/cs2/index.d.ts`, `packages/cs2/weapon.d.ts`.

**Explicitly excluded:** `packages/cs2/*.generated.d.ts`, `packages/sdk/src/**`, `packages/eslint-plugin/**`.

## 4. The convention (a short TSDoc house-style doc)

A new `docs/sdk-doc-conventions.md` codifies the style that `trace.d.ts`/`math.d.ts` already embody, so parallel PRs stay consistent. It states:

- **Every** exported symbol (function/const/class/interface/type/enum) **and every** interface/class member gets a `/** */` block.
- **Voice:** terse, factual, present-tense — match the existing files. The first sentence is a self-contained summary (it is what autocomplete shows before you commit to a symbol).
- **Tags:**
  - `@param name -` for non-obvious params (skip when the name fully says it).
  - `@returns` when the return semantics aren't obvious from the type (e.g. `fetch` resolves — not rejects — on 4xx/5xx with `ok=false`).
  - `@throws` / rejection semantics for anything that can throw or reject.
  - `@example` on **major entry points** (top-level functions + a namespace/object's primary methods) — a short, realistic snippet, **drawn from real `plugins/`/`examples/` usage**, not invented. Written as plain indented TS after the `@example` tag, with accurate imports.
  - `{@link Symbol}` to cross-reference related types (e.g. `TraceHit` ↔ `TraceOptions`); editors render these clickable.
  - `@defaultValue` to standardize the existing "Default X" prose on optional fields.
- **Keep** the existing per-file `@s2script/x — …` header banners and every RE/engine caveat verbatim (those are the most valuable content in the stubs).
- **Don't** merely restate the type; don't invent behavior — when unsure, read the shim/core or a real caller before writing the doc.

## 5. The coverage-audit script

`scripts/check-doc-coverage.mjs`:

- Uses the **TypeScript compiler API** (already a dependency) to parse each in-scope `.d.ts`, walk exported declarations and their members, and report any symbol with no leading JSDoc.
- Emits a per-file gap list + a total count; **exits non-zero** if any gaps remain.
- Accepts a file/glob filter so a PR can check only its own files.
- Optional `--warn-missing-example` soft-flags exported **functions** that lack an `@example` (a warning, not a failure — not every entry point needs one).

It is the objective **"done" measure per PR** (run filtered to the PR's files → zero gaps). It is **deliberately not** added to the CI gate suite (per the no-enforcement decision); wiring it in later is a trivial follow-up if ever wanted.

## 6. Execution — the stack (audit-driven, category-grouped)

Ship as a Graphite stack, each PR docs-only and independently gate-safe:

- **PR0 — foundation:** `docs/sdk-doc-conventions.md` + `scripts/check-doc-coverage.mjs` + fully document one currently-sparse exemplar (`http.d.ts`) as the worked reference (this also completes the flagship async-net file).
- **PR1 — async-net (rest):** `ws`, `net`, `db`, `cookies`
- **PR2 — entities & math:** `entity`, `trace`, `math`
- **PR3 — players & admin:** `clients`, `commands`, `chat`, `admin`, `bans`
- **PR4 — menus & votes:** `menu`, `votes`, `topmenu`, `sound`
- **PR5 — engine-core:** `events`, `damage`, `timers`, `server`, `console`, `plugin`, `plugins`, `interfaces`, `config`, `globals`, `transmit`, `usercmd`, `usermessages`
- **PR6 — cs2 game types + misc:** `cs2/index.d.ts`, `cs2/weapon.d.ts`, `translations`

PR0 + 6 = **7 PRs**. PR5 is the heaviest (13 files) and may be split into PR5a/PR5b if it feels bulky in review. Grouping mirrors the CLAUDE.md capability inventory so reviewers have context.

Each PR's Definition of Done:
1. `check-doc-coverage.mjs <its files>` → **zero gaps**.
2. `./scripts/check-plugins-typecheck.sh` (the 5E.1 gate) → **green** (proves the stubs still typecheck against every plugin/example; cheap insurance that no edit slipped past "comments only").
3. Its `@example`s cross-checked against real `plugins/`/`examples/` callers.

## 7. Verification

- **Types unchanged:** the 5E.1 typecheck gate on every PR is the guard that this stayed comments-only.
- **Completeness:** the coverage script drives it to zero per PR.
- **Correctness of examples:** each `@example` is derived from / matched to an actual caller in `plugins/` or `examples/`.

## 8. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| **Example rot** — `@example`s aren't type-checked, so they can drift as APIs evolve. | Derive from real callers; keep them short. Optional future work: an example-extraction typecheck harness. |
| **Voice drift across parallel PRs** — different authors, inconsistent tone. | The conventions doc + the PR0 exemplar are the guardrail. The audit guarantees *completeness*; reviewers own *quality*. |
| **"Fully documented" over-read** — generated cs2 fields stay bare. | Noted as an explicit non-goal in this spec and in `sdk-doc-conventions.md`. |
| **Accidental type edit** while "just adding comments." | The 5E.1 typecheck gate on each PR catches it. |

## 9. Open questions

None blocking. Deferred-by-decision: hosted API reference, CI enforcement gate, generated-schema docs, and CLI-source docs — each can become its own future slice if wanted.
