# Slice 5B.3 — Schema Codegen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `s2script gen-schema` CLI subcommand that transforms the committed `games/cs2/gamedata/schema-catalog.json` into typed field accessors for a curated set of CS2 entity classes, so authors write `pawn.health` / `pawn.friction` (idiomatic typed props) instead of raw `__s2_schema_offset` + `EntityRef.read*` plumbing.

**Architecture:** A pure pipeline in `packages/cli/src/schemagen/` — `model.ts` (catalog + curated list → normalized per-class descriptors: name transform, type→accessor-kind map, parent-chain flatten, skip-with-reason, collision→raw fallback), `emit-dts.ts` (model → `.d.ts` with an `extends` chain), `emit-js.ts` (model → runtime getter/setter descriptors resolving offsets live via `__s2_schema_offset`). `gen.ts` wires it to files; `cli.ts` adds the `gen-schema` command. Two **committed** outputs (`games/cs2/js/schema.generated.js`, `packages/cs2/schema.generated.d.ts`); `pawn.js` consumes the runtime accessors; `package-addon.sh` concatenates them ahead of `pawn.js`. A freshness gate diffs a fresh generation against the committed files.

**Tech Stack:** TypeScript (ESM, `node --experimental-strip-types`, `node:test` + `node:assert`), esbuild (existing CLI bundler). **No Rust/core/shim changes** — the codegen lives entirely in the game-package + tooling layer.

## Global Constraints

Every task's requirements implicitly include these (from spec §11):

- **Core stays engine-generic.** The codegen, curated list, and all generated CS2 accessors live ONLY in `packages/cli` (tooling) + `games/cs2` + `packages/cs2`. NOTHING CS2 enters `core/src`. Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Layout is data.** Generated code resolves offsets **live** via `__s2_schema_offset("<declaringClass>","<rawName>")` — it NEVER embeds an offset number. The catalog's recorded `offset` is reference-only; do not emit it.
- **Never expose a raw pointer across time.** Generated getters return `T | null` via the serial-gated `EntityRef`; handle fields return `EntityRef | null`. No pointer/offset escapes to author code.
- **Degrade-never-crash.** A field the generator can't safely back is SKIPPED with a logged reason (never emitted broken); a requested class absent from the catalog is a HARD gen error naming the class; a missing field at runtime → `__s2_schema_offset` returns `< 0` → the accessor returns `null`.
- **Deterministic output.** Same catalog + list → byte-identical `.js` and `.d.ts` (the freshness gate depends on this). No timestamps, no `Date`, no map-iteration-order nondeterminism; iterate arrays in catalog order and classes in a stable topological order.
- **Naming:** PascalCase generated interface names (schema class names verbatim: `CCSPlayerPawn`); camelCase idiomatic props (`friction`, `health`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5b3-schema-codegen`; do NOT push.

**Deferred — do NOT build:** `enum` accessors (catalog lacks enum byte-width), the `Vector`/`QAngle` value type + Vector fields, strings, embedded/`ptr`/nested accessors, typed wrappers for handle-referenced classes (handle → bare `EntityRef`), `i64`/`u64` (BigInt), the player-model / `Player`/`fromClient` rework + SM-parity entry points (5C/6), the `tsc` typecheck GATE (generate the `.d.ts`, don't build the enforcement gate), the `@s2script/std` module split (5C), config/permissions, registry (5.5), base suite (6).

---

## Catalog shape (the input, for reference)

`games/cs2/gamedata/schema-catalog.json` is a top-level map `className → { parent: string|null, fields: [{ name, offset, type: { kind, name?, inner? } }] }`. `fields` are the class's OWN fields; inheritance is the `parent` chain. `type.kind` ∈ `atomic|class|enum|handle|ptr|unknown`; for `atomic`, `type.name` is the subtype (`float32`,`bool`,`int32`,`uint8`,`CUtlSymbolLarge`,`Vector`,…); for `handle`, `type.inner` is the referenced class.

## File Structure

- **Create** `packages/cli/src/schemagen/model.ts` — types + pure model builder + shared method/type tables.
- **Create** `packages/cli/src/schemagen/emit-dts.ts` — `.d.ts` emitter.
- **Create** `packages/cli/src/schemagen/emit-js.ts` — runtime `.js` emitter.
- **Create** `packages/cli/src/schemagen/gen.ts` — file I/O: read catalog + list, run model + emitters, write or `--check`.
- **Create** `games/cs2/codegen-classes.json` — the curated class list (hand-maintained authoring input).
- **Generate (committed)** `games/cs2/js/schema.generated.js`, `packages/cs2/schema.generated.d.ts`.
- **Modify** `packages/cli/src/cli.ts` (add `gen-schema` command), `packages/cli/package.json` (test glob), `games/cs2/js/pawn.js` (delete hand-written `health`; apply generated accessors), `packages/cs2/index.d.ts` (re-export generated), `scripts/package-addon.sh` (concatenate), `README.md`, `CLAUDE.md`.
- **Create** `scripts/check-schema-generated.sh` — freshness gate.
- **Tests** `packages/cli/test/schemagen-model.test.mjs`, `…/schemagen-emit.test.mjs`, `…/schemagen-determinism.test.mjs`, `…/schema-runtime.test.mjs`.

---

## Task 1: The pure model (`model.ts`)

**Files:**
- Create: `packages/cli/src/schemagen/model.ts`
- Test: `packages/cli/test/schemagen-model.test.mjs`

**Interfaces:**
- Consumes: nothing (pure).
- Produces (used by Tasks 2–4):
  - `type Catalog = Record<string, { parent: string | null; fields: CatalogField[] }>`
  - `interface CatalogField { name: string; offset: number; type: { kind: string; name?: string; inner?: string } }`
  - `type AccessorKind = "f32"|"bool"|"i8"|"i16"|"i32"|"u8"|"u16"|"u32"|"handle"`
  - `interface FieldDescriptor { propName: string; rawName: string; declaringClass: string; accessorKind: AccessorKind; writable: boolean }`
  - `interface SkippedField { className: string; rawName: string; reason: string }`
  - `interface ClassDescriptor { className: string; parent: string | null; ownFields: FieldDescriptor[]; skipped: SkippedField[] }`
  - `interface SchemaModel { classes: ClassDescriptor[]; collisions: string[] }`
  - `function idiomaticName(raw: string): string`
  - `function classifyField(type: CatalogField["type"]): { accessorKind: AccessorKind; writable: boolean } | { skip: string }`
  - `function buildModel(catalog: Catalog, requested: string[]): SchemaModel`
  - `function flattenedFields(model: SchemaModel, className: string): FieldDescriptor[]`
  - `const READ: Record<AccessorKind, string>`, `const WRITE: Partial<Record<AccessorKind, string>>`, `const TSTYPE: Record<AccessorKind, string>`

- [ ] **Step 1: Write the failing tests** (`packages/cli/test/schemagen-model.test.mjs`):

```js
import { test } from "node:test";
import assert from "node:assert";
import { idiomaticName, classifyField, buildModel, flattenedFields } from "../src/schemagen/model.ts";

test("idiomaticName strips m_ + Hungarian tag, camelCases", () => {
  assert.equal(idiomaticName("m_iHealth"), "health");
  assert.equal(idiomaticName("m_flFriction"), "friction");
  assert.equal(idiomaticName("m_hController"), "controller");
  assert.equal(idiomaticName("m_bClientSideRagdoll"), "clientSideRagdoll");
  assert.equal(idiomaticName("m_ArmorValue"), "armorValue");   // no lowercase tag
  assert.equal(idiomaticName("m_flags"), "flags");             // all-lowercase, no uppercase boundary → unchanged
});

test("classifyField maps in-scope kinds, skips the rest with a reason", () => {
  assert.deepEqual(classifyField({ kind: "atomic", name: "float32" }), { accessorKind: "f32", writable: true });
  assert.deepEqual(classifyField({ kind: "atomic", name: "bool" }), { accessorKind: "bool", writable: true });
  assert.deepEqual(classifyField({ kind: "atomic", name: "int32" }), { accessorKind: "i32", writable: true });
  assert.deepEqual(classifyField({ kind: "atomic", name: "uint8" }), { accessorKind: "u8", writable: false });
  assert.deepEqual(classifyField({ kind: "handle", inner: "CBaseEntity" }), { accessorKind: "handle", writable: false });
  assert.ok("skip" in classifyField({ kind: "enum", name: "Team_t" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "CUtlSymbolLarge" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "Vector" }));
  assert.ok("skip" in classifyField({ kind: "atomic", name: "uint64" }));
  assert.ok("skip" in classifyField({ kind: "class", name: "CTransform" }));
  assert.ok("skip" in classifyField({ kind: "ptr" }));
  assert.ok("skip" in classifyField({ kind: "unknown" }));
});

test("buildModel: closure includes ancestors, own fields per class, skips logged, parent flatten", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_vecStuff", offset: 12, type: { kind: "atomic", name: "Vector" } },   // skipped
    ] },
    Mid: { parent: "Base", fields: [
      { name: "m_hOwner", offset: 20, type: { kind: "handle", inner: "Base" } },
    ] },
    Leaf: { parent: "Mid", fields: [
      { name: "m_flSpeed", offset: 24, type: { kind: "atomic", name: "float32" } },
    ] },
  };
  const m = buildModel(catalog, ["Leaf"]);
  // closure = Base, Mid, Leaf ; topo order root→leaf
  assert.deepEqual(m.classes.map(c => c.className), ["Base", "Mid", "Leaf"]);
  const base = m.classes.find(c => c.className === "Base");
  assert.deepEqual(base.ownFields.map(f => f.propName), ["health"]);      // Vector skipped
  assert.equal(base.ownFields[0].declaringClass, "Base");
  assert.equal(base.ownFields[0].writable, true);
  assert.equal(base.skipped.length, 1);
  assert.equal(base.skipped[0].rawName, "m_vecStuff");
  // flatten Leaf = Base.health + Mid.owner + Leaf.speed (root→leaf)
  assert.deepEqual(flattenedFields(m, "Leaf").map(f => f.propName), ["health", "owner", "speed"]);
  assert.equal(flattenedFields(m, "Leaf").find(f => f.propName === "owner").accessorKind, "handle");
});

test("buildModel: idiomatic-name collision across distinct fields → both fall back to raw", () => {
  const catalog = {
    Base: { parent: null, fields: [
      { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
      { name: "m_flHealth", offset: 12, type: { kind: "atomic", name: "float32" } },   // also → "health"
    ] },
  };
  const m = buildModel(catalog, ["Base"]);
  const names = m.classes[0].ownFields.map(f => f.propName).sort();
  assert.deepEqual(names, ["m_flHealth", "m_iHealth"]);   // both raw-fallback
  assert.equal(m.collisions.length, 1);
});

test("buildModel: a requested class absent from the catalog is a hard error", () => {
  assert.throws(() => buildModel({ Base: { parent: null, fields: [] } }, ["Nope"]), /Nope/);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-model.test.mjs`
Expected: FAIL — module `../src/schemagen/model.ts` not found.

- [ ] **Step 3: Implement** `packages/cli/src/schemagen/model.ts`:

```ts
// Pure model: catalog + curated list → normalized per-class accessor descriptors.
// No I/O, no Date/random — deterministic. See the plan's Global Constraints.

export type Catalog = Record<string, { parent: string | null; fields: CatalogField[] }>;
export interface CatalogField {
  name: string;
  offset: number;
  type: { kind: string; name?: string; inner?: string };
}

export type AccessorKind = "f32" | "bool" | "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "handle";

export interface FieldDescriptor {
  propName: string;
  rawName: string;
  declaringClass: string;
  accessorKind: AccessorKind;
  writable: boolean;
}
export interface SkippedField { className: string; rawName: string; reason: string; }
export interface ClassDescriptor { className: string; parent: string | null; ownFields: FieldDescriptor[]; skipped: SkippedField[]; }
export interface SchemaModel { classes: ClassDescriptor[]; collisions: string[]; }

// AccessorKind → EntityRef method (5B.2 surface) + TS type. Writable ⇔ a WRITE entry exists.
export const READ: Record<AccessorKind, string> = {
  f32: "readFloat32", bool: "readBool", i8: "readInt8", i16: "readInt16",
  i32: "readInt32", u8: "readUInt8", u16: "readUInt16", u32: "readUInt32", handle: "readHandle",
};
export const WRITE: Partial<Record<AccessorKind, string>> = { f32: "writeFloat32", bool: "writeBool", i32: "writeInt32" };
export const TSTYPE: Record<AccessorKind, string> = {
  f32: "number | null", bool: "boolean | null", i8: "number | null", i16: "number | null",
  i32: "number | null", u8: "number | null", u16: "number | null", u32: "number | null", handle: "EntityRef | null",
};

// atomic subtype → (kind, writable). Only genuine scalars; everything else falls through to skip.
const ATOMIC: Record<string, { k: AccessorKind; w: boolean }> = {
  float32: { k: "f32", w: true }, bool: { k: "bool", w: true },
  int8: { k: "i8", w: false }, int16: { k: "i16", w: false }, int32: { k: "i32", w: true },
  uint8: { k: "u8", w: false }, uint16: { k: "u16", w: false }, uint32: { k: "u32", w: false },
};

export function idiomaticName(raw: string): string {
  const s = raw.replace(/^m_/, "");
  const m = s.match(/^[a-z]+([A-Z].*)$/);   // leading lowercase Hungarian tag, then an Uppercase-led core
  const core = m ? m[1] : s;
  return core.charAt(0).toLowerCase() + core.slice(1);
}

export function classifyField(type: CatalogField["type"]): { accessorKind: AccessorKind; writable: boolean } | { skip: string } {
  if (type.kind === "handle") return { accessorKind: "handle", writable: false };
  if (type.kind === "atomic") {
    const m = ATOMIC[type.name ?? ""];
    if (m) return { accessorKind: m.k, writable: m.w };
    return { skip: `atomic '${type.name}' is not a scalar (string/vector/compound/64-bit)` };
  }
  if (type.kind === "enum") return { skip: "enum byte-width absent from catalog (deferred)" };
  if (type.kind === "class") return { skip: `embedded class '${type.name ?? ""}' deferred` };
  if (type.kind === "ptr") return { skip: "raw pointer" };
  return { skip: `unmapped kind '${type.kind}'` };
}

export function flattenedFields(model: SchemaModel, className: string): FieldDescriptor[] {
  const byName = new Map(model.classes.map((c) => [c.className, c]));
  const chain: ClassDescriptor[] = [];
  let cur: string | null = className;
  while (cur && byName.has(cur)) { const c = byName.get(cur)!; chain.unshift(c); cur = c.parent; }
  return chain.flatMap((c) => c.ownFields);
}

export function buildModel(catalog: Catalog, requested: string[]): SchemaModel {
  // 1. Closure: requested + ancestor chains (stop at null parent or a parent absent from the catalog).
  const inClosure = new Set<string>();
  for (const start of requested) {
    if (!catalog[start]) throw new Error(`gen-schema: requested class '${start}' is not in the catalog`);
    let cur: string | null = start;
    while (cur && catalog[cur] && !inClosure.has(cur)) { inClosure.add(cur); cur = catalog[cur].parent; }
  }
  // 2. Stable topological order: by depth-to-root, ties by name.
  const depth = (c: string): number => { let d = 0, cur: string | null = c; while (cur && catalog[cur]?.parent && inClosure.has(catalog[cur]!.parent!)) { d++; cur = catalog[cur]!.parent; } return d; };
  const ordered = [...inClosure].sort((a, b) => depth(a) - depth(b) || (a < b ? -1 : a > b ? 1 : 0));
  // 3. Per class: classify own fields.
  const classes: ClassDescriptor[] = ordered.map((className) => {
    const parent = catalog[className].parent;
    const ownFields: FieldDescriptor[] = [];
    const skipped: SkippedField[] = [];
    for (const f of catalog[className].fields) {
      const c = classifyField(f.type);
      if ("skip" in c) { skipped.push({ className, rawName: f.name, reason: c.skip }); continue; }
      ownFields.push({ propName: idiomaticName(f.name), rawName: f.name, declaringClass: className, accessorKind: c.accessorKind, writable: c.writable });
    }
    return { className, parent: parent && inClosure.has(parent) ? parent : null, ownFields, skipped };
  });
  // 4. Collision pass: an idiomatic propName shared by ≥2 distinct fields (by declaringClass+rawName) → raw fallback for all.
  const byProp = new Map<string, FieldDescriptor[]>();
  for (const c of classes) for (const f of c.ownFields) { (byProp.get(f.propName) ?? byProp.set(f.propName, []).get(f.propName)!).push(f); }
  const collisions: string[] = [];
  for (const [prop, fields] of byProp) {
    const distinct = new Set(fields.map((f) => `${f.declaringClass}.${f.rawName}`));
    if (distinct.size >= 2) { for (const f of fields) f.propName = f.rawName; collisions.push(`${prop} ← ${[...distinct].sort().join(", ")}`); }
  }
  collisions.sort();
  return { classes, collisions };
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-model.test.mjs`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/schemagen/model.ts packages/cli/test/schemagen-model.test.mjs
git commit -m "feat(slice5b3): pure schema-codegen model (name transform, type map, flatten, collision)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: The `.d.ts` emitter (`emit-dts.ts`)

**Files:**
- Create: `packages/cli/src/schemagen/emit-dts.ts`
- Test: `packages/cli/test/schemagen-emit.test.mjs` (shared with Task 3 — create it here, Task 3 appends)

**Interfaces:**
- Consumes: `SchemaModel`, `TSTYPE` from `./model.ts` (Task 1).
- Produces: `function emitDts(model: SchemaModel): string`.

- [ ] **Step 1: Write the failing test** (create `packages/cli/test/schemagen-emit.test.mjs`):

```js
import { test } from "node:test";
import assert from "node:assert";
import { buildModel } from "../src/schemagen/model.ts";
import { emitDts } from "../src/schemagen/emit-dts.ts";

const CATALOG = {
  Base: { parent: null, fields: [
    { name: "m_iHealth", offset: 8, type: { kind: "atomic", name: "int32" } },
    { name: "m_flFriction", offset: 12, type: { kind: "atomic", name: "float32" } },
    { name: "m_vecOrigin", offset: 16, type: { kind: "atomic", name: "Vector" } },   // skipped
  ] },
  Leaf: { parent: "Base", fields: [
    { name: "m_hController", offset: 24, type: { kind: "handle", inner: "Base" } },
    { name: "m_bScoped", offset: 28, type: { kind: "atomic", name: "bool" } },
  ] },
};

test("emitDts: extends chain, own fields only, writable vs readonly, skipped absent", () => {
  const dts = emitDts(buildModel(CATALOG, ["Leaf"]));
  assert.match(dts, /import type \{ EntityRef \} from "@s2script\/std";/);
  assert.match(dts, /export interface Base \{/);
  assert.match(dts, /health: number \| null;/);       // writable → mutable
  assert.match(dts, /friction: number \| null;/);
  assert.match(dts, /export interface Leaf extends Base \{/);
  assert.match(dts, /readonly controller: EntityRef \| null;/);  // handle → readonly
  assert.match(dts, /scoped: boolean \| null;/);
  assert.doesNotMatch(dts, /origin/);                  // Vector skipped
  assert.doesNotMatch(dts, /m_vecOrigin/);
});
```

- [ ] **Step 2: Run to verify failure** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-emit.test.mjs` → FAIL (`emit-dts.ts` missing).

- [ ] **Step 3: Implement** `packages/cli/src/schemagen/emit-dts.ts`:

```ts
import type { SchemaModel } from "./model.ts";
import { TSTYPE } from "./model.ts";

const HEADER = "// GENERATED by `s2script gen-schema` from schema-catalog.json — DO NOT EDIT.";

export function emitDts(model: SchemaModel): string {
  const names = new Set(model.classes.map((c) => c.className));
  const out: string[] = [HEADER, 'import type { EntityRef } from "@s2script/std";', ""];
  for (const c of model.classes) {
    const ext = c.parent && names.has(c.parent) ? ` extends ${c.parent}` : "";
    out.push(`export interface ${c.className}${ext} {`);
    for (const f of c.ownFields) {
      const ro = f.writable ? "" : "readonly ";
      out.push(`  ${ro}${f.propName}: ${TSTYPE[f.accessorKind]};`);
    }
    out.push("}", "");
  }
  return out.join("\n");
}
```

- [ ] **Step 4: Run to verify pass** — same command → PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/schemagen/emit-dts.ts packages/cli/test/schemagen-emit.test.mjs
git commit -m "feat(slice5b3): .d.ts emitter (extends chain, readonly for non-writable, skipped absent)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: The runtime `.js` emitter (`emit-js.ts`)

**Files:**
- Create: `packages/cli/src/schemagen/emit-js.ts`
- Test: append to `packages/cli/test/schemagen-emit.test.mjs`

**Interfaces:**
- Consumes: `SchemaModel`, `flattenedFields`, `READ`, `WRITE` from `./model.ts`.
- Produces: `function emitJs(model: SchemaModel): string`. The emitted source, when evaluated in the raw context, sets `globalThis.__s2pkg_cs2_schema = { applyAccessors(proto, className), wrap(className, ref) }`.

- [ ] **Step 1: Write the failing test** (append to `schemagen-emit.test.mjs`):

```js
import { emitJs } from "../src/schemagen/emit-js.ts";
import vm from "node:vm";

test("emitJs: flattened getters/setters, live off() resolve, notifyStateChanged on write", () => {
  const js = emitJs(buildModel(CATALOG, ["Leaf"]));
  // getter reads via the declaring class + raw name, resolved live:
  assert.match(js, /readInt32\(off\("Base","m_iHealth"\)\)/);
  assert.match(js, /readFloat32\(off\("Base","m_flFriction"\)\)/);
  assert.match(js, /readHandle\(off\("Leaf","m_hController"\)\)/);
  assert.doesNotMatch(js, /m_vecOrigin/);   // skipped field absent
  // NO baked offset numbers (layout-is-data): the reference offsets 8/12/24/28 must not appear as read args
  assert.doesNotMatch(js, /readInt32\(\s*8\s*\)/);

  // Evaluate in a sandbox with stub natives; assert the accessors work + writes notify.
  const reads = [];
  const writes = [];
  const notified = [];
  const ctx = {
    globalThis: {},
    __s2_schema_offset: (cls, field) => 100,   // any non-negative offset
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(js, ctx);
  const schema = ctx.__s2pkg_cs2_schema;
  assert.equal(typeof schema.applyAccessors, "function");
  const ref = {
    readInt32: (o) => { reads.push(o); return 100; },
    readFloat32: () => 0.25,
    readBool: () => true,
    readHandle: () => ({ index: 1, serial: 7 }),
    writeInt32: (o, v) => { writes.push([o, v]); return true; },
    writeFloat32: () => true, writeBool: () => true,
    notifyStateChanged: (o) => { notified.push(o); },
  };
  const pawn = schema.wrap("Leaf", ref);
  assert.equal(pawn.health, 100);            // getter, flattened from Base
  assert.equal(pawn.friction, 0.25);
  assert.deepEqual(pawn.controller, { index: 1, serial: 7 });
  pawn.health = 55;                          // writable → write + notify
  assert.deepEqual(writes, [[100, 55]]);
  assert.deepEqual(notified, [100]);
});
```

- [ ] **Step 2: Run to verify failure** → FAIL (`emit-js.ts` missing).

- [ ] **Step 3: Implement** `packages/cli/src/schemagen/emit-js.ts`:

```ts
import type { SchemaModel } from "./model.ts";
import { flattenedFields, READ, WRITE } from "./model.ts";

const HEADER = "// GENERATED by `s2script gen-schema` from schema-catalog.json — DO NOT EDIT.";
const S = JSON.stringify;

export function emitJs(model: SchemaModel): string {
  const out: string[] = [HEADER, "(function () {", "  var off = __s2_schema_offset;", "  var A = {};"];
  for (const c of model.classes) {
    out.push(`  A[${S(c.className)}] = {`);
    for (const f of flattenedFields(model, c.className)) {
      const resolve = `off(${S(f.declaringClass)}, ${S(f.rawName)})`;
      let entry = `get: function () { return this.ref.${READ[f.accessorKind]}(${resolve}); }`;
      if (f.writable) {
        const wm = WRITE[f.accessorKind]!;
        entry += `, set: function (v) { var o = ${resolve}; if (this.ref.${wm}(o, v)) this.ref.notifyStateChanged(o); }`;
      }
      out.push(`    ${S(f.propName)}: { ${entry} },`);
    }
    out.push("  };");
  }
  out.push(
    "  function applyAccessors(proto, className) {",
    "    var defs = A[className]; if (!defs) return;",
    "    for (var name in defs) {",
    "      Object.defineProperty(proto, name, { get: defs[name].get, set: defs[name].set, enumerable: true, configurable: true });",
    "    }",
    "  }",
    "  function wrap(className, ref) { var o = { ref: ref }; applyAccessors(o, className); return o; }",
    "  globalThis.__s2pkg_cs2_schema = { applyAccessors: applyAccessors, wrap: wrap };",
    "})();",
  );
  return out.join("\n");
}
```

- [ ] **Step 4: Run to verify pass** → PASS (both emit tests).

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/schemagen/emit-js.ts packages/cli/test/schemagen-emit.test.mjs
git commit -m "feat(slice5b3): runtime .js emitter (live off() resolve, flattened accessors, notify on write)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: `gen-schema` CLI + curated list + generate committed outputs + freshness gate

**Files:**
- Create: `packages/cli/src/schemagen/gen.ts`, `games/cs2/codegen-classes.json`, `scripts/check-schema-generated.sh`
- Create (generated, committed): `games/cs2/js/schema.generated.js`, `packages/cs2/schema.generated.d.ts`
- Modify: `packages/cli/src/cli.ts`, `packages/cli/package.json`
- Test: `packages/cli/test/schemagen-determinism.test.mjs`

**Interfaces:**
- Consumes: `buildModel`, `emitDts`, `emitJs`.
- Produces: `function runGenSchema(repoRoot: string, opts: { check: boolean }): { classes: number; fields: number; skipped: number; drift: string[] }` (in `gen.ts`); the `gen-schema` CLI command; the two committed generated files.

- [ ] **Step 1: Create the curated class list** `games/cs2/codegen-classes.json`:

```json
["CCSPlayerPawn", "CCSPlayerController", "CCSWeaponBase"]
```

- [ ] **Step 2: Write the determinism test** `packages/cli/test/schemagen-determinism.test.mjs` (runs against the REAL committed catalog — pure/offline):

```js
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { buildModel } from "../src/schemagen/model.ts";
import { emitDts } from "../src/schemagen/emit-dts.ts";
import { emitJs } from "../src/schemagen/emit-js.ts";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const catalog = JSON.parse(readFileSync(join(repo, "games/cs2/gamedata/schema-catalog.json"), "utf8"));
const list = JSON.parse(readFileSync(join(repo, "games/cs2/codegen-classes.json"), "utf8"));

test("generation is deterministic (byte-identical across runs)", () => {
  const a = buildModel(catalog, list);
  const b = buildModel(catalog, list);
  assert.equal(emitDts(a), emitDts(b));
  assert.equal(emitJs(a), emitJs(b));
});

test("real catalog: CCSPlayerPawn resolves health via CBaseEntity, friction present, chain intact", () => {
  const dts = emitDts(buildModel(catalog, list));
  assert.match(dts, /export interface CCSPlayerPawn extends CCSPlayerPawnBase \{/);
  const js = emitJs(buildModel(catalog, list));
  assert.match(js, /readInt32\(off\("CBaseEntity","m_iHealth"\)\)/);      // health inherited from CBaseEntity
  assert.match(js, /readFloat32\(off\("CBaseEntity","m_flFriction"\)\)/);
});
```

- [ ] **Step 3: Run the determinism guard** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schemagen-determinism.test.mjs`. This test imports only the Task 1–3 modules + the Step-1 list (it does NOT drive `gen.ts`/CLI, so it is a determinism/real-catalog guard, not a fail-first driver). Expected: PASS (Tasks 1–3 + the list exist). If it fails, the failure is in the model/emitters or the list — fix before proceeding. The fail-first driver for THIS task's new code (`gen.ts` + CLI + gate) is Steps 7–9 (generate → gate PASS; drift sanity-check → FAIL).

- [ ] **Step 4: Implement** `packages/cli/src/schemagen/gen.ts`:

```ts
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildModel, type Catalog } from "./model.ts";
import { emitDts } from "./emit-dts.ts";
import { emitJs } from "./emit-js.ts";

const CATALOG_PATH = "games/cs2/gamedata/schema-catalog.json";
const LIST_PATH = "games/cs2/codegen-classes.json";
const JS_OUT = "games/cs2/js/schema.generated.js";
const DTS_OUT = "packages/cs2/schema.generated.d.ts";

/** Generate the two artifacts. `check:true` compares against the committed files (no write) and reports drift. */
export function runGenSchema(repoRoot: string, opts: { check: boolean }): { classes: number; fields: number; skipped: number; drift: string[] } {
  const catalog: Catalog = JSON.parse(readFileSync(join(repoRoot, CATALOG_PATH), "utf8"));
  const list: string[] = JSON.parse(readFileSync(join(repoRoot, LIST_PATH), "utf8"));
  const model = buildModel(catalog, list);
  const dts = emitDts(model);
  const js = emitJs(model);

  const fields = model.classes.reduce((n, c) => n + c.ownFields.length, 0);
  const skipped = model.classes.reduce((n, c) => n + c.skipped.length, 0);
  const drift: string[] = [];

  const files: [string, string][] = [[JS_OUT, js], [DTS_OUT, dts]];
  for (const [rel, content] of files) {
    const abs = join(repoRoot, rel);
    if (opts.check) {
      let cur = "";
      try { cur = readFileSync(abs, "utf8"); } catch { /* missing */ }
      if (cur !== content) drift.push(rel);
    } else {
      writeFileSync(abs, content);
    }
  }
  // Report collisions + a per-class skip summary to stderr (auditable coverage).
  if (model.collisions.length) console.error(`gen-schema: ${model.collisions.length} name collision(s) → raw fallback:\n  ` + model.collisions.join("\n  "));
  for (const c of model.classes) if (c.skipped.length) console.error(`gen-schema: ${c.className}: skipped ${c.skipped.length} field(s)`);
  return { classes: model.classes.length, fields, skipped, drift };
}
```

- [ ] **Step 5: Wire the CLI** — modify `packages/cli/src/cli.ts` to add the `gen-schema` command (repo root = two levels up from `packages/cli`):

```ts
import { buildPlugin } from "./build.ts";
import { runGenSchema } from "./schemagen/gen.ts";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const [command, arg] = process.argv.slice(2);

if (command === "gen-schema") {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");   // dist/ → packages/cli → packages → repo
  const check = arg === "--check";
  const r = runGenSchema(repoRoot, { check });
  if (check) {
    if (r.drift.length) { console.error(`FAIL: generated files out of date — run \`s2script gen-schema\`:\n  ${r.drift.join("\n  ")}`); process.exit(1); }
    console.log(`schema codegen up to date (${r.classes} classes, ${r.fields} fields, ${r.skipped} skipped)`);
  } else {
    console.log(`gen-schema: wrote ${r.classes} classes, ${r.fields} fields (${r.skipped} skipped)`);
  }
} else if (command === "build" && arg) {
  try { console.log(await buildPlugin(arg)); }
  catch (err) { console.error(String(err)); process.exit(1); }
} else {
  console.error("Usage: s2script build <dir> | s2script gen-schema [--check]");
  process.exit(1);
}
```

Note: `dirname(fileURLToPath(import.meta.url))` is `packages/cli/dist` when bundled → `../../..` = repo root. Verify the depth when building.

- [ ] **Step 6: Update the test script** in `packages/cli/package.json` to run all test files:

```json
    "test": "node --experimental-strip-types --no-warnings --test",
```

- [ ] **Step 7: Build the CLI + generate the committed artifacts**

```bash
cd /home/gkh/projects/s2script/packages/cli && node build.mjs
cd /home/gkh/projects/s2script && node packages/cli/dist/cli.js gen-schema
```
Expected: writes `games/cs2/js/schema.generated.js` + `packages/cs2/schema.generated.d.ts`; prints the class/field/skip counts. Inspect both files: the `.d.ts` has `export interface CCSPlayerPawn extends CCSPlayerPawnBase {` with `health`/`friction` reachable up the chain; the `.js` sets `globalThis.__s2pkg_cs2_schema` and every read goes through `off("<class>","<m_name>")` — grep to confirm **no bare integer offset** appears as a read argument.

- [ ] **Step 8: Add the freshness gate** `scripts/check-schema-generated.sh` (mirror the existing gate style, `chmod +x`):

```bash
#!/usr/bin/env bash
# Fail if the committed schema codegen is out of date vs a fresh generation from the catalog.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
( cd packages/cli && node build.mjs >/dev/null )
node packages/cli/dist/cli.js gen-schema --check
echo "PASS: schema codegen is up to date"
```

- [ ] **Step 9: Verify the gate + determinism test + build**

```bash
cd /home/gkh/projects/s2script
bash scripts/check-schema-generated.sh          # PASS (just generated → no drift)
cd packages/cli && node --experimental-strip-types --no-warnings --test   # all green (build + schemagen tests)
cd /home/gkh/projects/s2script && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Sanity-check drift detection: temporarily add a class to `codegen-classes.json`, run `bash scripts/check-schema-generated.sh` → it must FAIL; revert.

- [ ] **Step 10: Commit** (the generated files are committed artifacts):

```bash
git add packages/cli/src/schemagen/gen.ts packages/cli/src/cli.ts packages/cli/package.json \
        games/cs2/codegen-classes.json games/cs2/js/schema.generated.js packages/cs2/schema.generated.d.ts \
        scripts/check-schema-generated.sh packages/cli/test/schemagen-determinism.test.mjs
git commit -m "feat(slice5b3): gen-schema CLI + committed generated accessors + freshness gate

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 5: Integrate the generated runtime into `pawn.js` + type surface + packaging

**Files:**
- Modify: `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`, `scripts/package-addon.sh`
- Test: `packages/cli/test/schema-runtime.test.mjs`

**Interfaces:**
- Consumes: the committed `games/cs2/js/schema.generated.js` (sets `globalThis.__s2pkg_cs2_schema` with `applyAccessors(proto, className)`), `packages/cs2/schema.generated.d.ts` (exports `CCSPlayerPawn` etc.).
- Produces: `Pawn` with generated field accessors (`health`, `friction`, `controller`, …) applied to its prototype; `pawn.js` no longer hand-writes `health`.

- [ ] **Step 1: Write the failing runtime-compose test** `packages/cli/test/schema-runtime.test.mjs` (evaluates `schema.generated.js` THEN `pawn.js` in a sandbox with stub natives — proves the concatenation + accessor application, offline):

```js
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
const pawnJs = readFileSync(join(repo, "games/cs2/js/pawn.js"), "utf8");

test("schema.generated.js + pawn.js compose: Pawn.prototype has generated accessors", () => {
  const stdPkg = { EntityRef: function (i, s) { this.index = i; this.serial = s; } };
  stdPkg.EntityRef.prototype.isValid = function () { return true; };
  stdPkg.EntityRef.prototype.readInt32 = function () { return 100; };
  stdPkg.EntityRef.prototype.readFloat32 = function () { return 0.25; };
  stdPkg.EntityRef.prototype.readBool = function () { return false; };
  stdPkg.EntityRef.prototype.readHandle = function () { return new stdPkg.EntityRef(1, 7); };
  const ctx = {
    __s2require: (name) => (name === "@s2script/std" ? stdPkg : null),
    __s2_schema_offset: () => 100,
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);   // concatenation order: schema first, pawn second
  const Pawn = ctx.__s2pkg_cs2.Pawn;
  assert.equal(typeof Object.getOwnPropertyDescriptor(Pawn.prototype, "health").get, "function");
  assert.equal(typeof Object.getOwnPropertyDescriptor(Pawn.prototype, "friction").get, "function");
  const p = new Pawn(new stdPkg.EntityRef(5, 9));
  assert.equal(p.health, 100);
  assert.equal(p.friction, 0.25);
});
```

- [ ] **Step 2: Run to verify failure** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/schema-runtime.test.mjs` → FAIL (`pawn.js` still hand-writes health / doesn't apply accessors / `Pawn` still takes `healthOff`).

- [ ] **Step 3: Rewrite** `games/cs2/js/pawn.js` — delete the hand-written `health` accessor and `healthOff`; apply the generated `CCSPlayerPawn` accessors; keep `forSlot` unchanged in behavior:

```js
// @s2script/cs2 — the injected game package. CS2 identifiers live ONLY in this file (never in core).
// The generated field accessors (schema.generated.js) run BEFORE this file (concatenated ahead of it by
// scripts/package-addon.sh) and set globalThis.__s2pkg_cs2_schema; this file applies the generated
// CCSPlayerPawn accessors to Pawn.prototype and keeps the behavioral entry point (Pawn.forSlot).
// Offsets are resolved live (Slice 3) and cached by the core OffsetCache; nothing is baked.
(function () {
  var EntityRef = __s2require("@s2script/std").EntityRef;
  var schema = globalThis.__s2pkg_cs2_schema;   // set by schema.generated.js

  function Pawn(ref) { this.ref = ref; }
  if (schema) schema.applyAccessors(Pawn.prototype, "CCSPlayerPawn");   // health, friction, controller, ...

  // slot -> controller entity (index slot+1) -> m_hPlayerPawn handle -> pawn EntityRef.
  Pawn.forSlot = function (slot) {
    var PAWN_HANDLE = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");
    if (PAWN_HANDLE < 0) return null;
    var ctrlIndex = slot + 1;
    var ctrl = new EntityRef(ctrlIndex, __s2_ent_current_serial(ctrlIndex));
    if (!ctrl.isValid()) return null;
    var handle = ctrl.readInt32(PAWN_HANDLE);
    if (handle === null) return null;
    var decoded = __s2_handle_decode(handle >>> 0);
    var pawn = new EntityRef(decoded[0], decoded[1]);
    return pawn.isValid() ? new Pawn(pawn) : null;
  };

  globalThis.__s2pkg_cs2 = { Pawn: Pawn };
})();
```

- [ ] **Step 4: Run to verify pass** — same command → PASS.

- [ ] **Step 5: Rewrite** `packages/cs2/index.d.ts` to re-export the generated interfaces and define `Pawn` as `CCSPlayerPawn` + the ref:

```ts
/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API. NO runtime code.
 * The typed field accessors are GENERATED (schema.generated.d.ts) from the schema catalog by
 * `s2script gen-schema`; this file adds the hand-written entry points on top.
 */
import type { EntityRef } from "@s2script/std";
export * from "./schema.generated";
import type { CCSPlayerPawn } from "./schema.generated";

/** A CS2 player pawn: the generated CCSPlayerPawn schema fields + the underlying serial-gated ref. */
export interface Pawn extends CCSPlayerPawn {
  readonly ref: EntityRef;
}
export declare const Pawn: {
  /** The Pawn for a player slot, or null if unoccupied / invalidated. */
  forSlot(slot: number): Pawn | null;
};
```

- [ ] **Step 6: Update packaging** — in `scripts/package-addon.sh`, replace the `pawn.js` copy (~lines 37–40) with a concatenation that puts the generated accessors FIRST:

```bash
# --- CS2 JS package (schema.generated.js + pawn.js — CS2 names live here, never in core) ---
mkdir -p "$DIST/s2script/js"
if [ -f games/cs2/js/pawn.js ]; then
    # schema.generated.js MUST precede pawn.js (pawn.js reads globalThis.__s2pkg_cs2_schema).
    cat games/cs2/js/schema.generated.js games/cs2/js/pawn.js > "$DIST/s2script/js/pawn.js"
fi
```
(Confirm the exact `$DIST` variable + surrounding lines when editing; keep the existing guard structure.)

- [ ] **Step 7: Full verification**

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
All green (the runtime-compose test proves pawn.js + generated accessors work offline; gates confirm no core leak).

- [ ] **Step 8: Commit**

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts scripts/package-addon.sh packages/cli/test/schema-runtime.test.mjs
git commit -m "feat(slice5b3): apply generated accessors in pawn.js; cs2 type surface; concat packaging

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 6: Live gate (Docker CS2) + README + CLAUDE (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: the generated runtime accessors (injected via the packaged `pawn.js`); `Pawn.forSlot`; `pawn.friction`/`pawn.health` (generated).

**No sniper rebuild needed** — 5B.3 changed no Rust/core/shim. The live gate only needs the repackaged addon JS (concatenated `schema.generated.js` + `pawn.js`) in the running server.

- [ ] **Step 1: Update the demo** `examples/demo-plugin/src/plugin.ts` to read GENERATED fields (no `__s2_schema_offset`, no `p.ref.read*` — that's the point). Keep the 5A stash line for the death demo:

```ts
import { OnGameFrame } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

let stashed: Pawn | null = null;
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad (generated accessors)");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    if (!stashed) stashed = Pawn.forSlot(0);
    const p = Pawn.forSlot(0);
    console.log("[demo] tick " + ticks
      + " health=" + (p ? p.health : "none")           // generated (m_iHealth)
      + " friction=" + (p ? p.friction : "none")       // generated (m_flFriction)
      + " stashed.health=" + (stashed ? stashed.health : "none"));  // null once that pawn dies
    if (stashed && stashed.health === null) stashed = null;
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```

- [ ] **Step 2: Build the demo `.s2sp` + repackage the addon JS**

```bash
cd /home/gkh/projects/s2script
node packages/cli/dist/cli.js build examples/demo-plugin       # esbuild strips types; pawn.friction resolves at runtime
bash scripts/package-addon.sh                                   # concatenates schema.generated.js + pawn.js into dist/.../js/pawn.js
```
Confirm `dist/addons/s2script/js/pawn.js` (or the packaged path) begins with the generated `__s2pkg_cs2_schema` block and ends with the `Pawn` IIFE.

- [ ] **Step 3: Run the live gate on Docker CS2.** Drop the demo `.s2sp` into the mounted `plugins/`; since only JS changed, restart the container to reload the injected addon JS (no rebuild). Arm: `python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1"`; wait past the boot window. Expect:
  - `[demo] tick … health=100 friction=<~0.25 or a real float> stashed.health=100` — the GENERATED `pawn.health`/`pawn.friction` read correct values through the generated accessors.
  - `bot_kick` → `health=none friction=none stashed.health=null` (forSlot null + the stashed pawn's generated `health` getter → null on the serial mismatch); server keeps ticking, no crash.
  Capture the log. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with the exact commands/errors.

- [ ] **Step 4: README** — add a `## Schema codegen (Slice 5B.3)` section: the author-facing typed-accessor usage (`pawn.health`, `pawn.friction`), that they're generated from the catalog (raw plumbing now internal), the `s2script gen-schema` **treadmill runbook** (regenerate after each CS2 update: re-dump the catalog (5B.1) → `s2script gen-schema` → commit → `scripts/check-schema-generated.sh` guards freshness), the curated-list note (grows in Slice 6), and the captured live-gate log.

- [ ] **Step 5: CLAUDE.md** — update "## Current state": Slice 5B complete (5B.1 catalog dump + 5B.2 typed access + 5B.3 codegen); "Current focus: Slice 5C next" (std breadth + the SourceMod-style module split + the player-model/`fromClient` API). Do NOT alter the standing conventions above it.

- [ ] **Step 6: Final verification + commit** (do NOT commit build artifacts — `.s2sp`, `dist/`):

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test
cd /home/gkh/projects/s2script && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md
git commit -m "feat(slice5b3): live gate PASSED — demo reads generated pawn.health/friction; README treadmill + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §7)

1. `s2script gen-schema` regenerates both artifacts deterministically; `scripts/check-schema-generated.sh` green; CLI prints coverage + skip counts.
2. Unit tests (model + emitters + determinism + runtime-compose) green; both boundary gates green.
3. Live gate: a GENERATED field (`pawn.friction`/`pawn.health`) reads correctly, `null` on entity death, no crash — with no sniper rebuild.
4. README documents `gen-schema` + the treadmill runbook + author usage; CLAUDE.md "Current state" updated (5B done; focus → 5C).
