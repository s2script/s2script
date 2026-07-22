# Rich TSDoc for the author-facing SDK stubs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fill the uneven TSDoc coverage across the 33 hand-authored `@s2script/sdk` + `@s2script/cs2` `.d.ts` stubs so every author-facing exported symbol and member gives rich in-editor intellisense.

**Architecture:** A machine-checkable doc-coverage analyzer (TypeScript compiler API) drives the work; a short house-style doc + one fully-worked exemplar (`http.d.ts`) set the pattern; then six category PRs document the rest, each gated by "analyzer reports zero gaps for its files" + "the 5E.1 plugin typecheck stays green." Comments only — no exported type may change.

**Tech Stack:** TypeScript compiler API (`typescript` 5.9.3, already an SDK dep), `node:test` + `node:assert/strict`, Node `--experimental-strip-types` (the repo's proven way to run `.ts` from a script/test), Graphite (`gt`) for the stack.

## Global Constraints

- **Comments only.** No exported type, signature, name, or shape may change in any `.d.ts`. The only edits are `/** */` blocks (and keeping the existing per-file banner). Verified by the 5E.1 typecheck gate on every PR.
- **Scope = 33 hand-authored files only:** the 31 `packages/sdk/*.d.ts` + `packages/cs2/index.d.ts` + `packages/cs2/weapon.d.ts`. Never touch `packages/cs2/*.generated.d.ts`, `packages/sdk/src/**`, or `packages/eslint-plugin/**`.
- **Per-file banner stays.** Keep each file's existing `/** @s2script/x — … */` header verbatim; per the analyzer it is the module comment and does **not** satisfy the first symbol's own doc. Convention: banner, blank line, then per-symbol docs.
- **`@example`s come from real callers.** Draw snippets from actual `plugins/`/`examples/` usage (cited per task); keep them short with accurate imports. Don't invent behavior — read the shim/core or a caller when unsure.
- **The analyzer is a dev tool, not a CI gate.** Do NOT add `check-doc-coverage` to `.github/` or the gate suite.
- **Ship as a Graphite stack**, PR0 at the bottom, one PR per task-group below. Run the per-PR gate on each. Branch prefix: `sdk-tsdoc/…`. This plan's spec + this plan are already committed on `sdk-tsdoc/design-spec`; stack the work on top of it.
- **Voice:** terse, factual, present-tense — match `trace.d.ts`/`math.d.ts`. First sentence is a self-contained summary (it's what autocomplete shows).

---

## File Structure

**New files:**
- `packages/sdk/src/doccov/doccov.ts` — the pure doc-coverage analyzer (`analyzeSource`, `findUndocumented`). One responsibility: given `.d.ts` text, list exported symbols/members lacking their own `/** */`.
- `packages/sdk/test/doccov.test.mjs` — unit tests for the analyzer (inline fixtures, no temp files).
- `scripts/check-doc-coverage.mjs` — thin CLI: resolves the in-scope file set (or argv), calls the analyzer, prints gaps, exits non-zero if any. Mirrors `scripts/check-plugins-typecheck.sh`'s `--experimental-strip-types` invocation.
- `docs/sdk-doc-conventions.md` — the TSDoc house style.

**Modified files (docs only):** the 33 stubs, grouped into PRs 1–6 below.

---

## Task 1: Doc-coverage analyzer (PR0)

**Files:**
- Create: `packages/sdk/src/doccov/doccov.ts`
- Test: `packages/sdk/test/doccov.test.mjs`

**Interfaces:**
- Produces:
  - `analyzeSource(fileName: string, text: string): Gap[]` — analyze one file's source text.
  - `findUndocumented(files: string[]): Gap[]` — read + analyze many files.
  - `interface Gap { file: string; line: number; symbol: string; kind: string }` (`line` 1-based).

- [ ] **Step 1: Write the failing test**

Create `packages/sdk/test/doccov.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { analyzeSource } from "../src/doccov/doccov.ts";

const names = (gaps) => gaps.map((g) => g.symbol).sort();

test("flags an undocumented interface member but not a documented one", () => {
  const src = [
    "/** @s2script/x — banner. */",
    "",
    "/** A foo. */",
    "export interface Foo {",
    "  /** the a. */",
    "  a: number;",
    "  b: string;",
    "}",
  ].join("\n");
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), ["b"]);
});

test("the file banner does NOT count as the first symbol's own doc", () => {
  const src = [
    "/** @s2script/x — banner. */",
    "export interface Bar {",
    "  /** x. */ x: number;",
    "}",
  ].join("\n");
  // Bar is flagged (only doc above it is the banner); x is documented.
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), ["Bar"]);
});

test("walks the members of an exported const object type", () => {
  const src = [
    "/** banner */",
    "",
    "/** The API. */",
    "export declare const Api: {",
    "  /** does a. */",
    "  a(): void;",
    "  b(): void;",
    "};",
  ].join("\n");
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), ["b"]);
});

test("re-exports and imports are not flagged", () => {
  const src = [
    "/** banner */",
    "",
    'import type { Client } from "./clients";',
    'export * from "./schema.generated";',
    "/** A row. */",
    "export type Row = Record<string, string>;",
  ].join("\n");
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), []);
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/doccov.test.mjs`
Expected: FAIL — `Cannot find module '../src/doccov/doccov.ts'`.

- [ ] **Step 3: Implement the analyzer**

Create `packages/sdk/src/doccov/doccov.ts`:

```ts
// Doc-coverage analyzer for the author-facing .d.ts stubs (the intellisense surface).
// Flags every exported symbol / member lacking its OWN /** */ doc comment.
// The per-file banner (a /** at file offset 0) does NOT satisfy a symbol.
import ts from "typescript";
import { readFileSync } from "node:fs";

export interface Gap {
  file: string;
  line: number; // 1-based
  symbol: string;
  kind: string;
}

function hasExportMod(node: ts.Node): boolean {
  const mods = ts.canHaveModifiers(node) ? ts.getModifiers(node) : undefined;
  return !!mods?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword);
}

function memberName(node: ts.Node): string | null {
  const name = (node as { name?: ts.Node }).name;
  if (name && (ts.isIdentifier(name) || ts.isStringLiteral(name))) return name.text;
  return null;
}

export function analyzeSource(fileName: string, text: string): Gap[] {
  const sf = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, /*setParentNodes*/ true);
  const gaps: Gap[] = [];

  // Banner = a /** leading the first statement that starts at file offset 0.
  let bannerPos = -1;
  const first = sf.statements[0];
  if (first) {
    const r = (ts.getLeadingCommentRanges(text, first.getFullStart()) ?? []).find(
      (x) =>
        x.kind === ts.SyntaxKind.MultiLineCommentTrivia && text.slice(x.pos, x.pos + 3) === "/**",
    );
    if (r && r.pos === 0) bannerPos = 0;
  }

  const hasOwnDoc = (node: ts.Node): boolean =>
    (ts.getLeadingCommentRanges(text, node.getFullStart()) ?? []).some(
      (r) =>
        r.kind === ts.SyntaxKind.MultiLineCommentTrivia &&
        text.slice(r.pos, r.pos + 3) === "/**" &&
        r.pos !== bannerPos,
    );

  const lineOf = (node: ts.Node) => sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
  const flag = (node: ts.Node, name: string, kind: string) => {
    if (!hasOwnDoc(node)) gaps.push({ file: fileName, line: lineOf(node), symbol: name, kind });
  };

  const walkMembers = (members: ts.NodeArray<ts.Node>) => {
    for (const m of members) {
      switch (m.kind) {
        case ts.SyntaxKind.PropertySignature:
        case ts.SyntaxKind.MethodSignature:
        case ts.SyntaxKind.PropertyDeclaration:
        case ts.SyntaxKind.MethodDeclaration:
        case ts.SyntaxKind.GetAccessor:
        case ts.SyntaxKind.SetAccessor:
        case ts.SyntaxKind.EnumMember: {
          const nm = memberName(m);
          if (nm) flag(m, nm, ts.SyntaxKind[m.kind]);
          break;
        }
        default:
          break; // IndexSignature / Constructor / Construct+Call signatures → skipped
      }
    }
  };

  for (const st of sf.statements) {
    if (!hasExportMod(st)) continue; // imports, `export * from`, non-exported decls → skip
    if (ts.isInterfaceDeclaration(st)) {
      flag(st, st.name.text, "interface");
      walkMembers(st.members);
    } else if (ts.isClassDeclaration(st) && st.name) {
      flag(st, st.name.text, "class");
      walkMembers(st.members);
    } else if (ts.isEnumDeclaration(st)) {
      flag(st, st.name.text, "enum");
      walkMembers(st.members);
    } else if (ts.isTypeAliasDeclaration(st)) {
      flag(st, st.name.text, "type");
      if (ts.isTypeLiteralNode(st.type)) walkMembers(st.type.members);
    } else if (ts.isFunctionDeclaration(st) && st.name) {
      flag(st, st.name.text, "function");
    } else if (ts.isVariableStatement(st)) {
      const documented = hasOwnDoc(st);
      for (const d of st.declarationList.declarations) {
        const nm = ts.isIdentifier(d.name) ? d.name.text : "(const)";
        if (!documented) gaps.push({ file: fileName, line: lineOf(d), symbol: nm, kind: "const" });
        if (d.type && ts.isTypeLiteralNode(d.type)) walkMembers(d.type.members);
      }
    }
  }
  return gaps;
}

export function findUndocumented(files: string[]): Gap[] {
  return files.flatMap((f) => analyzeSource(f, readFileSync(f, "utf8")));
}
```

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/doccov.test.mjs`
Expected: PASS — all 4 tests green.

- [ ] **Step 5: Commit**

```bash
git add packages/sdk/src/doccov/doccov.ts packages/sdk/test/doccov.test.mjs
git commit -m "feat(sdk): doc-coverage analyzer for author-facing .d.ts stubs"
```

---

## Task 2: `check-doc-coverage` CLI (PR0)

**Files:**
- Create: `scripts/check-doc-coverage.mjs`

**Interfaces:**
- Consumes: `findUndocumented(files)` from Task 1.
- Produces: CLI `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs [file …]` — no args ⇒ the full 33-file in-scope set; exits 0 when clean, 1 with a per-file gap list otherwise.

- [ ] **Step 1: Implement the CLI**

Create `scripts/check-doc-coverage.mjs`:

```js
// Doc-coverage check for the author-facing .d.ts stubs (dev tool — NOT a CI gate).
// Usage: node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs [file ...]
// No file args → the full in-scope set (31 packages/sdk/*.d.ts + 2 cs2 hand-authored).
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..");
const { findUndocumented } = await import(join(repo, "packages/sdk/src/doccov/doccov.ts"));

function defaultFiles() {
  const sdkDir = join(repo, "packages/sdk");
  const sdk = readdirSync(sdkDir)
    .filter((f) => f.endsWith(".d.ts"))
    .map((f) => join(sdkDir, f));
  return [...sdk, join(repo, "packages/cs2/index.d.ts"), join(repo, "packages/cs2/weapon.d.ts")];
}

const args = process.argv.slice(2);
const files = args.length ? args : defaultFiles();
const gaps = findUndocumented(files);

if (gaps.length === 0) {
  console.log(`PASS: ${files.length} file(s) fully documented`);
  process.exit(0);
}

const byFile = new Map();
for (const g of gaps) {
  if (!byFile.has(g.file)) byFile.set(g.file, []);
  byFile.get(g.file).push(g);
}
for (const [file, gs] of byFile) {
  console.error(`\n${relative(repo, file)} — ${gs.length} undocumented:`);
  for (const g of gs) console.error(`  L${g.line}  ${g.kind} ${g.symbol}`);
}
console.error(`\nFAIL: ${gaps.length} undocumented symbol(s) across ${byFile.size} file(s)`);
process.exit(1);
```

- [ ] **Step 2: Run it against the whole surface (expect many gaps)**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs`
Expected: FAIL — a per-file list of undocumented symbols (this is the current gap; `http.d.ts`, `ws.d.ts`, etc. appear). This confirms the CLI works end-to-end.

- [ ] **Step 3: Confirm an already-clean file reports zero**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/trace.d.ts`
Expected: `PASS: 1 file(s) fully documented` (trace.d.ts is already fully documented — a sanity check that the analyzer isn't over-reporting).
If it reports gaps, fix the analyzer (Task 1) before proceeding — a false positive here means real work will be mis-scoped.

- [ ] **Step 4: Commit**

```bash
git add scripts/check-doc-coverage.mjs
git commit -m "feat(sdk): check-doc-coverage CLI over the author-facing stub set"
```

---

## Task 3: House-style conventions doc (PR0)

**Files:**
- Create: `docs/sdk-doc-conventions.md`

- [ ] **Step 1: Write the conventions doc**

Create `docs/sdk-doc-conventions.md`:

```markdown
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
```

- [ ] **Step 2: Commit**

```bash
git add docs/sdk-doc-conventions.md
git commit -m "docs(sdk): TSDoc house-style conventions for the author-facing stubs"
```

---

## Task 4: `http.d.ts` — the worked exemplar (PR0)

**Files:**
- Modify: `packages/sdk/http.d.ts`

**Interfaces:**
- `@example` source: `examples/http-demo/src/plugin.ts:24` (`fetch("https://httpbin.org/get", { timeoutMs: 15000 })`, reads `r.status`/`r.text()`).

- [ ] **Step 1: Confirm the current gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/http.d.ts`
Expected: FAIL listing `FetchOptions`, its fields, `Response`, its members (`status`, `ok`, `statusText`, `headers`, `text`, `json`).

- [ ] **Step 2: Rewrite `http.d.ts` fully documented**

Replace the whole file with (note: banner kept, blank line, then per-symbol docs):

```ts
/** @s2script/http — async HTTP. NO runtime code (injected as __s2pkg_http). */

/** Options for {@link fetch}. */
export interface FetchOptions {
  /** HTTP method (e.g. `"GET"`, `"POST"`). @defaultValue `"GET"` */
  method?: string;
  /** Request headers as a name→value map. */
  headers?: Record<string, string>;
  /** Request body (already-serialized string; set your own `content-type`). */
  body?: string;
  /** Abort the request after this many milliseconds. @defaultValue no timeout */
  timeoutMs?: number;
}

/** The response from a {@link fetch} call — a copied snapshot (no live socket). */
export interface Response {
  /** HTTP status code (e.g. `200`, `404`). */
  readonly status: number;
  /** True iff {@link Response.status} is in the 2xx range. */
  readonly ok: boolean;
  /** HTTP status reason phrase (e.g. `"Not Found"`). */
  readonly statusText: string;
  /** Response headers as a lower-cased name→value map. */
  readonly headers: Record<string, string>;
  /** The response body decoded as text. */
  text(): string;
  /** Parse the response body as JSON into `T` (throws on invalid JSON). */
  json<T = unknown>(): T;
}

/**
 * Perform an HTTP request off the game thread.
 * @param url - Absolute `http(s)://` URL.
 * @param options - Method, headers, body, and timeout (see {@link FetchOptions}).
 * @returns Resolves for ANY HTTP response — a 4xx/5xx resolves with `ok=false`, it does not reject.
 * @throws Rejects only on a network error or timeout.
 * @example
 * import { fetch } from "@s2script/sdk/http";
 * const r = await fetch("https://httpbin.org/get", { timeoutMs: 15000 });
 * if (r.ok) console.log(r.json<{ url: string }>().url);
 */
export declare function fetch(url: string, options?: FetchOptions): Promise<Response>;
```

- [ ] **Step 3: Verify the file is clean and types unchanged**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/http.d.ts`
Expected: `PASS: 1 file(s) fully documented`.

Run: `./scripts/check-plugins-typecheck.sh`
Expected: `PASS: all plugins and examples typecheck` (proves the http types still compile against every consumer — i.e. this was comments-only).

- [ ] **Step 4: Commit (bottom of the PR0 branch)**

```bash
git add packages/sdk/http.d.ts
git commit -m "docs(sdk): fully document http.d.ts (doc-conventions exemplar)"
```

> **PR0 boundary:** Tasks 1–4 form PR0. Open it with `gt` once all four are committed. PR body Stack Context: "Establishes the doc-coverage tooling, house style, and worked exemplar for the SDK intellisense pass." Then stack PRs 1–6 on top.

---

## How to execute a category task (Tasks 5–10)

Each category task documents its files to convention. The method is identical — the exemplar (`http.d.ts`) and the analyzer make it concrete and objective:

1. `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs <the task's files>` → read the gap list.
2. For each listed symbol, add a `/** */` per `docs/sdk-doc-conventions.md`, following the `http.d.ts` exemplar. Add `@example` (from the cited real caller) to the file's major entry point(s). Keep the banner; keep existing caveats.
3. Re-run the analyzer on the task's files → must print `PASS`.
4. `./scripts/check-plugins-typecheck.sh` → must print `PASS` (guards comments-only).
5. Commit, then `gt` the PR.

Each task below gives its file list, the exact `@example` caller sources, and one fully-worked before→after so the pattern for that category's shape is unambiguous.

---

## Task 5: async-net — the rest (PR1)

**Files:** Modify `packages/sdk/ws.d.ts`, `net.d.ts`, `db.d.ts`, `cookies.d.ts`.

**`@example` sources:** `ws` → `examples/ws-demo/src/plugin.ts:12` (`WebSocket.connect("wss://ws.postman-echo.com/raw")`). `db` → `plugins/clientprefs/src/plugin.ts:15` (`Database.open("clientprefs")`). `cookies` → `examples/clientprefs-demo/src/plugin.ts:25-26` (`Cookies.register` + `Cookies.get`). `net` has no in-repo caller — write a short illustrative `@example` on `Net.connectTcp`.

- [ ] **Step 1: Read the gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/ws.d.ts packages/sdk/net.d.ts packages/sdk/db.d.ts packages/sdk/cookies.d.ts`
Expected: FAIL — `ws` (WebSocket interface + its 5 members), `net` (TcpSocket/UdpSocket + members), `db` (SqlValue, Row, ExecuteResult, DriverConnection + members, ConnectionConfig, Driver + members, most Database members), `cookies` (CookieAccess + members, Cookie + members, CookieOptions + members).

- [ ] **Step 2: Document `ws.d.ts` (fully worked reference for this task)**

Replace with:

```ts
/** @s2script/ws — client WebSocket. NO runtime code (injected as __s2pkg_ws). */

/** An open WebSocket connection (a per-plugin handle). */
export interface WebSocket {
  /** Register a handler for each inbound text message. */
  onMessage(handler: (data: string) => void): void;
  /** Register a handler for connection close (`code`/`reason` from the close frame). */
  onClose(handler: (code: number, reason: string) => void): void;
  /** Register a handler for a transport error. */
  onError(handler: (err: string) => void): void;
  /** Send a text frame. */
  send(data: string): void;
  /** Close the connection. */
  close(): void;
}

/** Entry point for opening WebSocket connections. */
export declare const WebSocket: {
  /**
   * Connect to a WebSocket server (`wss://` for TLS).
   * @returns Resolves on the open handshake.
   * @throws Rejects on connect failure.
   * @example
   * import { WebSocket } from "@s2script/sdk/ws";
   * const ws = await WebSocket.connect("wss://ws.postman-echo.com/raw");
   * ws.onMessage((data) => console.log("echo:", data));
   * ws.send("ping");
   */
  connect(url: string): Promise<WebSocket>;
};
```

- [ ] **Step 3: Document `net.d.ts`, `db.d.ts`, `cookies.d.ts`**

Apply the same pattern to each remaining file (banner kept; one line per member; `@example` on the entry-point const). For `db.d.ts` put the `@example` on `Database.open`:

```ts
   * @example
   * import { Database } from "@s2script/sdk/db";
   * const db = await Database.open("clientprefs");
   * const rows = await db.query("SELECT value FROM prefs WHERE key = ?", ["boots"]);
```

For `cookies.d.ts` put the `@example` on `Cookies.register`:

```ts
   * @example
   * import { Cookies } from "@s2script/sdk/cookies";
   * const boots = Cookies.register("demo_boots", { default: "0" });
   * const n = parseInt(Cookies.get(client, boots), 10);
```

For `net.d.ts` put a short illustrative `@example` on `Net.connectTcp` (no in-repo caller):

```ts
   * @example
   * import { Net } from "@s2script/sdk/net";
   * const sock = await Net.connectTcp("127.0.0.1", 9000);
   * sock.onData((bytes) => { /* … */ });
   * sock.send("hello");
```

Document every member the analyzer listed (e.g. `db`'s `SqlValue`/`Row`/`ExecuteResult` type aliases, `DriverConnection.query`/`execute`/`close`, `cookies`' `CookieAccess.Public`/`Protected`/`Private`).

- [ ] **Step 4: Verify clean + types intact**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/ws.d.ts packages/sdk/net.d.ts packages/sdk/db.d.ts packages/sdk/cookies.d.ts`
Expected: `PASS: 4 file(s) fully documented`.

Run: `./scripts/check-plugins-typecheck.sh`
Expected: `PASS: all plugins and examples typecheck`.

- [ ] **Step 5: Commit + PR**

```bash
git add packages/sdk/ws.d.ts packages/sdk/net.d.ts packages/sdk/db.d.ts packages/sdk/cookies.d.ts
git commit -m "docs(sdk): document async-net stubs (ws, net, db, cookies)"
```
Then `gt` the PR (PR1). PR body Why: "Fills the intellisense gap on the async-network capability stubs."

---

## Task 6: entities & math (PR2)

**Files:** Modify `packages/sdk/entity.d.ts`, `trace.d.ts`, `math.d.ts`.

**Note:** `trace.d.ts` and `math.d.ts` are likely already clean; run the analyzer first — if they `PASS`, this task is only `entity.d.ts` (the largest stub, ~161 lines, ~51 existing JSDoc — expect partial gaps on members/interfaces, not a full rewrite).

**`@example` source:** entity creation/handling from `examples/` (grep `createEntity`/`EntityRef` under `plugins/`/`examples/` for a real snippet); if none fits a given entry point, a short illustrative `@example` is acceptable per the conventions.

- [ ] **Step 1: Read the gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/entity.d.ts packages/sdk/trace.d.ts packages/sdk/math.d.ts`
Expected: a gap list scoped to `entity.d.ts` members/interfaces (trace/math likely already `PASS`).

- [ ] **Step 2: Document each listed symbol**

Follow the exemplar. Worked pattern for a bare interface member (apply to every gap):

```ts
// before
export interface EntityRef { index: number; id: number; }
// after
/** A liveness-gated reference to an entity — resolve to a live view or `null`; never a raw pointer. */
export interface EntityRef {
  /** The entity's slot index in the game entity system. */
  index: number;
  /** The host-minted identity used to detect a stale/recycled slot. */
  id: number;
}
```

(Read the surrounding code and the [[cs2-schema-entity-access]] / entity-safety notes in CLAUDE.md before writing behavioral claims — don't invent semantics.)

- [ ] **Step 3: Verify + commit**

Run the analyzer on the three files → `PASS: 3 file(s) fully documented`.
Run `./scripts/check-plugins-typecheck.sh` → `PASS`.
```bash
git add packages/sdk/entity.d.ts packages/sdk/trace.d.ts packages/sdk/math.d.ts
git commit -m "docs(sdk): document entity/trace/math stubs"
```
Then `gt` the PR (PR2).

---

## Task 7: players & admin (PR3)

**Files:** Modify `packages/sdk/clients.d.ts`, `commands.d.ts`, `chat.d.ts`, `admin.d.ts`, `bans.d.ts`.

**`@example` sources:** grep `plugins/basecommands`, `plugins/basechat`, `plugins/basebans`, `plugins/playercommands` for real `Commands.register`/`Chat`/`Admin`/`Bans` usage; cite the file:line in each `@example`.

- [ ] **Step 1: Read the gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/clients.d.ts packages/sdk/commands.d.ts packages/sdk/chat.d.ts packages/sdk/admin.d.ts packages/sdk/bans.d.ts`

- [ ] **Step 2: Document each listed symbol**

Follow the exemplar. Worked pattern for an entry-point method with a real `@example`:

```ts
/**
 * Register a chat/console command.
 * @param name - Command name without the leading `sm_`/`!`.
 * @example
 * import { Commands } from "@s2script/sdk/commands";
 * Commands.register("hello", (client, args) => Chat.reply(client, "hi"));
 */
register(name: string, handler: CommandHandler): void;
```

Replace the illustrative body with the actual signature/behavior from `commands.d.ts` and a snippet lifted from a real base-plugin caller.

- [ ] **Step 3: Verify + commit**

Analyzer on the five files → `PASS`. `./scripts/check-plugins-typecheck.sh` → `PASS`.
```bash
git add packages/sdk/clients.d.ts packages/sdk/commands.d.ts packages/sdk/chat.d.ts packages/sdk/admin.d.ts packages/sdk/bans.d.ts
git commit -m "docs(sdk): document players/admin stubs (clients, commands, chat, admin, bans)"
```
Then `gt` the PR (PR3).

---

## Task 8: menus & votes (PR4)

**Files:** Modify `packages/sdk/menu.d.ts`, `votes.d.ts`, `topmenu.d.ts`, `sound.d.ts`.

**`@example` sources:** grep `plugins/disabled/nominations`, `plugins/disabled/rockthevote`, `plugins/adminmenu`/`basevotes` (and any `Menu`/`Vote`/`TopMenu`/`sound` caller) for real usage; cite file:line.

- [ ] **Step 1: Read the gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/menu.d.ts packages/sdk/votes.d.ts packages/sdk/topmenu.d.ts packages/sdk/sound.d.ts`

- [ ] **Step 2: Document each listed symbol**

Follow the exemplar (one-line member descriptions; `@example` on the menu/vote entry points from a real caller). Read the [[cs2-menu-and-per-client-events]] context in CLAUDE.md for accurate behavioral wording.

- [ ] **Step 3: Verify + commit**

Analyzer on the four files → `PASS`. `./scripts/check-plugins-typecheck.sh` → `PASS`.
```bash
git add packages/sdk/menu.d.ts packages/sdk/votes.d.ts packages/sdk/topmenu.d.ts packages/sdk/sound.d.ts
git commit -m "docs(sdk): document menus/votes stubs (menu, votes, topmenu, sound)"
```
Then `gt` the PR (PR4).

---

## Task 9: engine-core (PR5)

**Files:** Modify `packages/sdk/events.d.ts`, `damage.d.ts`, `timers.d.ts`, `server.d.ts`, `console.d.ts`, `plugin.d.ts`, `plugins.d.ts`, `interfaces.d.ts`, `config.d.ts`, `globals.d.ts`, `transmit.d.ts`, `usercmd.d.ts`, `usermessages.d.ts`.

**`@example` sources:** grep base plugins for `Events.on`/`Damage.onPre`/`delay`/`nextTick`/`Server.command`/`Console`/`plugin(ctx)`/`UserCmd`/`UserMessages` usage; cite file:line per entry point.

**Split note:** 13 files. If review feels heavy, split into **PR5a** = `events, damage, timers, server, console, plugin, plugins` and **PR5b** = `interfaces, config, globals, transmit, usercmd, usermessages` (two commits, two PRs). The gate runs on each half's file set.

- [ ] **Step 1: Read the gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/sdk/events.d.ts packages/sdk/damage.d.ts packages/sdk/timers.d.ts packages/sdk/server.d.ts packages/sdk/console.d.ts packages/sdk/plugin.d.ts packages/sdk/plugins.d.ts packages/sdk/interfaces.d.ts packages/sdk/config.d.ts packages/sdk/globals.d.ts packages/sdk/transmit.d.ts packages/sdk/usercmd.d.ts packages/sdk/usermessages.d.ts`

- [ ] **Step 2: Document each listed symbol**

Follow the exemplar. These stubs carry dense RE/engine caveats (usercmd SLOT math, usermessage hook path, damage detour) — **preserve every existing caveat** and read the matching CLAUDE.md/[[usercmd-primitive]]/[[usermessage-hook]]/[[cs2-damage-hooks-detour]] notes before writing behavioral claims.

- [ ] **Step 3: Verify + commit**

Analyzer on all 13 (or each half) → `PASS`. `./scripts/check-plugins-typecheck.sh` → `PASS`.
```bash
git add packages/sdk/events.d.ts packages/sdk/damage.d.ts packages/sdk/timers.d.ts packages/sdk/server.d.ts packages/sdk/console.d.ts packages/sdk/plugin.d.ts packages/sdk/plugins.d.ts packages/sdk/interfaces.d.ts packages/sdk/config.d.ts packages/sdk/globals.d.ts packages/sdk/transmit.d.ts packages/sdk/usercmd.d.ts packages/sdk/usermessages.d.ts
git commit -m "docs(sdk): document engine-core stubs"
```
Then `gt` the PR(s) (PR5 / PR5a+PR5b).

---

## Task 10: cs2 game types + translations, and the final full-surface gate (PR6)

**Files:** Modify `packages/cs2/index.d.ts`, `packages/cs2/weapon.d.ts`, `packages/sdk/translations.d.ts`. Append a note to `docs/PROGRESS.md`.

**`@example` sources:** grep `plugins/`/`examples/` and the TTT port for `Pawn`/`Weapon`/`Translations` usage; cite file:line. `cs2/index.d.ts` already has ~68 JSDoc blocks — expect partial gaps (the hand-written Pawn/entry-point members), not a rewrite.

- [ ] **Step 1: Read the gaps**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/cs2/index.d.ts packages/cs2/weapon.d.ts packages/sdk/translations.d.ts`

- [ ] **Step 2: Document each listed symbol**

Follow the exemplar. For `cs2/index.d.ts`, only the hand-written surface is in scope; the `export * from "./schema.generated"` re-export is not flagged (generated fields stay bare — a documented non-goal).

- [ ] **Step 3: Verify the task's files, then the WHOLE surface**

Run: `node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs packages/cs2/index.d.ts packages/cs2/weapon.d.ts packages/sdk/translations.d.ts`
Expected: `PASS`.

Run the full-surface gate (no args → all 33 files):
`node --experimental-strip-types --no-warnings scripts/check-doc-coverage.mjs`
Expected: `PASS: 33 file(s) fully documented` — the whole author-facing surface is now covered.

Run: `./scripts/check-plugins-typecheck.sh`
Expected: `PASS`.

- [ ] **Step 4: Record it in PROGRESS.md**

Append a short entry to `docs/PROGRESS.md` noting the SDK TSDoc intellisense pass: the 33-file coverage, the `check-doc-coverage` dev tool, and the deferred generated-cs2 docs.

- [ ] **Step 5: Commit + PR**

```bash
git add packages/cs2/index.d.ts packages/cs2/weapon.d.ts packages/sdk/translations.d.ts docs/PROGRESS.md
git commit -m "docs(sdk): document cs2 game-type stubs + translations; full-surface coverage"
```
Then `gt` the PR (PR6). PR body Why: "Completes the SDK intellisense pass — the full 33-file author-facing surface reports zero doc gaps."

---

## Self-Review

**Spec coverage:**
- Uneven-coverage problem (spec §1) → Tasks 4–10 fill it; §2 comments-only guard → the `check-plugins-typecheck.sh` step in every category task.
- Scope = 33 hand-authored files (spec §3) → the `defaultFiles()` set in Task 2 + the exact file lists in Tasks 4–10; generated/CLI/eslint excluded (never listed, and the `export *` re-export is unflagged).
- Convention doc (spec §4) → Task 3. Coverage-audit script (spec §5) → Tasks 1–2; `--warn-missing-example` was specced as *optional* and is intentionally omitted from the plan to avoid scope creep (the conventions doc still requires `@example` on entry points; reviewers enforce it).
- 7-PR stack (spec §6) → PR0 (Tasks 1–4), PR1 (5), PR2 (6), PR3 (7), PR4 (8), PR5 (9, splittable), PR6 (10). DoD per PR (analyzer zero-gaps + typecheck green + real-caller examples) → the verify steps in each task.
- Verification (spec §7) → the two-command gate (analyzer + typecheck) in every category task; final full-surface gate in Task 10.
- Risks (spec §8): example-rot → examples cited from real callers; voice drift → conventions doc + exemplar; over-read → non-goal noted in the conventions doc and Task 10; accidental type edit → typecheck gate each PR.

**Placeholder scan:** No TBD/TODO. The category tasks (5–10) intentionally instruct "document every symbol the analyzer lists following the exemplar" rather than hand-listing ~600 comments — the method is shown in full (analyzer + conventions + worked `http.d.ts`/`ws.d.ts`/`EntityRef`/`Commands.register` examples) and completeness is machine-verified, which is the honest granularity for a docs pass. The `/* … */` fragments inside `@example` blocks are illustrative snippet bodies, not plan placeholders.

**Type consistency:** `Gap { file, line, symbol, kind }`, `analyzeSource(fileName, text)`, `findUndocumented(files)` are used identically in Tasks 1 (definition), 2 (CLI import), and the test. CLI invocation flags (`--experimental-strip-types --no-warnings`) match the repo's proven `check-plugins-typecheck.sh` pattern throughout.
