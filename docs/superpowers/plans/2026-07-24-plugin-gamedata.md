# Plugin-Shippable Gamedata & Declared Engine Calls — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a plugin ship its own gamedata declaring an engine function core does not wrap, and call it from TypeScript with generated types, load-time validation, and per-descriptor degradation.

**Architecture:** Three layers. **SDK** (TS) owns the format — parse/validate the plugin's gamedata, generate `.s2script/gamedata.d.ts`, pack `gamedata.json` into the `.s2sp`. **Core** (Rust) owns the per-plugin descriptor registry, argument marshalling, and the `Engine.call`/`Engine.status` natives; class/field/signature names cross core as opaque strings only. **Shim** (C++) owns resolution (sigscan, RTTI vtable, `prologue` validation, `.text`-range) and the actual invoke.

**Tech Stack:** TypeScript + vitest (`.test.mjs`) for the SDK; Rust (`cargo test -p s2script-core`) for core; C++17 + `nlohmann::json` for the shim; the existing `s2sig` pattern matcher.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md`. Every requirement there applies.
- **Arg budget:** at most **5 integer-class** args and at most **8 `float`** args. Integer-class = `bool`, `int`, `entity`, `string`, `vector`. Exceeding either bound **fails the build**.
- **Return vocabulary:** `void`, `bool`, `int`, `float`, `entity`. Arg vocabulary: `bool`, `int`, `float`, `string`, `vector`, `entity`.
- **v1 validator set is exactly `{ prologue }`.** A `target.kind: "vtable"` without `validate.prologue` **fails the build**.
- **v1 gamedata sections are exactly `{ signatures, calls }`.** `interfaces`/`offsets` in plugin gamedata is a build error.
- **`S2EngineOps` is ABI-ordered.** New op fields are **appended only** — never inserted (the struct keeps dead fields "for ABI order"). Same for the shim's populate order.
- **No raw pointer crosses into JS.** Core passes entity `(index, serial)` pairs; the shim resolves pointers. Entity returns go pointer → identity `CEntityHandle` → books-gated `__s2_handle_adopt`.
- **Boundary:** no game identifier may be compiled into `core/`. `make check-boundary` must stay green.
- **Platform id:** `linuxsteamrt64`. Gamedata shape is a named entry whose keys are platform ids, details nested inside (matches `gamedata/core.gamedata.jsonc`).
- **Degrade, never crash.** Every failure is per-descriptor with a named reason.

## File Structure

| File | Responsibility |
|---|---|
| `packages/sdk/src/gamedata/types.ts` | Plugin-gamedata TS types (`PluginGamedata`, `CallDecl`, `ArgKind`, `RetKind`) |
| `packages/sdk/src/gamedata/validate.ts` | Pure validator → `string[]` errors (mirrors `config-validate.ts` style) |
| `packages/sdk/src/gamedata/gen-types.ts` | Emit `.s2script/gamedata.d.ts` from validated gamedata |
| `packages/sdk/src/build.ts` | Wire: load → validate → generate → typecheck → pack `gamedata.json` |
| `packages/sdk/unsafe.d.ts` | `@s2script/sdk/unsafe` author contract (`Engine`, `EngineCalls`) |
| `core/config-templates/permissions.json` | Operator allow-list template |
| `core/src/gamedata_calls.rs` | Descriptor registry, arg marshalling, permission check |
| `core/src/loader.rs` | `Manifest` gains `gamedata` + `permissions`; read `gamedata.json` member |
| `core/src/v8host.rs` | Two appended engine ops + `__s2_engine_call_*` natives + prelude |
| `shim/src/engine_calls.h/.cpp` | Resolve (sig/vtable + prologue + `.text`) and invoke (SysV thunk) |
| `shim/src/s2script_mm.cpp` | Populate the two new ops |

---

### Task 1: Plugin gamedata types + validator (SDK)

**Files:**
- Create: `packages/sdk/src/gamedata/types.ts`
- Create: `packages/sdk/src/gamedata/validate.ts`
- Test: `packages/sdk/test/gamedata-validate.test.mjs`

**Interfaces:**
- Consumes: nothing.
- Produces: `validatePluginGamedata(gd: unknown, opts: { permissions: string[] }): string[]` — returns `[]` when valid. Types `PluginGamedata`, `CallDecl`, `ArgKind = "bool"|"int"|"float"|"string"|"vector"|"entity"`, `RetKind = "void"|"bool"|"int"|"float"|"entity"`, and `INT_CLASS_ARGS: ReadonlySet<ArgKind>`.

- [ ] **Step 1: Write the failing test**

```js
// packages/sdk/test/gamedata-validate.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { validatePluginGamedata } from "../dist/gamedata/validate.js";

const PERMS = ["engine:calls"];
const sigCall = {
  signatures: { Foo: { linuxsteamrt64: { module: "libserver.so", pattern: "55 48", resolve: "direct" } } },
  calls: { foo: { receiver: { kind: "entity" }, target: { kind: "signature", name: "Foo" }, args: [], returns: "void" } },
};

test("a valid signature-backed call passes", () => {
  assert.deepEqual(validatePluginGamedata(sigCall, { permissions: PERMS }), []);
});

test("calls section without engine:calls permission is rejected", () => {
  const errs = validatePluginGamedata(sigCall, { permissions: [] });
  assert.ok(errs.some((e) => e.includes("engine:calls")));
});

test("vtable target without validate.prologue is rejected", () => {
  const gd = { calls: { f: { receiver: { kind: "entity" },
    target: { kind: "vtable", class: "C", linuxsteamrt64: { index: 5 } }, args: [], returns: "void" } } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("validate.prologue")));
});

test("six integer-class args are rejected (this occupies rdi)", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["int", "int", "int", "bool", "entity", "string"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("integer-class")));
});

test("five integer-class plus eight float args are accepted", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["int", "int", "int", "bool", "entity", ...Array(8).fill("float")];
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test("nine float args are rejected", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = Array(9).fill("float");
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("float")));
});

test("unknown arg kind is rejected", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["pointer"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("pointer")));
});

test("string return is rejected in v1", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.returns = "string";
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("returns")));
});

test("an offsets section is rejected in v1", () => {
  const gd = { ...structuredClone(sigCall), offsets: { X: { linuxsteamrt64: 8 } } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("offsets")));
});

test("a signature target naming a missing signature is rejected", () => {
  const gd = { calls: { f: { receiver: { kind: "entity" },
    target: { kind: "signature", name: "Nope" }, args: [], returns: "void" } } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("Nope")));
});

test("receiver.via requires both class and field", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.receiver.via = { class: "CBasePlayerPawn" };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("via")));
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/sdk && npm run build && node --test test/gamedata-validate.test.mjs`
Expected: FAIL — cannot find module `../dist/gamedata/validate.js`.

- [ ] **Step 3: Write the types**

```ts
// packages/sdk/src/gamedata/types.ts
/** Plugin-shippable gamedata. v1 accepts ONLY `signatures` + `calls` (spec §14). */
export const PLATFORM = "linuxsteamrt64" as const;

export type ArgKind = "bool" | "int" | "float" | "string" | "vector" | "entity";
export type RetKind = "void" | "bool" | "int" | "float" | "entity";

export const ARG_KINDS: readonly ArgKind[] = ["bool", "int", "float", "string", "vector", "entity"];
export const RET_KINDS: readonly RetKind[] = ["void", "bool", "int", "float", "entity"];

/** Everything except `float` occupies an integer register under SysV. */
export const INT_CLASS_ARGS: ReadonlySet<ArgKind> = new Set<ArgKind>(["bool", "int", "string", "vector", "entity"]);
/** `this` consumes rdi, leaving 5 GP argument registers. */
export const MAX_INT_ARGS = 5;
export const MAX_FLOAT_ARGS = 8;

export interface SigSpec { module: string; pattern: string; resolve: string }
export interface ViaSpec { class: string; field: string }
export interface Receiver { kind: "entity"; via?: ViaSpec }

export interface SignatureTarget { kind: "signature"; name: string }
export interface VtablePlatform { index: number; validate: { prologue: string } }
export interface VtableTarget { kind: "vtable"; class: string; [platform: string]: unknown }

export interface CallDecl {
  receiver: Receiver;
  target: SignatureTarget | VtableTarget;
  args: ArgKind[];
  returns: RetKind;
}

export interface PluginGamedata {
  signatures?: Record<string, Record<string, SigSpec>>;
  calls?: Record<string, CallDecl>;
}
```

- [ ] **Step 4: Write the validator**

```ts
// packages/sdk/src/gamedata/validate.ts
import {
  ARG_KINDS, RET_KINDS, INT_CLASS_ARGS, MAX_INT_ARGS, MAX_FLOAT_ARGS, PLATFORM,
  type ArgKind, type PluginGamedata,
} from "./types.js";

const ALLOWED_SECTIONS = new Set(["signatures", "calls"]);

/** Validate a plugin's gamedata. Returns [] when valid; every string is a build-blocking error. */
export function validatePluginGamedata(gd: unknown, opts: { permissions: string[] }): string[] {
  const errs: string[] = [];
  if (gd == null || typeof gd !== "object" || Array.isArray(gd)) return ["gamedata must be an object"];
  const g = gd as Record<string, unknown>;

  for (const key of Object.keys(g)) {
    if (!ALLOWED_SECTIONS.has(key)) {
      errs.push(`gamedata section '${key}' is not supported in v1 (allowed: signatures, calls)`);
    }
  }

  const sigs = (g.signatures ?? {}) as Record<string, Record<string, unknown>>;
  if (typeof sigs !== "object" || Array.isArray(sigs)) errs.push("signatures must be an object");
  for (const [name, platforms] of Object.entries(sigs)) {
    const p = (platforms as Record<string, unknown>)?.[PLATFORM] as Record<string, unknown> | undefined;
    if (!p) { errs.push(`signature '${name}': missing platform '${PLATFORM}'`); continue; }
    for (const f of ["module", "pattern", "resolve"]) {
      if (typeof p[f] !== "string" || !(p[f] as string).length) {
        errs.push(`signature '${name}': '${f}' must be a non-empty string`);
      }
    }
  }

  const calls = (g.calls ?? {}) as Record<string, unknown>;
  const callNames = Object.keys(calls);
  if (callNames.length && !opts.permissions.includes("engine:calls")) {
    errs.push('gamedata declares a `calls` section but the manifest does not declare permission "engine:calls"');
  }

  for (const [name, rawDecl] of Object.entries(calls)) {
    const where = `call '${name}'`;
    if (rawDecl == null || typeof rawDecl !== "object") { errs.push(`${where}: must be an object`); continue; }
    const decl = rawDecl as Record<string, unknown>;

    // receiver
    const recv = decl.receiver as Record<string, unknown> | undefined;
    if (!recv || recv.kind !== "entity") {
      errs.push(`${where}: receiver.kind must be "entity" in v1`);
    } else if (recv.via !== undefined) {
      const via = recv.via as Record<string, unknown>;
      if (typeof via?.class !== "string" || typeof via?.field !== "string") {
        errs.push(`${where}: receiver.via requires both 'class' and 'field' strings`);
      }
    }

    // target
    const target = decl.target as Record<string, unknown> | undefined;
    if (!target) {
      errs.push(`${where}: missing 'target'`);
    } else if (target.kind === "signature") {
      if (typeof target.name !== "string") errs.push(`${where}: target.name must be a string`);
      else if (!(target.name in sigs)) errs.push(`${where}: target.name '${target.name}' has no entry in 'signatures'`);
    } else if (target.kind === "vtable") {
      if (typeof target.class !== "string") errs.push(`${where}: vtable target requires 'class'`);
      const plat = target[PLATFORM] as Record<string, unknown> | undefined;
      if (!plat) {
        errs.push(`${where}: vtable target missing platform '${PLATFORM}'`);
      } else {
        if (!Number.isInteger(plat.index) || (plat.index as number) < 0) {
          errs.push(`${where}: vtable index must be a non-negative integer`);
        }
        const prologue = (plat.validate as Record<string, unknown> | undefined)?.prologue;
        if (typeof prologue !== "string" || !prologue.length) {
          errs.push(`${where}: a vtable target REQUIRES validate.prologue (a bare borrowed index is never trusted)`);
        }
      }
    } else {
      errs.push(`${where}: target.kind must be "signature" or "vtable"`);
    }

    // args
    const args = decl.args;
    if (!Array.isArray(args)) {
      errs.push(`${where}: 'args' must be an array`);
    } else {
      let ints = 0, floats = 0;
      for (const a of args) {
        if (typeof a !== "string" || !ARG_KINDS.includes(a as ArgKind)) {
          errs.push(`${where}: unknown arg kind ${JSON.stringify(a)} (allowed: ${ARG_KINDS.join(", ")})`);
          continue;
        }
        if (INT_CLASS_ARGS.has(a as ArgKind)) ints++; else floats++;
      }
      if (ints > MAX_INT_ARGS) {
        errs.push(`${where}: ${ints} integer-class args exceeds the max of ${MAX_INT_ARGS} (\`this\` occupies rdi)`);
      }
      if (floats > MAX_FLOAT_ARGS) {
        errs.push(`${where}: ${floats} float args exceeds the max of ${MAX_FLOAT_ARGS}`);
      }
    }

    // returns
    if (typeof decl.returns !== "string" || !RET_KINDS.includes(decl.returns as never)) {
      errs.push(`${where}: 'returns' must be one of ${RET_KINDS.join(", ")}`);
    }
  }

  return errs;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd packages/sdk && npm run build && node --test test/gamedata-validate.test.mjs`
Expected: PASS (11 tests).

- [ ] **Step 6: Commit**

```bash
git add packages/sdk/src/gamedata/ packages/sdk/test/gamedata-validate.test.mjs
git commit -m "Add plugin gamedata types and validator"
```

---

### Task 2: Type generation (SDK)

**Files:**
- Create: `packages/sdk/src/gamedata/gen-types.ts`
- Test: `packages/sdk/test/gamedata-gen.test.mjs`

**Interfaces:**
- Consumes: `PluginGamedata`, `ArgKind`, `RetKind` from Task 1.
- Produces: `generateGamedataTypes(gd: PluginGamedata): string` — the full text of `.s2script/gamedata.d.ts`.

- [ ] **Step 1: Write the failing test**

```js
// packages/sdk/test/gamedata-gen.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { generateGamedataTypes } from "../dist/gamedata/gen-types.js";

const gd = {
  calls: {
    ignite: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
              args: ["float", "bool"], returns: "void" },
    lookupAttachment: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
              args: ["string"], returns: "int" },
    dropWeapon: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
              args: ["entity"], returns: "entity" },
  },
};

test("imports EntityRef so the module augmentation resolves", () => {
  const out = generateGamedataTypes(gd);
  assert.match(out, /import type \{ EntityRef \} from "@s2script\/sdk\/entity";/);
});

test("augments @s2script/sdk/unsafe with an EngineCalls interface", () => {
  assert.match(generateGamedataTypes(gd), /declare module "@s2script\/sdk\/unsafe"\s*\{[\s\S]*interface EngineCalls/);
});

test("void return stays void; non-void returns are nullable", () => {
  const out = generateGamedataTypes(gd);
  assert.match(out, /ignite: \(self: EntityRef, a0: number, a1: boolean\) => void;/);
  assert.match(out, /lookupAttachment: \(self: EntityRef, a0: string\) => number \| null;/);
});

test("entity args are nullable EntityRef and entity returns are EntityRef | null", () => {
  assert.match(generateGamedataTypes(gd),
    /dropWeapon: \(self: EntityRef, a0: EntityRef \| null\) => EntityRef \| null;/);
});

test("a gamedata with no calls still emits a valid empty augmentation", () => {
  const out = generateGamedataTypes({});
  assert.match(out, /interface EngineCalls \{\s*\}/);
});

test("output is deterministic (calls sorted by name)", () => {
  assert.equal(generateGamedataTypes(gd), generateGamedataTypes(gd));
  const idxDrop = generateGamedataTypes(gd).indexOf("dropWeapon");
  const idxIgnite = generateGamedataTypes(gd).indexOf("ignite");
  assert.ok(idxDrop < idxIgnite, "expected alphabetical ordering");
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/sdk && npm run build && node --test test/gamedata-gen.test.mjs`
Expected: FAIL — cannot find module `../dist/gamedata/gen-types.js`.

- [ ] **Step 3: Write the generator**

```ts
// packages/sdk/src/gamedata/gen-types.ts
import type { ArgKind, PluginGamedata, RetKind } from "./types.js";

const ARG_TS: Record<ArgKind, string> = {
  bool: "boolean", int: "number", float: "number",
  string: "string", vector: "readonly [number, number, number]",
  entity: "EntityRef | null",
};
const RET_TS: Record<RetKind, string> = {
  void: "void", bool: "boolean | null", int: "number | null",
  float: "number | null", entity: "EntityRef | null",
};

/** Emit `.s2script/gamedata.d.ts`. Deterministic: calls are sorted by name. */
export function generateGamedataTypes(gd: PluginGamedata): string {
  const calls = gd.calls ?? {};
  const lines = Object.keys(calls)
    .sort()
    .map((name) => {
      const decl = calls[name]!;
      const params = ["self: EntityRef", ...decl.args.map((a, i) => `a${i}: ${ARG_TS[a]}`)];
      return `    ${name}: (${params.join(", ")}) => ${RET_TS[decl.returns]};`;
    });

  const body = lines.length ? `\n${lines.join("\n")}\n  ` : "\n  ";
  return [
    "// GENERATED by `s2s build` from the plugin's gamedata — DO NOT EDIT.",
    "// The EntityRef import is REQUIRED: a `declare module` augmentation resolves type names in its",
    "// own file scope, so an un-imported EntityRef degrades to an error skipLibCheck can swallow.",
    'import type { EntityRef } from "@s2script/sdk/entity";',
    "",
    'declare module "@s2script/sdk/unsafe" {',
    `  interface EngineCalls {${body}}`,
    "}",
    "",
  ].join("\n");
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd packages/sdk && npm run build && node --test test/gamedata-gen.test.mjs`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/sdk/src/gamedata/gen-types.ts packages/sdk/test/gamedata-gen.test.mjs
git commit -m "Generate EngineCalls types from plugin gamedata"
```

---

### Task 3: `@s2script/sdk/unsafe` author contract

**Files:**
- Create: `packages/sdk/unsafe.d.ts`
- Modify: `packages/sdk/package.json` (add `"./unsafe"` to `exports`)
- Test: `packages/sdk/test/gamedata-exports.test.mjs`

**Interfaces:**
- Produces: the `@s2script/sdk/unsafe` module declaring `EngineCalls` (empty base, augmented by Task 2's output) and `Engine`.

- [ ] **Step 1: Write the failing test**

```js
// packages/sdk/test/gamedata-exports.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("package.json exports ./unsafe", () => {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.ok(pkg.exports["./unsafe"], "expected an ./unsafe export subpath");
});

test("unsafe.d.ts declares an augmentable EngineCalls and Engine", () => {
  const dts = readFileSync(join(root, "unsafe.d.ts"), "utf8");
  assert.match(dts, /export interface EngineCalls/);
  assert.match(dts, /export declare const Engine/);
  assert.match(dts, /status\(/);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/sdk && node --test test/gamedata-exports.test.mjs`
Expected: FAIL — `unsafe.d.ts` does not exist.

- [ ] **Step 3: Write the contract**

```ts
// packages/sdk/unsafe.d.ts
/**
 * @s2script/sdk/unsafe — plugin-declared engine calls. NO runtime code: the engine injects the
 * implementation at load (`__s2pkg_unsafe`).
 *
 * UNSAFE by design. A declared call reaches a real engine function. The framework validates every
 * descriptor at load and degrades a failure to `null`, but it cannot prove your signature or vtable
 * index names the function you meant. Requires manifest `permissions: ["engine:calls"]` AND an
 * operator allow-list entry (see docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md §6).
 */

/**
 * The calls this plugin declares. The base interface is EMPTY — `s2s build` generates
 * `.s2script/gamedata.d.ts`, which augments this from your gamedata's `calls` section.
 */
export interface EngineCalls {}

export declare const Engine: {
  /**
   * The declared call, or `null` when its descriptor failed a load-time gate (signature miss,
   * validator rejection, slot outside `.text`, missing platform entry, or the plugin is not
   * operator-allow-listed). Guard once at load; the returned function is a plain callable.
   */
  call<K extends keyof EngineCalls>(name: K): EngineCalls[K] | null;
  /** Why a descriptor is unavailable; `"available"` when it resolved. For diagnostics/operator reports. */
  status(name: string): string;
};
```

- [ ] **Step 4: Add the export subpath**

In `packages/sdk/package.json`, add to `exports`, keeping the existing alphabetical placement style (between `"./translations"` and `"./usercmd"`):

```json
    "./unsafe": { "types": "./unsafe.d.ts" },
```

Match the exact shape of the sibling entries in that file — if they use a different key set (e.g. a bare string), mirror that instead.

- [ ] **Step 5: Run test to verify it passes**

Run: `cd packages/sdk && node --test test/gamedata-exports.test.mjs`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add packages/sdk/unsafe.d.ts packages/sdk/package.json packages/sdk/test/gamedata-exports.test.mjs
git commit -m "Add the @s2script/sdk/unsafe author contract"
```

---

### Task 4: Wire gamedata into `s2s build`

**Files:**
- Modify: `packages/sdk/src/build.ts`
- Test: `packages/sdk/test/gamedata-build.test.mjs`

**Interfaces:**
- Consumes: `validatePluginGamedata` (Task 1), `generateGamedataTypes` (Task 2).
- Produces: a `.s2sp` containing a `gamedata.json` member; `manifest.json` gains `permissions: string[]`; `.s2script/gamedata.d.ts` written before the typecheck gate.

**Ordering requirement:** generate `.s2script/gamedata.d.ts` **before** the existing `tsc` gate runs, so a wrong arg count in plugin code fails the build.

- [ ] **Step 1: Write the failing test**

```js
// packages/sdk/test/gamedata-build.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, mkdirSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { buildPlugin } from "../dist/build.js";

function scaffold(gamedata, permissions, body) {
  const dir = mkdtempSync(join(tmpdir(), "s2gd-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  mkdirSync(join(dir, "gamedata"), { recursive: true });
  writeFileSync(join(dir, "gamedata", "plugin.gamedata.jsonc"), JSON.stringify(gamedata));
  writeFileSync(join(dir, "package.json"), JSON.stringify({
    name: "@demo/gd", version: "0.1.0", main: "src/plugin.ts",
    s2script: { gamedata: "gamedata/plugin.gamedata.jsonc", ...(permissions ? { permissions } : {}) },
  }));
  writeFileSync(join(dir, "src", "plugin.ts"), body);
  return dir;
}

const GD = {
  signatures: { Ig: { linuxsteamrt64: { module: "libserver.so", pattern: "55 48", resolve: "direct" } } },
  calls: { ignite: { receiver: { kind: "entity" }, target: { kind: "signature", name: "Ig" },
                     args: ["float"], returns: "void" } },
};
const OK_BODY = `import { plugin } from "@s2script/sdk/plugin";
import { Engine } from "@s2script/sdk/unsafe";
export default plugin(() => { const f = Engine.call("ignite"); void f; });`;

test("packs gamedata.json into the .s2sp and records permissions in the manifest", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  const out = await buildPlugin(dir);
  const zip = unzipSync(readFileSync(out));
  assert.ok(zip["gamedata.json"], "expected a gamedata.json member");
  const manifest = JSON.parse(Buffer.from(zip["manifest.json"]).toString("utf8"));
  assert.deepEqual(manifest.permissions, ["engine:calls"]);
  assert.ok(JSON.parse(Buffer.from(zip["gamedata.json"]).toString("utf8")).calls.ignite);
});

test("writes .s2script/gamedata.d.ts", async () => {
  const dir = scaffold(GD, ["engine:calls"], OK_BODY);
  await buildPlugin(dir);
  assert.ok(existsSync(join(dir, ".s2script", "gamedata.d.ts")));
});

test("a calls section without the permission fails the build", async () => {
  const dir = scaffold(GD, undefined, OK_BODY);
  await assert.rejects(() => buildPlugin(dir), /engine:calls/);
});

test("a wrong arg count fails the typecheck gate", async () => {
  const body = `import { plugin } from "@s2script/sdk/plugin";
import { Engine } from "@s2script/sdk/unsafe";
export default plugin(() => { const f = Engine.call("ignite"); if (f) f(null as never, 1, 2); });`;
  const dir = scaffold(GD, ["engine:calls"], body);
  await assert.rejects(() => buildPlugin(dir));
});

test("a plugin with no gamedata key still builds and packs no gamedata.json", async () => {
  const dir = mkdtempSync(join(tmpdir(), "s2gd-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "@demo/plain", version: "0.1.0", main: "src/plugin.ts", s2script: {} }));
  writeFileSync(join(dir, "src", "plugin.ts"), `import { plugin } from "@s2script/sdk/plugin";\nexport default plugin(() => {});`);
  const zip = unzipSync(readFileSync(await buildPlugin(dir)));
  assert.equal(zip["gamedata.json"], undefined);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/sdk && npm run build && node --test test/gamedata-build.test.mjs`
Expected: FAIL — no `gamedata.json` member.

- [ ] **Step 3: Implement the wiring in `build.ts`**

Read `packages/sdk/src/build.ts` first and follow its existing style. Insert, in this order:

1. Near the other `s2script.*` reads, load the gamedata (JSONC — strip comments with the same helper the repo already uses for `.jsonc`, or `JSON.parse` after comment-stripping):

```ts
import { validatePluginGamedata } from "./gamedata/validate.js";
import { generateGamedataTypes } from "./gamedata/gen-types.js";
import type { PluginGamedata } from "./gamedata/types.js";

const permissions: string[] = Array.isArray(s2.permissions) ? s2.permissions : [];
let gamedata: PluginGamedata | undefined;
if (typeof s2.gamedata === "string") {
  const gdPath = join(dir, s2.gamedata);
  const raw = readFileSync(gdPath, "utf8").replace(/^\s*\/\/.*$/gm, "");
  try {
    gamedata = JSON.parse(raw) as PluginGamedata;
  } catch (e) {
    throw new Error(`invalid gamedata ${gdPath}: ${(e as Error).message}`);
  }
  const gdErrs = validatePluginGamedata(gamedata, { permissions });
  if (gdErrs.length) throw new Error(`invalid gamedata:\n  ${gdErrs.join("\n  ")}`);
}
```

2. **Before** the typecheck gate, write the generated types:

```ts
if (gamedata) {
  const genDir = join(dir, ".s2script");
  mkdirSync(genDir, { recursive: true });
  writeFileSync(join(genDir, "gamedata.d.ts"), generateGamedataTypes(gamedata));
}
```

3. In the manifest derivation block, alongside `manifest.config`:

```ts
if (permissions.length > 0) manifest.permissions = permissions;
```

4. In the zip assembly, alongside the `types/` members:

```ts
if (gamedata) zipFiles["gamedata.json"] = Buffer.from(JSON.stringify(gamedata, null, 2));
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd packages/sdk && npm run build && node --test test/gamedata-build.test.mjs`
Expected: PASS (5 tests).

- [ ] **Step 5: Run the whole SDK suite for regressions**

Run: `cd packages/sdk && node --test test/`
Expected: no NEW failures. (13 CLI failures pre-exist on this branch — compare against `git stash`-free baseline if unsure.)

- [ ] **Step 6: Commit**

```bash
git add packages/sdk/src/build.ts packages/sdk/test/gamedata-build.test.mjs
git commit -m "Validate, generate types for, and pack plugin gamedata in s2s build"
```

---

### Task 5: Core — manifest fields, permissions allow-list, gamedata member

**Files:**
- Modify: `core/src/loader.rs` (`Manifest` + `read_s2sp`)
- Create: `core/config-templates/permissions.json`
- Test: in-file `#[cfg(test)]` in `core/src/loader.rs`

**Interfaces:**
- Produces: `Manifest.permissions: Vec<String>`, `Manifest.gamedata: Option<String>`; `read_s2sp` returns the optional raw `gamedata.json` text as a third tuple element: `read_s2sp(bytes) -> Result<(Manifest, String, Option<String>), String>`; `permission_allowed(plugin_id: &str, permission: &str) -> bool` (default-deny).

**Caller update required:** every existing `read_s2sp` call site must be updated for the new tuple arity. Find them with `rg 'read_s2sp\('`.

- [ ] **Step 1: Write the failing tests**

```rust
// append to the existing #[cfg(test)] mod in core/src/loader.rs
#[test]
fn manifest_parses_permissions_and_gamedata() {
    let bytes = make_test_s2sp(
        r#"{"id":"@demo/gd","version":"0.1.0","apiVersion":"2.x","permissions":["engine:calls"]}"#,
        "module.exports.default={__s2plugin:1};",
    );
    let (m, _js, gd) = read_s2sp(&bytes).expect("valid s2sp");
    assert_eq!(m.permissions, vec!["engine:calls".to_string()]);
    assert!(gd.is_none(), "no gamedata.json member in this archive");
}

#[test]
fn manifest_without_permissions_defaults_empty() {
    let bytes = make_test_s2sp(
        r#"{"id":"@demo/p","version":"0.1.0","apiVersion":"2.x"}"#,
        "module.exports.default={__s2plugin:1};",
    );
    let (m, _js, _gd) = read_s2sp(&bytes).expect("valid s2sp");
    assert!(m.permissions.is_empty());
}

#[test]
fn permission_is_default_deny() {
    // With no allow-list loaded, nothing is permitted.
    assert!(!permission_allowed("@demo/gd", "engine:calls"));
}

#[test]
fn permission_allowed_after_allow_list_load() {
    load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).expect("parses");
    assert!(permission_allowed("@demo/gd", "engine:calls"));
    assert!(!permission_allowed("@other/x", "engine:calls"));
    assert!(!permission_allowed("@demo/gd", "engine:other"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p s2script-core loader::tests`
Expected: FAIL — `permissions` field / `permission_allowed` not found; tuple arity mismatch.

- [ ] **Step 3: Implement**

Add to `Manifest`:

```rust
    /// Capabilities this plugin requests (spec §6). Declaration is necessary but NOT sufficient —
    /// an operator allow-list entry is also required (`permission_allowed`).
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Author-side path to the plugin's gamedata source. Informational only at runtime; the runtime
    /// consumes the packed `gamedata.json` member.
    #[serde(default)]
    pub gamedata: Option<String>,
```

Extend `read_s2sp` to also read an optional `gamedata.json` member (mirroring the existing
`manifest.json` read, but `Ok(None)` when `by_name` misses), and return it as the third element.

Add the host-global allow-list, following the `ADMINS_TEMPLATE`/`admins.json` pattern already in
`v8host.rs` (a `OnceLock`/`RwLock` static in `loader.rs` is fine):

```rust
static PERMISSIONS: std::sync::RwLock<Option<std::collections::HashMap<String, Vec<String>>>> =
    std::sync::RwLock::new(None);

/// Parse an operator allow-list: `{"engine:calls":["@me/burn"]}`. Exact-match ids, no globs (v1).
pub fn load_permissions_from_str(s: &str) -> Result<(), String> {
    let map: std::collections::HashMap<String, Vec<String>> =
        serde_json::from_str(s).map_err(|e| format!("permissions.json: {}", e))?;
    *PERMISSIONS.write().map_err(|_| "permissions lock poisoned".to_string())? = Some(map);
    Ok(())
}

/// Default-DENY: unloaded or absent allow-list permits nothing.
pub fn permission_allowed(plugin_id: &str, permission: &str) -> bool {
    PERMISSIONS
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.get(permission).is_some_and(|v| v.iter().any(|p| p == plugin_id))))
        .unwrap_or(false)
}
```

Create the template:

```json
{
  "engine:calls": []
}
```

Wire it into packaging the same way `admins.json` is (see `scripts/package-release.sh:140` and the
`ADMINS_TEMPLATE` `include_str!` in `v8host.rs`), and fix every `read_s2sp` call site.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p s2script-core loader::tests`
Expected: PASS. Note the `permission_*` tests share global state — if they interfere, run
`cargo test -p s2script-core` (the crate is already forced single-threaded via `.cargo/config.toml`).

- [ ] **Step 5: Commit**

```bash
git add core/src/loader.rs core/config-templates/permissions.json scripts/package-release.sh
git commit -m "Add manifest permissions, gamedata member, and the default-deny allow-list"
```

---

### Task 6: Shim — resolve + invoke

**Files:**
- Create: `shim/src/engine_calls.h`, `shim/src/engine_calls.cpp`
- Modify: `shim/CMakeLists.txt` (add the new source)

**Interfaces:**
- Consumes: `s2sig::ParsePattern`/`FindPattern` (`shim/src/sigscan.h`), the existing
  `IsAddressInServerText` helper and RTTI vtable-by-name resolver in `shim/src/s2script_mm.cpp`
  (`s2vtable::GetVTableByName`).
- Produces two C-ABI functions the ops in Task 7 point at:

```c
// Resolve a descriptor. Returns a call id >= 0, or -1 with a human reason in reasonOut.
int S2_EngineCallResolve(const char* kind, const char* module, const char* pattern,
                         const char* resolve, const char* className, int vtableIndex,
                         const char* prologue, char* reasonOut, int reasonCap);

// Invoke. gpKind[i]: 0=scalar, 1=entity((uint64)index<<32|serial), 2=string(index into strs),
//                    3=vector(index into vecs, 3 floats each)
// retKind: 0=void 1=bool 2=int 3=float 4=entity
// Returns 1 on success (retOut written), 0 on degrade.
int S2_EngineCallInvoke(int callId, int entIndex, int entSerial, int subObjOff,
                        const uint64_t* gp, const unsigned char* gpKind, int gpCount,
                        const double* fp, int fpCount,
                        const char* const* strs, const float* vecs,
                        int retKind, uint64_t* retOut);
```

**The invoke technique — read this before implementing.** Under SysV x86-64, integer-class and
float-class arguments are assigned to *independent* register sequences, so positional interleaving
does not matter. Therefore a single fixed max-arity prototype can call any descriptor in the closed
vocabulary: always pass all 5 GP + 8 xmm slots; the callee reads only what its real prototype
declares and ignores the rest. This avoids both a combinatorial switch and hand-written asm. Two
casts are needed, chosen by return class (`void`/`bool`/`int`/`entity` read `rax`; `float` reads
`xmm0`):

```cpp
using FnU64 = uint64_t (*)(void*, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           double, double, double, double, double, double, double, double);
using FnF32 = float    (*)(void*, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           double, double, double, double, double, double, double, double);
```

This is valid only for **non-variadic** callees — variadics are excluded by the spec (§4).

- [ ] **Step 1: Implement `engine_calls.h`**

```cpp
#pragma once
#include <cstdint>

extern "C" {
int S2_EngineCallResolve(const char* kind, const char* module, const char* pattern,
                         const char* resolve, const char* className, int vtableIndex,
                         const char* prologue, char* reasonOut, int reasonCap);
int S2_EngineCallInvoke(int callId, int entIndex, int entSerial, int subObjOff,
                        const uint64_t* gp, const unsigned char* gpKind, int gpCount,
                        const double* fp, int fpCount,
                        const char* const* strs, const float* vecs,
                        int retKind, uint64_t* retOut);
}
```

- [ ] **Step 2: Implement `engine_calls.cpp`**

Structure it as:

1. A `struct ResolvedCall { void* fn; }` and a `static std::vector<ResolvedCall> g_calls;` — the
   returned call id is the index.
2. `S2_EngineCallResolve`:
   - `kind == "signature"` → reuse the existing signature path (same `module`/`pattern`/`resolve`
     handling `LoadSignatures` consumers use). On `s2sig::kFail`, write `"signature did not match this build"` and return -1.
   - `kind == "vtable"` → RTTI-resolve the class vtable by name; bounds-check `vtableIndex`; read the
     slot; **require** `prologue` non-empty (write `"vtable target requires validate.prologue"` and
     return -1 if empty); `IsAddressInServerText(slotFn)` else `"resolved slot outside libserver .text"`;
     then match `prologue` via `s2sig::ParsePattern` + a direct compare at `slotFn`, else
     `"prologue mismatch (resolved slot is not the intended function)"`.
   - On success push to `g_calls` and return the index.
3. `S2_EngineCallInvoke`:
   - Resolve the receiver: index/serial → `CBaseEntity*` via the same helper `entity_subobj_vcall`
     already uses; if `subObjOff >= 0`, deref the sub-object pointer at that offset and use it as
     `this` (null → return 0).
   - Build `uint64_t g[5] = {0}` and `double f[8] = {0}`. Walk `gpKind`: `0` copies `gp[i]`; `1`
     unpacks index/serial and resolves to a `CBaseEntity*` (a stale ref becomes `0`/nullptr, which is
     a legitimate "no entity" arg); `2` stores `(uint64_t)strs[gp[i]]`; `3` stores
     `(uint64_t)&vecs[gp[i] * 3]`. Copy `fp` into `f`.
   - Call through `FnU64` (or `FnF32` when `retKind == 3`), passing all 13 slots.
   - `retKind == 4` (entity): the returned `void*` must **not** become a ref directly — read the
     entity's identity `CEntityHandle` from the returned pointer (the same identity read
     `__s2_handle_adopt`'s callers rely on) and write **that handle value** to `*retOut`. Core then
     runs it through the books-gated adopt path. A null/non-entity pointer writes `0`.
   - `retKind == 3`: store the `float` bit pattern via `memcpy` into `*retOut`.
   - Return 1.

- [ ] **Step 3: Add to the build**

Add `src/engine_calls.cpp` to the shim target's source list in `shim/CMakeLists.txt`, next to the
other `src/*.cpp` entries.

- [ ] **Step 4: Verify it compiles**

Run: `make shim`
Expected: builds with no new warnings.

- [ ] **Step 5: Commit**

```bash
git add shim/src/engine_calls.h shim/src/engine_calls.cpp shim/CMakeLists.txt
git commit -m "Add shim descriptor resolution and the SysV invoke thunk"
```

---

### Task 7: Core — descriptor registry, engine ops, and natives

**Files:**
- Create: `core/src/gamedata_calls.rs`
- Modify: `core/src/v8host.rs` (append two ops; register natives; extend the prelude)
- Modify: `core/src/lib.rs` (declare the module)
- Modify: `shim/src/s2script_mm.cpp` (populate the two new ops)
- Test: in-file `#[cfg(test)]` in `core/src/gamedata_calls.rs`

**Interfaces:**
- Consumes: `permission_allowed` (Task 5); `S2_EngineCallResolve`/`S2_EngineCallInvoke` (Task 6).
- Produces exactly two natives. **Registration is NOT a native** — core registers every descriptor
  in Rust at plugin load by parsing the packed `gamedata.json` (Task 5's third `read_s2sp` element),
  so JS never supplies a declaration:
  - `__s2_engine_call_ready(pluginId, callName) -> boolean` — true iff the descriptor resolved.
  - `__s2_engine_call_status(pluginId, callName) -> string` — `"available"` or a named reason.
  - `__s2_engine_call_invoke(pluginId, callName, selfIndex, selfId, argsArray) -> value` — core looks
    the descriptor up to obtain the call id, arg kinds, return kind, and (lazily) the `via` sub-object
    offset. JS passes only the receiver identity and the raw arg values.

Core owns all marshalling knowledge; the prelude stays a thin shim. This is why `via` can resolve
lazily at first invoke (spec §11) without JS involvement.

**ABI constraint:** append these two fields at the **end** of `S2EngineOps`, and populate them at the
matching end of the shim's initializer. Never insert.

```rust
pub type EngineCallResolveFn = extern "C" fn(
    kind: *const c_char, module: *const c_char, pattern: *const c_char, resolve: *const c_char,
    class_name: *const c_char, vtable_index: c_int, prologue: *const c_char,
    reason_out: *mut c_char, reason_cap: c_int) -> c_int;
pub type EngineCallInvokeFn = extern "C" fn(
    call_id: c_int, ent_index: c_int, ent_serial: c_int, subobj_off: c_int,
    gp: *const u64, gp_kind: *const u8, gp_count: c_int,
    fp: *const f64, fp_count: c_int,
    strs: *const *const c_char, vecs: *const f32,
    ret_kind: c_int, ret_out: *mut u64) -> c_int;
```

- [ ] **Step 1: Write the failing tests**

```rust
// core/src/gamedata_calls.rs — #[cfg(test)] mod
#[test]
fn registry_degrades_when_permission_denied() {
    let mut reg = CallRegistry::new();
    // No allow-list loaded → default-deny.
    reg.register("@demo/gd", "ignite", &decl_json(), /*ops_available=*/true);
    assert!(reg.call_id("@demo/gd", "ignite").is_none());
    assert!(reg.status("@demo/gd", "ignite").contains("not permitted"));
}

#[test]
fn registry_degrades_when_ops_absent() {
    crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
    let mut reg = CallRegistry::new();
    reg.register("@demo/gd", "ignite", &decl_json(), /*ops_available=*/false);
    assert!(reg.call_id("@demo/gd", "ignite").is_none());
    assert_eq!(reg.status("@demo/gd", "ignite"), "engine op unavailable");
}

#[test]
fn status_of_unknown_call_is_named() {
    let reg = CallRegistry::new();
    assert!(reg.status("@demo/gd", "nope").contains("not declared"));
}

#[test]
fn unload_drops_a_plugins_descriptors() {
    crate::loader::load_permissions_from_str(r#"{"engine:calls":["@demo/gd"]}"#).unwrap();
    let mut reg = CallRegistry::new();
    reg.register("@demo/gd", "ignite", &decl_json(), true);
    reg.drop_plugin("@demo/gd");
    assert!(reg.status("@demo/gd", "ignite").contains("not declared"));
}

#[test]
fn arg_classification_splits_int_and_float_slots() {
    // ["float","int","float"] -> 1 GP slot, 2 float slots, preserving per-class order.
    let (gp_kinds, fp_count) = classify_args(&["float".into(), "int".into(), "float".into()]);
    assert_eq!(gp_kinds.len(), 1);
    assert_eq!(fp_count, 2);
}

fn decl_json() -> String {
    r#"{"receiver":{"kind":"entity"},"target":{"kind":"signature","name":"Ig",
        "module":"libserver.so","pattern":"55 48","resolve":"direct"},
        "args":["float"],"returns":"void"}"#.to_string()
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p s2script-core gamedata_calls`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement `gamedata_calls.rs`**

Implement `CallRegistry` with:
- `new()`, `register(plugin_id, call_name, decl_json, ops_available)`, `call_id(plugin_id, name) -> Option<i32>`, `status(plugin_id, name) -> String`, `drop_plugin(plugin_id)`.
- `register` order of checks: (1) `permission_allowed(plugin_id, "engine:calls")` else status
  `"not permitted by the operator allow-list"`; (2) ops available else `"engine op unavailable"`;
  (3) call the resolve op, storing either the id or the shim's reason string.
- `classify_args(args: &[String]) -> (Vec<u8>, usize)` returning the per-GP-slot kind bytes
  (`0` scalar, `1` entity, `2` string, `3` vector) and the float-slot count, **preserving order
  within each class**.
- Keys: `(String, String)` → enum `Descriptor { Ready(i32), Degraded(String) }`.

Class/field/signature strings are passed straight through as opaque `CString`s — do **not** interpret
them in core.

- [ ] **Step 4: Register the natives and extend the prelude**

In `v8host.rs`: append the two op fields, add the three natives following the existing
`s2_entity_subobj_vcall` style (`catch_unwind`, `rv.set_*` default-first), register them in the
`set_native` block, and add a `__s2pkg_unsafe` prelude exposing:

```js
globalThis.__s2pkg_unsafe = {
  Engine: {
    call: function (name) {
      var pid = __s2_current_plugin();
      if (!__s2_engine_call_ready(pid, name)) return null;
      return function () {
        var args = Array.prototype.slice.call(arguments);
        var self = args.shift();
        if (!self) return null;
        return __s2_engine_call_invoke(pid, name, self.index, self.id, args);
      };
    },
    status: function (name) { return __s2_engine_call_status(__s2_current_plugin(), name); },
  },
};
```

The readiness check happens once inside `call()`; the returned closure captures the plugin id and
call name. Note it captures `pid` at `call()` time — correct, because `call()` runs inside the
plugin's own factory.

Also extend the plugin-load path to register descriptors: after `read_s2sp` yields the optional
`gamedata.json` text, parse its `calls` map and invoke `CallRegistry::register` once per entry, and
call `drop_plugin` on unload from the same place the ledger tears other per-plugin state down.

- [ ] **Step 5: Populate the ops in the shim**

In `shim/src/s2script_mm.cpp`, `#include "engine_calls.h"` and assign the two fields at the **end** of
the `S2EngineOps` initializer, matching the Rust field order exactly.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p s2script-core` then `make core && make shim`
Expected: PASS; both build.

- [ ] **Step 7: Verify the boundary gate**

Run: `make check-boundary`
Expected: PASS — no game identifiers in `core/`.

- [ ] **Step 8: Commit**

```bash
git add core/src/gamedata_calls.rs core/src/v8host.rs core/src/lib.rs shim/src/s2script_mm.cpp
git commit -m "Add the core call registry, engine ops, and Engine.call natives"
```

---

### Task 8: RE spike — resolve a real target signature

**Files:**
- Create: `docs/re-notes/2026-07-24-ignite.md`
- Binary: `/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so`

**Interfaces:**
- Produces: a verified `module`/`pattern`/`resolve` triple for one entity-receiver function with a
  known arg list, for use by Task 9. Prefer `CBaseEntity::Ignite`; if it does not exist as a callable
  member on this build, pick any confirmable entity-receiver function and record why.

Per `docs/re-strategy.md`: self-resolve against **our** binary. A borrowed pattern is a HINT.

- [ ] **Step 1: Locate candidate functions**

```bash
BIN=/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so
nm -DC "$BIN" 2>/dev/null | grep -iE 'Ignite' | head
strings -t x "$BIN" | grep -iE 'ignite' | head
```

- [ ] **Step 2: Disassemble the candidate and capture its prologue**

```bash
objdump -dC --start-address=0x<addr> --stop-address=0x<addr+64> "$BIN" | head -30
```

Record the first ~20 bytes as a masked pattern (`??` for relocation-varying bytes).

- [ ] **Step 3: Prove the pattern is UNIQUE**

```bash
python3 - <<'EOF'
import re
BIN="/home/gkh/projects/s2script/docker/cs2-data/game/csgo/bin/linuxsteamrt64/libserver.so"
pat="55 48 89 E5 ..."   # replace with the captured pattern
toks=pat.split()
data=open(BIN,'rb').read()
rx=b''.join(b'.' if t in ('?','??') else re.escape(bytes([int(t,16)])) for t in toks)
print("matches:", len(re.findall(rx, data, re.S)))
EOF
```

Expected: exactly **1**. If more, extend the pattern until unique.

- [ ] **Step 4: Write the RE note**

Record: the function, its address on this pinned build, the unique pattern, the arg list with types,
the return type, how uniqueness was proven, and the CS2 build number. Follow the commenting depth
already used in `gamedata/core.gamedata.jsonc`.

- [ ] **Step 5: Commit**

```bash
git add docs/re-notes/2026-07-24-ignite.md
git commit -m "Record the RE spike for the demo engine call"
```

---

### Task 9: Demo plugin + live gate

**Files:**
- Create: `examples/engine-call-demo/package.json`, `gamedata/plugin.gamedata.jsonc`, `src/plugin.ts`
- Modify: `docs/PROGRESS.md` (append the finished-slice entry)

**Interfaces:**
- Consumes: everything above, plus Task 8's verified pattern.

- [ ] **Step 1: Write the demo plugin**

`package.json`:

```json
{
  "name": "@demo/engine-call",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": {
    "gamedata": "gamedata/plugin.gamedata.jsonc",
    "permissions": ["engine:calls"]
  }
}
```

`gamedata/plugin.gamedata.jsonc` — use the **verified** values from Task 8 (not placeholders).

`src/plugin.ts`:

```ts
import { plugin } from "@s2script/sdk/plugin";
import { Engine } from "@s2script/sdk/unsafe";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Player } from "@s2script/cs2";

export default plugin((ctx) => {
  const ignite = Engine.call("ignite");
  console.log(`[engine-call-demo] ignite: ${Engine.status("ignite")}`);

  ctx.commands.registerAdmin("sm_burn", ADMFLAG.SLAY, (cmd) => {
    if (!ignite) return cmd.reply(`sm_burn unavailable: ${Engine.status("ignite")}`);
    for (const p of Player.target(cmd.arg(1), cmd.callerSlot)) {
      const pawn = p.pawn;
      if (pawn?.isValid) ignite(pawn.ref, 10.0, false, 0.0, false);
    }
  });
});
```

- [ ] **Step 2: Build it**

Run: `cd examples/engine-call-demo && npx @s2script/sdk build`
Expected: a `.s2sp` in `dist/`; `.s2script/gamedata.d.ts` generated.

- [ ] **Step 3: Confirm the negative-validation case**

Add a second call to the gamedata using the documented ItemServices vtable index 24 with a
deliberately wrong `prologue`. Rebuild, deploy, and confirm the load log names a
`prologue mismatch` reason for that descriptor **while `ignite` still resolves**. This is spec
success-criterion #4.

- [ ] **Step 4: Deploy and gate live**

Build the deployable binaries (host glibc is too new — this MUST be the sniper container):

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
./scripts/package-release.sh
```

Add `@demo/engine-call` to `addons/s2script/configs/permissions.json` under `engine:calls`, deploy the
addon + plugin, then:

```bash
docker compose -f docker/docker-compose.yml restart cs2
python3 scripts/rcon.py "sm_burn @me"
```

Expected: the pawn ignites. Then remove the allow-list entry, restart, and confirm `sm_burn` replies
`not permitted by the operator allow-list` — spec criterion #5.

Note: `docker/cs2-data` and `docker/metamod` are gitignored and absent in a fresh worktree; symlink
them from `/home/gkh/projects/s2script/docker/` before deploying.

- [ ] **Step 5: Full gate**

Run: `make ci` and `CI=1 make ci-js`
Expected: green. A fresh worktree needs `git submodule update --init --recursive` first (already done).

- [ ] **Step 6: Commit and open the PR**

```bash
git add examples/engine-call-demo docs/PROGRESS.md
git commit -m "Add the engine-call demo plugin and log the slice"
```

Write the PR body to a file and use `gh pr edit N --body-file` (never a heredoc — it mangles tables).
The body must include **Why**.

---

## Self-Review

**Spec coverage:** §3 failure posture → Tasks 1, 6. §4 call surface/arg budget → Tasks 1, 6, 7.
§5 descriptor format → Task 1. §6 permissions → Tasks 1, 4, 5. §7 package format → Tasks 4, 5.
§8 type generation → Tasks 2, 4. §9 plugin API → Tasks 3, 7. §10 layering → Tasks 6, 7 (+
`check-boundary` in Task 7 Step 7). §11 resolution timing → Task 7 (`via` resolves at invoke).
§12 error handling → Tasks 1 (build), 6/7 (load), 6 (call), 7 (unload). §13 testing → Tasks 1, 2, 4,
5, 7, 9. §15 criteria 1–7 → Task 9 covers 1/2/4/5, Task 1 covers 3, Task 7 Step 7 covers 6,
Task 5 covers 7.

**Known deviation:** the spec's §13 "deliberately-corrupted prologue" live check is folded into
Task 9 Step 3 rather than its own task, since it shares the demo plugin's build/deploy cycle.

**Fixed during review:** the native contract was inconsistent — the prelude called
`__s2_engine_call_resolve(pid, name)` while the interface block declared a three-arg form taking a
declaration JSON. Registration actually belongs in Rust at load (from the packed `gamedata.json`), so
the natives are now `__s2_engine_call_ready` / `_status` / `_invoke`, and descriptor registration is an
internal `CallRegistry::register` call on the load path, not a native.

**Type consistency:** `validatePluginGamedata(gd, {permissions})` → used identically in Task 4.
`generateGamedataTypes(gd)` → used identically in Task 4. `read_s2sp` three-tuple → Task 5 flags the
call-site update. `permission_allowed(plugin_id, permission)` → consumed in Task 7.
`S2_EngineCallResolve`/`Invoke` C signatures in Task 6 match the Rust `extern "C"` types in Task 7
field-for-field. `classify_args` returns `(Vec<u8>, usize)` in both its test and its description.
