# Plugin Contract Grammar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decouple the interface contract from the plugin package in the manifest grammar, make the host the sole source of an interface's version, and prove it by migrating zones — killing the three-version-sites drift with no registry involved.

**Architecture:** `s2script.publishes` in `package.json` is authored as `"self"` or a `{interface: range}` map; the CLI expands it, hashes the contract `.d.ts`, and derives a manifest carrying `{version, typesSha256}` per interface. Core parses that map, and `publishInterface(name, impl)` loses its version parameter — the host injects the version from the manifest and fails the load if a plugin publishes a name it never declared. Two live producers of one interface name are rejected.

**Tech Stack:** Rust (core, serde, V8), TypeScript (`@s2script/cli`, esbuild, adm-zip), `node:test` for CLI tests, `cargo test -p s2script-core` for core.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-15-plugin-contract-distribution-design.md` (commit fd35670). Every task traces to a section; cited below per task.
- **`typesSha256` is the sha256 of the contract `.d.ts`'s raw published bytes — NO normalization** (no line-ending or whitespace canonicalization). Spec §4.2.
- **No version string may appear in TypeScript source** once Task 5 lands. Spec §4.3.
- **Core is engine-generic.** No CS2 identifier may enter `core/src`. `make check-boundary` must stay green. CLAUDE.md.
- **Degrade per-descriptor, never crash globally.** A bad `publishes` entry disables *that* plugin's load with a named WARN; the framework keeps running. CLAUDE.md.
- **Manifest is derived, never hand-authored.** The `.s2sp` consumes a minimal manifest. CLAUDE.md.
- **`publishes: "self"` does not compose** — a plugin publishing its own contract *and* implementing another's must use the map form. Spec §4.2.
- **Backwards compatibility is NOT required.** Pre-users; no `.s2sp` exists outside this repo. A 3-arg `publishInterface` becomes a hard error, not a deprecation.
- **Out of scope (do not build):** semver unification (`version_satisfies` stays major-only — Task 12 pins the tests that the follow-on spec will change), registry/contract-artifact publish, `s2script add`/`install`, virtual-dependency resolution, `@s2script/*` stub consolidation, the `/npm/*` facade. Spec §10.
- **Worktree:** all work in a dedicated worktree on `feat/contract-grammar`, rebased onto current `main` before the live gate. PR required. Changeset required (`packages/cli` and `packages/interfaces` both change).

## File Structure

| File | Responsibility |
|---|---|
| `packages/cli/src/publish-gate.ts` | **Create.** Adapted from `origin/cursor/s2script-install-lockfile-5823`. `publishes ⇒ types` validation; `hasPublishes` helper. |
| `packages/cli/src/publishes.ts` | **Create.** The grammar: expand `"self"` → map form, validate entries, compute `typesSha256`, derive the manifest `publishes` block. The single owner of the grammar. |
| `packages/cli/src/build.ts` | **Modify.** Wire the gate before tsc; call `derivePublishes`; add the embedded types member. |
| `packages/interfaces/index.d.ts` | **Modify.** `publishInterface(name, impl)` — drop the version param. |
| `core/src/loader.rs` | **Modify.** `Manifest.publishes: HashMap<String, PublishDecl>`; expose it to the host at load. |
| `core/src/interfaces.rs` | **Modify.** `publish()` returns a result; reject a second live producer of a live name. |
| `core/src/v8host.rs` | **Modify.** `__s2_iface_publish(name, impl)` — 2 args; version from the manifest; fail load on undeclared name; prune `IFACE_METHODS` by `(producer_id, name)`. |
| `plugins/zones/api.d.ts` | **Create.** The zones contract (moved from `packages/zones/index.d.ts`). |
| `plugins/zones/package.json` | **Modify.** Drop `private`, add `types`, `publishes: "self"`, single version. |
| `plugins/zones/src/plugin.ts` | **Modify.** Type the impl against `../api`; drop the `"0.1.0"` literal. |
| `packages/zones/` | **Delete.** Spec §6. |
| `examples/zones-consumer-demo/package.json` | **Modify.** Re-pin to the zones package version. |

---

### Task 1: The `publishes` grammar module

Spec §4.2. The single owner of the grammar — expansion, validation, hashing. Pure functions, no I/O beyond reading the contract file.

**Files:**
- Create: `packages/cli/src/publishes.ts`
- Create: `packages/cli/test/publishes.test.mjs`

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces:
  - `export interface PublishDecl { version: string; typesSha256: string; }`
  - `export type PublishesAuthored = string | Record<string, string> | undefined;`
  - `export function expandPublishes(authored: PublishesAuthored, pkgName: string, pkgVersion: string): Record<string, string>` — `"self"` → `{[pkgName]: pkgVersion}`; a map passes through; `undefined`/empty → `{}`. Throws `Error` on a non-`"self"` string or a non-string entry value.
  - `export function hashContract(typesPath: string): string` — sha256 hex of the file's raw bytes.
  - `export function derivePublishes(authored: PublishesAuthored, pkgName: string, pkgVersion: string, typesPath: string | null): Record<string, PublishDecl>` — expands, then attaches `typesSha256` from `typesPath` to every entry. Throws if entries exist and `typesPath` is null.

- [ ] **Step 1: Write the failing test**

Create `packages/cli/test/publishes.test.mjs`:

```javascript
/**
 * TDD test: the publishes grammar — "self" sugar, map form, hashing.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/publishes.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { createHash } from "node:crypto";
import { writeFileSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expandPublishes, hashContract, derivePublishes } from "../src/publishes.ts";

test("expandPublishes: 'self' becomes a single self-named entry at the package version", () => {
  const out = expandPublishes("self", "@s2script/zones", "1.2.0");
  assert.deepEqual(out, { "@s2script/zones": "1.2.0" });
});

test("expandPublishes: map form passes through unchanged", () => {
  const out = expandPublishes({ "@community/mapchooser": "^1.2.0" }, "@edge/mce", "3.1.0");
  assert.deepEqual(out, { "@community/mapchooser": "^1.2.0" });
});

test("expandPublishes: absent or empty yields no entries", () => {
  assert.deepEqual(expandPublishes(undefined, "@a/b", "1.0.0"), {});
  assert.deepEqual(expandPublishes({}, "@a/b", "1.0.0"), {});
});

test("expandPublishes: a string other than 'self' is a named error", () => {
  assert.throws(
    () => expandPublishes("mine", "@a/b", "1.0.0"),
    /publishes: the only valid string form is "self"/
  );
});

test("expandPublishes: a non-string entry value is a named error", () => {
  assert.throws(
    () => expandPublishes({ "@x/y": { version: "1.0.0" } }, "@a/b", "1.0.0"),
    /publishes\["@x\/y"\] must be a version range string/
  );
});

test("hashContract: sha256 of raw bytes, no normalization", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  // CRLF + trailing whitespace must survive: hashing the RAW bytes is the contract.
  const body = "export declare function a(): void;\r\n  \r\n";
  writeFileSync(p, body);
  const expected = createHash("sha256").update(readFileSync(p)).digest("hex");
  assert.equal(hashContract(p), expected);
  // And prove no normalization happened: an LF twin must hash differently.
  const q = join(dir, "api2.d.ts");
  writeFileSync(q, body.replace(/\r\n/g, "\n"));
  assert.notEqual(hashContract(p), hashContract(q));
});

test("derivePublishes: attaches the contract hash to every entry", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  writeFileSync(p, "export declare function z(): void;\n");
  const out = derivePublishes("self", "@s2script/zones", "1.2.0", p);
  assert.deepEqual(Object.keys(out), ["@s2script/zones"]);
  assert.equal(out["@s2script/zones"].version, "1.2.0");
  assert.equal(out["@s2script/zones"].typesSha256, hashContract(p));
});

test("derivePublishes: entries without a contract file is a named error", () => {
  assert.throws(
    () => derivePublishes("self", "@a/b", "1.0.0", null),
    /publishes is set but no contract \.d\.ts was resolved/
  );
});

test("derivePublishes: no entries yields an empty block and needs no contract", () => {
  assert.deepEqual(derivePublishes(undefined, "@a/b", "1.0.0", null), {});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/publishes.test.mjs`
Expected: FAIL — `Cannot find module '../src/publishes.ts'`

- [ ] **Step 3: Write minimal implementation**

Create `packages/cli/src/publishes.ts`:

```typescript
/**
 * The `s2script.publishes` grammar (design spec 2026-07-15 §4.2).
 *
 * AUTHORED (package.json):  "self"  |  { "<interface>": "<range>" }
 * DERIVED  (manifest.json): { "<interface>": { version, typesSha256 } }
 *
 * The interface NAME is decoupled from the package name: @edge/mce@3.1.0 may
 * publish @community/mapchooser@1.2.0. "self" is sugar for the dominant case
 * (name = package name, version = package version) and does NOT compose — a
 * plugin publishing its own contract AND implementing another's uses the map form.
 */

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

/** One derived manifest entry: the resolved contract version + its content hash. */
export interface PublishDecl {
  version: string;
  typesSha256: string;
}

/** The authored form, straight off package.json. */
export type PublishesAuthored = string | Record<string, string> | undefined;

/** Expand the authored form to `{interface: range}`. Throws on a malformed grammar. */
export function expandPublishes(
  authored: PublishesAuthored,
  pkgName: string,
  pkgVersion: string,
): Record<string, string> {
  if (authored === undefined || authored === null) return {};
  if (typeof authored === "string") {
    if (authored.trim() !== "self") {
      throw new Error(
        `publishes: the only valid string form is "self" (got ${JSON.stringify(authored)}); ` +
          `use the map form to publish a differently-named contract`,
      );
    }
    return { [pkgName]: pkgVersion };
  }
  if (typeof authored !== "object") {
    throw new Error(`publishes must be "self" or an object (got ${typeof authored})`);
  }
  const out: Record<string, string> = {};
  for (const [iface, range] of Object.entries(authored)) {
    if (typeof range !== "string") {
      throw new Error(`publishes[${JSON.stringify(iface)}] must be a version range string`);
    }
    out[iface] = range;
  }
  return out;
}

/** sha256 hex of the contract's RAW bytes. No normalization — any canonicalization
 *  step would be a second source of truth (spec §4.2). */
export function hashContract(typesPath: string): string {
  return createHash("sha256").update(readFileSync(typesPath)).digest("hex");
}

/** Expand + hash → the manifest `publishes` block. */
export function derivePublishes(
  authored: PublishesAuthored,
  pkgName: string,
  pkgVersion: string,
  typesPath: string | null,
): Record<string, PublishDecl> {
  const expanded = expandPublishes(authored, pkgName, pkgVersion);
  const names = Object.keys(expanded);
  if (names.length === 0) return {};
  if (typesPath === null) {
    throw new Error(
      `publishes is set but no contract .d.ts was resolved — set "types": "api.d.ts" in package.json`,
    );
  }
  const typesSha256 = hashContract(typesPath);
  const out: Record<string, PublishDecl> = {};
  for (const name of names) {
    out[name] = { version: expanded[name], typesSha256 };
  }
  return out;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/publishes.test.mjs`
Expected: PASS — 8 tests

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/publishes.ts packages/cli/test/publishes.test.mjs
git commit -m "feat(cli): the publishes grammar — self sugar, map form, contract hashing"
```

**Note for Task 6:** `derivePublishes` currently stamps the *authored range* as `version`. For `"self"` that range IS the concrete package version, which is correct. For the map form (`"^1.2.0"`) resolving a range to a concrete contract version requires the registry and is **out of scope** (spec §10) — Task 6 rejects the map form at build time with a named "not yet supported" error rather than shipping a manifest with a range where a concrete version belongs.

---

### Task 2: The `publishes ⇒ types` gate

Spec §4.6. Adapted from `origin/cursor/s2script-install-lockfile-5823:packages/cli/src/publish-gate.ts`. A plugin that publishes anything must have a contract file.

**Files:**
- Create: `packages/cli/src/publish-gate.ts`
- Create: `packages/cli/test/publish-gate.test.mjs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `export function hasPublishes(publishes: unknown): boolean`
  - `export type PublishGateResult = { ok: true; typesPath: string | null } | { ok: false; error: string }`
  - `export function assertPublishesTypes(pkg: PluginPkgForGate, pluginDir: string): PublishGateResult` — `typesPath` is absolute, or `null` when the plugin publishes nothing.

- [ ] **Step 1: Write the failing test**

Create `packages/cli/test/publish-gate.test.mjs`:

```javascript
/**
 * TDD test: publishes ⇒ types gate.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/publish-gate.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertPublishesTypes, hasPublishes } from "../src/publish-gate.ts";

function dirWith(files) {
  const d = mkdtempSync(join(tmpdir(), "s2gate-"));
  for (const [name, body] of Object.entries(files)) writeFileSync(join(d, name), body);
  return d;
}

test("hasPublishes: recognises self, a non-empty map, and nothing", () => {
  assert.equal(hasPublishes("self"), true);
  assert.equal(hasPublishes({ "@x/y": "^1.0.0" }), true);
  assert.equal(hasPublishes({}), false);
  assert.equal(hasPublishes(undefined), false);
  assert.equal(hasPublishes(null), false);
  assert.equal(hasPublishes(""), false);
});

test("a plugin that publishes nothing passes with no contract", () => {
  const d = dirWith({});
  const r = assertPublishesTypes({ s2script: {} }, d);
  assert.equal(r.ok, true);
  assert.equal(r.typesPath, null);
});

test("publishes with a valid contract resolves an absolute types path", () => {
  const d = dirWith({ "api.d.ts": "export declare function z(): void;\n" });
  const r = assertPublishesTypes({ types: "api.d.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, true);
  assert.equal(r.typesPath, join(d, "api.d.ts"));
});

test("publishes without a types field is a named error", () => {
  const d = dirWith({});
  const r = assertPublishesTypes({ s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /"types" is missing/);
});

test("publishes pointing at a non-.d.ts is a named error", () => {
  const d = dirWith({ "api.ts": "export function z() {}\n" });
  const r = assertPublishesTypes({ types: "api.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /must be a \.d\.ts file/);
});

test("publishes pointing at a missing file is a named error", () => {
  const d = dirWith({});
  const r = assertPublishesTypes({ types: "api.d.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /types file not found/);
});

test("publishes pointing at an empty file is a named error", () => {
  const d = dirWith({ "api.d.ts": "" });
  const r = assertPublishesTypes({ types: "api.d.ts", s2script: { publishes: "self" } }, d);
  assert.equal(r.ok, false);
  assert.match(r.error, /empty/);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/publish-gate.test.mjs`
Expected: FAIL — `Cannot find module '../src/publish-gate.ts'`

- [ ] **Step 3: Write minimal implementation**

Create `packages/cli/src/publish-gate.ts` — this is the branch file verbatim except the doc comment cites the new spec:

```typescript
/**
 * publishes ⇒ types gate (design spec 2026-07-15 §4.6): if s2script.publishes is
 * set, package.json must point "types"/"typings" at an existing non-empty .d.ts.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

export interface PluginPkgForGate {
  types?: string;
  typings?: string;
  s2script?: {
    publishes?: Record<string, unknown> | string | null;
  };
}

export interface PublishGateOk {
  ok: true;
  typesPath: string | null; // absolute path, or null when no publishes
}

export interface PublishGateErr {
  ok: false;
  error: string;
}

export type PublishGateResult = PublishGateOk | PublishGateErr;

export function hasPublishes(publishes: unknown): boolean {
  if (publishes == null) return false;
  if (typeof publishes === "string") return publishes.trim().length > 0;
  if (typeof publishes === "object") return Object.keys(publishes as object).length > 0;
  return false;
}

/** Validate publishes ⇒ types. `pluginDir` is the package root. */
export function assertPublishesTypes(
  pkg: PluginPkgForGate,
  pluginDir: string
): PublishGateResult {
  const publishes = pkg.s2script?.publishes;
  if (!hasPublishes(publishes)) {
    return { ok: true, typesPath: null };
  }

  const typesRel = pkg.types ?? pkg.typings;
  if (!typesRel || typeof typesRel !== "string") {
    return {
      ok: false,
      error: 'publishes is set but "types" is missing — add api.d.ts and set "types": "api.d.ts"',
    };
  }
  if (!typesRel.endsWith(".d.ts")) {
    return {
      ok: false,
      error: `published API must be a .d.ts file (got ${JSON.stringify(typesRel)})`,
    };
  }

  const typesPath = resolve(pluginDir, typesRel);
  if (!existsSync(typesPath)) {
    return { ok: false, error: `types file not found: ${typesRel}` };
  }
  const st = statSync(typesPath);
  if (!st.isFile() || st.size === 0) {
    return { ok: false, error: `types file is empty or not a file: ${typesRel}` };
  }

  const body = readFileSync(typesPath, "utf8").trim();
  if (!body) {
    return { ok: false, error: `types file is empty: ${typesRel}` };
  }

  return { ok: true, typesPath };
}

/** Read package.json from a plugin dir and run the gate. */
export function assertPublishesTypesInDir(pluginDir: string): PublishGateResult {
  const pkgPath = join(pluginDir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8")) as PluginPkgForGate;
  return assertPublishesTypes(pkg, pluginDir);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/publish-gate.test.mjs`
Expected: PASS — 7 tests

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/publish-gate.ts packages/cli/test/publish-gate.test.mjs
git commit -m "feat(cli): publishes ⇒ types gate (adapted from the registry branch)"
```

---

### Task 3: Core parses the manifest `publishes` map

Spec §4.2. `Manifest.publishes` becomes structured. Serde must accept the derived form and reject nothing else silently.

**Files:**
- Modify: `core/src/loader.rs:22-36` (the `Manifest` struct)
- Test: `core/src/loader.rs` (the existing `#[cfg(test)]` module at the bottom)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct PublishDecl { pub version: String, pub types_sha256: String }` (serde: `typesSha256`)
  - `Manifest.publishes: HashMap<String, PublishDecl>` (serde default → empty map when absent)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `core/src/loader.rs`:

```rust
    #[test]
    fn manifest_parses_derived_publishes_block() {
        let json = r#"{
            "id":"@s2script/zones","version":"1.2.0","apiVersion":"1.x",
            "publishes":{"@s2script/zones":{"version":"1.2.0","typesSha256":"abc123"}}
        }"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        let d = m.publishes.get("@s2script/zones").expect("entry present");
        assert_eq!(d.version, "1.2.0");
        assert_eq!(d.types_sha256, "abc123");
    }

    #[test]
    fn manifest_without_publishes_yields_an_empty_map() {
        let json = r#"{"id":"@demo/x","version":"0.1.0","apiVersion":"1.x"}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert!(m.publishes.is_empty());
    }

    #[test]
    fn manifest_publishes_may_name_a_different_interface_than_the_package() {
        // @edge/mce publishes @community/mapchooser — the decoupling this grammar exists for.
        let json = r#"{
            "id":"@edge/mce","version":"3.1.0","apiVersion":"1.x",
            "publishes":{"@community/mapchooser":{"version":"1.2.0","typesSha256":"deadbeef"}}
        }"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert_eq!(m.publishes["@community/mapchooser"].version, "1.2.0");
        assert!(!m.publishes.contains_key("@edge/mce"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p s2script-core manifest_parses_derived_publishes_block`
Expected: FAIL — compile error, `Manifest` has no field `publishes`

- [ ] **Step 3: Write minimal implementation**

In `core/src/loader.rs`, add above the `Manifest` struct:

```rust
/// One derived `publishes` entry: the contract's resolved version + the sha256 of the
/// exact `.d.ts` bytes the implementation typechecked against (design spec §4.2).
/// The interface NAME is the map key and is deliberately independent of the plugin id.
#[derive(Debug, Deserialize, Clone)]
pub struct PublishDecl {
    pub version: String,
    #[serde(rename = "typesSha256", default)]
    pub types_sha256: String,
}
```

Then add this field to `Manifest` (after `optional_plugin_dependencies`):

```rust
    /// Interfaces this plugin implements: interface-name → {version, typesSha256}.
    /// Empty when the plugin publishes nothing. The host injects an interface's version
    /// from HERE — a plugin may never type a version string (spec §4.3).
    #[serde(default)]
    pub publishes: std::collections::HashMap<String, PublishDecl>,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p s2script-core manifest_`
Expected: PASS — 3 tests (`manifest_parses_derived_publishes_block`, `manifest_without_publishes_yields_an_empty_map`, `manifest_publishes_may_name_a_different_interface_than_the_package`)

- [ ] **Step 5: Commit**

```bash
git add core/src/loader.rs
git commit -m "feat(core): parse the derived publishes map (interface name decoupled from plugin id)"
```

---

### Task 4: Reject a second live producer of one interface

Spec §4.8. Today `InterfaceRegistry::publish` silently overwrites — correct for republish-on-reload (same producer), wrong for a different producer.

**Files:**
- Modify: `core/src/interfaces.rs:63-76` (`publish`)
- Test: `core/src/interfaces.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `InterfaceEntry` (existing: `version`, `producer_id`, `producer_gen`, `method_names`, `subscribers`).
- Produces: `pub fn publish(&mut self, name: &str, version: &str, producer_id: &str, producer_gen: u64, method_names: Vec<String>) -> Result<(), String>` — **signature change**: was `()`. `Err(msg)` when a *different* `producer_id` already holds a live entry for `name`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `core/src/interfaces.rs`:

```rust
    #[test]
    fn republish_by_the_same_producer_succeeds_and_keeps_subscribers() {
        let mut r = InterfaceRegistry::new();
        r.publish("@c/mapchooser", "1.2.0", "@edge/mce", 1, vec!["pick".into()]).expect("first");
        r.add_subscriber("@c/mapchooser", Subscriber {
            sub_id: 7, consumer_id: "@x/rtv".into(), consumer_gen: 1, event: "changed".into(),
        });
        // Same producer republishing (hot-reload) is allowed and preserves subscribers.
        r.publish("@c/mapchooser", "1.3.0", "@edge/mce", 2, vec!["pick".into()]).expect("republish");
        let e = r.lookup("@c/mapchooser").expect("entry");
        assert_eq!(e.version, "1.3.0");
        assert_eq!(e.subscribers.len(), 1, "republish must keep subscribers");
    }

    #[test]
    fn a_second_live_producer_of_the_same_interface_is_rejected() {
        let mut r = InterfaceRegistry::new();
        r.publish("@c/mapchooser", "1.2.0", "@edge/mce", 1, vec!["pick".into()]).expect("first");
        // A DIFFERENT producer claiming the same live name: implementations are alternatives.
        let err = r.publish("@c/mapchooser", "1.2.0", "@stock/mapchooser", 1, vec!["pick".into()])
            .expect_err("second producer must be rejected");
        assert!(err.contains("@c/mapchooser"), "error names the interface: {}", err);
        assert!(err.contains("@edge/mce"), "error names the incumbent producer: {}", err);
        // The incumbent is untouched.
        assert_eq!(r.lookup("@c/mapchooser").expect("entry").producer_id, "@edge/mce");
    }

    #[test]
    fn a_new_producer_may_claim_a_name_after_the_incumbent_unloads() {
        let mut r = InterfaceRegistry::new();
        r.publish("@c/mapchooser", "1.2.0", "@edge/mce", 1, vec!["pick".into()]).expect("first");
        r.remove_by_producer("@edge/mce");
        r.publish("@c/mapchooser", "1.2.0", "@stock/mapchooser", 1, vec!["pick".into()])
            .expect("free after unload");
        assert_eq!(r.lookup("@c/mapchooser").expect("entry").producer_id, "@stock/mapchooser");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p s2script-core a_second_live_producer`
Expected: FAIL — compile error (`.expect()` on `()`), and once compiling, the rejection assert fails because `publish` overwrites.

- [ ] **Step 3: Write minimal implementation**

Replace `publish` in `core/src/interfaces.rs`:

```rust
    /// Register (or re-register) an interface. Returns Err when a DIFFERENT producer already
    /// holds a live entry for `name` — implementations are alternatives (you run mapchooser OR
    /// mapchooser_extended, never both), spec §4.8. Re-publish by the SAME producer is a
    /// hot-reload and preserves subscribers.
    pub fn publish(
        &mut self,
        name: &str,
        version: &str,
        producer_id: &str,
        producer_gen: u64,
        method_names: Vec<String>,
    ) -> Result<(), String> {
        if let Some(existing) = self.ifaces.get(name) {
            if existing.producer_id != producer_id {
                return Err(format!(
                    "interface '{}' is already published by '{}' — '{}' cannot also publish it \
                     (implementations are alternatives; load only one)",
                    name, existing.producer_id, producer_id
                ));
            }
        }
        // Preserve any existing subscribers on republish of the same name: a producer updating its
        // interface in place keeps its consumers subscribed. (A fresh producer's entry starts empty
        // because `remove_by_producer` cleared the prior one on unload.)
        let subscribers = self.ifaces.get(name).map(|e| e.subscribers.clone()).unwrap_or_default();
        self.ifaces.insert(name.to_string(), InterfaceEntry {
            version: version.to_string(),
            producer_id: producer_id.to_string(),
            producer_gen,
            method_names,
            subscribers,
        });
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p s2script-core --lib interfaces::`
Expected: PASS. If other call sites fail to compile, they are fixed in Task 5 — if `cargo build` breaks *only* at `v8host.rs`'s `IFACES.with(...publish(...))` call, that is expected; proceed.

- [ ] **Step 5: Commit**

```bash
git add core/src/interfaces.rs
git commit -m "feat(core): reject a second live producer of an interface name"
```

---

### Task 5: Host-injected interface version

Spec §4.3. The freeze target's runtime half. `__s2_iface_publish` takes 2 args; the version comes from the manifest; publishing an undeclared name fails.

**Files:**
- Modify: `core/src/v8host.rs:4285-4330` (`s2_iface_publish`)
- Modify: `core/src/v8host.rs:807-811` (the `interfaces` prelude object)
- Modify: `core/src/v8host.rs:8985-8987` (the `IFACE_METHODS` prune TODO)
- Modify: `packages/interfaces/index.d.ts`
- Test: `core/src/v8host.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `loader::PublishDecl` and `Manifest.publishes` (Task 3); `InterfaceRegistry::publish(...) -> Result<(), String>` (Task 4).
- Produces:
  - JS: `publishInterface(name, impl)` → `PublishHandle` (2 args).
  - Native: `__s2_iface_publish(name, implObj)` (2 args).
  - Rust: `pub fn set_plugin_publishes(plugin_id: &str, publishes: HashMap<String, loader::PublishDecl>)` — the loader calls this before `load_plugin_js`; the host reads it inside `s2_iface_publish`. Mirrors the existing `set_plugin_imports` pattern.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `core/src/v8host.rs`:

```rust
    #[test]
    fn publish_interface_takes_its_version_from_the_manifest() {
        let _ = init(dummy_logger());
        // The manifest declares the contract; the plugin never types a version.
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "2.5.0".into(), types_sha256: "abc".into() },
        )].into_iter().collect());
        create_plugin_context("prod");
        eval_in_context("prod", r#"__s2_iface_publish("@x/greeter",{ greet:function(){return "hi";} });"#)
            .expect("publish");
        let v = IFACES.with(|r| r.borrow().lookup("@x/greeter").map(|e| e.version.clone()));
        assert_eq!(v, Some("2.5.0".to_string()), "version must come from the manifest, not JS");
        shutdown();
    }

    #[test]
    fn publish_interface_of_an_undeclared_name_is_refused() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", std::collections::HashMap::new());
        create_plugin_context("prod");
        // Publishing a name absent from the manifest must NOT register anything.
        let _ = eval_in_context("prod", r#"__s2_iface_publish("@x/undeclared",{ a:function(){} });"#);
        let found = IFACES.with(|r| r.borrow().lookup("@x/undeclared").is_some());
        assert!(!found, "an undeclared interface must never reach the registry");
        shutdown();
    }

    #[test]
    fn publish_interface_of_a_name_owned_by_another_producer_is_refused() {
        let _ = init(dummy_logger());
        let decl = crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() };
        set_plugin_publishes("first", [("@x/dup".to_string(), decl.clone())].into_iter().collect());
        set_plugin_publishes("second", [("@x/dup".to_string(), decl)].into_iter().collect());
        create_plugin_context("first");
        create_plugin_context("second");
        eval_in_context("first", r#"__s2_iface_publish("@x/dup",{ a:function(){return 1;} });"#).expect("first");
        let _ = eval_in_context("second", r#"__s2_iface_publish("@x/dup",{ a:function(){return 2;} });"#);
        let owner = IFACES.with(|r| r.borrow().lookup("@x/dup").map(|e| e.producer_id.clone()));
        assert_eq!(owner, Some("first".to_string()), "the incumbent producer must keep the name");
        shutdown();
    }

    #[test]
    fn prelude_publish_interface_takes_two_args() {
        let _ = init(dummy_logger());
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.4.0".into(), types_sha256: "abc".into() },
        )].into_iter().collect());
        load_plugin_js("prod", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/greeter", { greet: function (n) { return "hi " + n.who; } });
        "#).expect("load");
        let v = IFACES.with(|r| r.borrow().lookup("@x/greeter").map(|e| e.version.clone()));
        assert_eq!(v, Some("1.4.0".to_string()));
        shutdown();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p s2script-core publish_interface_takes_its_version`
Expected: FAIL — compile error, `set_plugin_publishes` not found

- [ ] **Step 3: Write minimal implementation**

**(a)** Add the publishes store beside the existing plugin-imports store in `core/src/v8host.rs` (place it next to the other `thread_local!` declarations):

```rust
thread_local! {
    /// plugin_id → the manifest's `publishes` map. The SOLE source of an interface's version
    /// (spec §4.3): JS never carries one. Set by the loader before load_plugin_js.
    static PLUGIN_PUBLISHES: std::cell::RefCell<
        std::collections::HashMap<String, std::collections::HashMap<String, crate::loader::PublishDecl>>
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Record a plugin's declared `publishes` map (from its manifest) before its context loads.
pub fn set_plugin_publishes(
    plugin_id: &str,
    publishes: std::collections::HashMap<String, crate::loader::PublishDecl>,
) {
    PLUGIN_PUBLISHES.with(|p| { p.borrow_mut().insert(plugin_id.to_string(), publishes); });
}

/// Drop a plugin's publishes map (teardown).
pub fn clear_plugin_publishes(plugin_id: &str) {
    PLUGIN_PUBLISHES.with(|p| { p.borrow_mut().remove(plugin_id); });
}
```

**(b)** Replace `s2_iface_publish` (currently at ~4285). The doc comment, arity, version lookup, and both refusals change:

```rust
/// `__s2_iface_publish(name, implObj)` — the producer registers an interface it DECLARED.
/// The version is injected from the plugin's manifest `publishes` map (spec §4.3): a plugin may
/// never type a version string. Refuses (WARN + return, no throw — publish is producer-side) when:
/// the name is absent from the manifest, or another live producer already owns it (spec §4.8).
fn s2_iface_publish(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_undefined();
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(impl_obj) = v8::Local::<v8::Object>::try_from(args.get(1)) else {
            log_warn(&format!("WARN: iface_publish('{}'): impl is not an object", name));
            return;
        };
        let Some(owner) = current_plugin(scope) else {
            log_warn("WARN: iface_publish: no current plugin");
            return;
        };

        // The manifest is the sole source of the version. An undeclared name never registers.
        let Some(decl) = PLUGIN_PUBLISHES.with(|p| {
            p.borrow().get(&owner).and_then(|m| m.get(&name)).cloned()
        }) else {
            log_warn(&format!(
                "WARN: iface_publish('{}'): plugin '{}' did not declare this interface in its \
                 manifest `publishes` — refusing",
                name, owner
            ));
            return;
        };

        let generation = REGISTRY.with(|r| r.borrow().generation_of(&owner)).unwrap_or(0);

        // Enumerate own function properties → method names + capture Globals.
        let mut method_names: Vec<String> = Vec::new();
        let mut captured: Vec<(String, v8::Global<v8::Function>)> = Vec::new();
        if let Some(prop_names) = impl_obj.get_own_property_names(scope, Default::default()) {
            for i in 0..prop_names.length() {
                let Some(key) = prop_names.get_index(scope, i) else { continue };
                let Some(val) = impl_obj.get(scope, key) else { continue };
                if let Ok(f) = v8::Local::<v8::Function>::try_from(val) {
                    let m = key.to_rust_string_lossy(scope);
                    method_names.push(m.clone());
                    captured.push((m, v8::Global::new(scope.as_ref(), f)));
                }
            }
        }

        // Register FIRST: a REJECTED publish must not leave method Globals behind (a rejected
        // second producer's functions would otherwise shadow the incumbent's in IFACE_METHODS,
        // which is keyed by name — see (c)).
        if let Err(e) = IFACES.with(|r| {
            r.borrow_mut().publish(&name, &decl.version, &owner, generation, method_names)
        }) {
            log_warn(&format!("WARN: iface_publish('{}'): {}", name, e));
            return;
        }
        for (m, g) in captured {
            IFACE_METHODS.with(|mm| { mm.borrow_mut().insert((name.clone(), m), g); });
        }
        REGISTRY.with(|r| {
            if let Some(l) = r.borrow_mut().ledger_mut(&owner) { l.record_interface(name.clone()); }
        });
    }));
}
```

**(c)** **`IFACE_METHODS` keeps its `(iface_name, method)` key — do NOT re-key it.** The slice-5 TODO at ~8984 asks for a `(producer_id, name)` key so that unloading one producer cannot drop another producer's Globals for the same name. Task 4 makes that scenario **unreachable**: a second live producer is rejected before it registers, so one-producer-per-name is now an enforced invariant rather than a hopeful comment. Re-keying would churn `iface_call`'s hot-path lookup at `v8host.rs:4402` for no benefit.

The only ordering requirement is the one in (b): insert Globals *after* a successful `publish()`, never before, so a rejected publish leaves nothing behind.

**(d)** Retire the TODO at ~8984 — the prune body is unchanged, only the comment. Replace the TODO paragraph inside the `Resource::Interface(name)` arm with:

```rust
                plugin::Resource::Interface(name) => {
                    // Prunes IFACE_METHODS by interface NAME. Safe by construction since the
                    // contract-grammar slice: InterfaceRegistry::publish rejects a second live
                    // producer of a name (spec §4.8), so at most one producer can ever hold the
                    // methods being pruned here. (Retires the slice-5 TODO, which asked for a
                    // (producer_id, name) key against a case that can no longer occur.)
                    IFACES.with(|r| { let _ = r.borrow_mut().remove_by_producer(id); });
                    IFACE_METHODS.with(|m| {
                        m.borrow_mut().retain(|(iface, _method), _| iface != &name);
                    });
                }
```

**(e)** The prelude at ~807:

```rust
  const interfaces = {
    publishInterface: function (name, impl) {
      __s2_iface_publish(name, impl);
      return { emit: function (ev, payload) { return __s2_iface_emit(name, ev, payload); } };
    },
  };
```

**(f)** `packages/interfaces/index.d.ts` — replace the `publishInterface` declaration and its doc:

```typescript
/**
 * Publish a typed inter-plugin interface under `name`. `impl`'s methods become the
 * natives consumers call (`interface.method(...)`); the returned handle's `emit` fans
 * forwarded events out to consumers' `on(event, …)` subscriptions.
 *
 * The interface's VERSION is injected by the host from this plugin's manifest
 * `publishes` map — never passed here, and never written in TypeScript source.
 * Publishing a name the manifest does not declare is refused at load.
 *
 * Auto-ledgered: the interface is withdrawn (and hard-dep consumers degraded) on unload.
 */
export declare function publishInterface(
  name: string,
  impl: Record<string, (...args: any[]) => any>,
): PublishHandle;
```

**(g)** Wire the loader. There are exactly **three** `set_plugin_imports` call sites, each immediately preceding a load, and **one** clear site. Add a publishes call beside each:

- `core/src/loader.rs:236`, `core/src/loader.rs:349`, `core/src/loader.rs:376` — after each existing line:
  ```rust
  crate::v8host::set_plugin_imports(&manifest.id, imports_from_manifest(&manifest));
  crate::v8host::set_plugin_publishes(&manifest.id, manifest.publishes.clone());   // ← add
  ```
- `core/src/v8host.rs:9035` — in the unload path, after `IFACES.with(|r| r.borrow_mut().clear_imports(id));`:
  ```rust
  clear_plugin_publishes(id);   // ← add
  ```

**(h)** Migrate the in-isolate tests still calling the 3-arg form. There are exactly **five**:

| Line | Interface | Version asserted |
|---|---|---|
| ~9859 | `@x/greeter` | `1.0.0` |
| ~9905 | `@x/greeter` | `1.0.0` |
| ~9941 | `@x/boom` | `1.0.0` |
| ~9955 | `@x/void` | `1.0.0` |
| ~9975 | `@x/greeter` | `1.0.0` |

Each needs two edits. First, add a publishes declaration before the producer's context is created — e.g. for ~9859's `"prod"`:

```rust
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "test".into() },
        )].into_iter().collect());
```

Second, drop the version argument from the call:

```rust
        // before: __s2_iface_publish("@x/greeter","1.0.0",{ greet:function(n){return "hi "+n;} });
        // after:
        eval_in_context("prod", r#"__s2_iface_publish("@x/greeter",{ greet:function(n){return "hi "+n;} });"#).expect("publish");
```

The declared version must equal what the consumer's `set_plugin_imports` range accepts — every one of these consumers pins `^1.0.0`, so declaring `1.0.0` keeps them green.

`v8host.rs:10003` asserts `IFACE_METHODS.get(&("@x/greeter".into(),"greet".into())).is_none()` — the key is unchanged by (c), so **this test needs no edit**.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p s2script-core`
Expected: PASS — the full suite, including the 4 new tests and every migrated 3-arg call site.

- [ ] **Step 5: Verify the boundary gate**

Run: `make check-boundary`
Expected: PASS — no CS2 name entered core.

- [ ] **Step 6: Commit**

```bash
git add core/src/v8host.rs packages/interfaces/index.d.ts
git commit -m "feat(core)!: host-injects the interface version; publishInterface drops its version arg

The manifest publishes map is now the sole source of an interface's version.
Publishing an undeclared name, or a name a live producer already owns, is refused.
Retires the slice-5 IFACE_METHODS prune TODO: the two-producer rejection makes
one-producer-per-name an enforced invariant, so the name-only key is safe by
construction and iface_call's hot path is untouched."
```

---

### Task 6: Build derives the manifest `publishes` block

Spec §4.2, §4.5. Wire the gate + grammar into `buildPlugin`, and add the embedded verified copy.

**Files:**
- Modify: `packages/cli/src/build.ts:40-120`
- Modify: `packages/cli/test/build.test.mjs`
- Create: `packages/cli/test/fixtures/publisher/` (package.json, api.d.ts, src/plugin.ts)

**Interfaces:**
- Consumes: `assertPublishesTypes` (Task 2); `derivePublishes`, `PublishDecl` (Task 1).
- Produces: a `.s2sp` whose `manifest.json` carries `publishes: {iface: {version, typesSha256}}` and which contains `types/<sanitized-iface>.d.ts` when the plugin publishes.

- [ ] **Step 1: Write the failing test**

Create the fixture `packages/cli/test/fixtures/publisher/package.json`:

```json
{
  "name": "@demo/publisher",
  "version": "2.1.0",
  "main": "src/plugin.ts",
  "types": "api.d.ts",
  "s2script": {
    "apiVersion": "1.x",
    "publishes": "self"
  },
  "private": true
}
```

`packages/cli/test/fixtures/publisher/api.d.ts`:

```typescript
export interface Publisher {
  ping(): boolean;
}
```

`packages/cli/test/fixtures/publisher/src/plugin.ts`:

```typescript
import { publishInterface } from "@s2script/interfaces";
import type { Publisher } from "../api";

const impl: Publisher = {
  ping(): boolean {
    return true;
  },
};

publishInterface("@demo/publisher", impl);
```

Add to `packages/cli/test/build.test.mjs`:

```javascript
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

test("build derives publishes {version, typesSha256} and embeds the contract", async () => {
  const out = await buildPlugin("test/fixtures/publisher", packagesDir);
  const zip = new AdmZip(out);

  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  const decl = manifest.publishes["@demo/publisher"];
  assert.ok(decl, "self sugar must expand to a self-named entry");
  assert.equal(decl.version, "2.1.0", "self takes the package version");

  const contractBytes = readFileSync("test/fixtures/publisher/api.d.ts");
  const expected = createHash("sha256").update(contractBytes).digest("hex");
  assert.equal(decl.typesSha256, expected, "hash is of the contract's raw bytes");

  // The embedded verified copy: redundant, hash-checked, never authoritative.
  const embedded = zip.readFile("types/@demo_publisher.d.ts");
  assert.ok(embedded, "a publishing plugin embeds its contract");
  assert.equal(
    createHash("sha256").update(embedded).digest("hex"),
    decl.typesSha256,
    "the embedded copy must hash to the manifest's typesSha256",
  );
});

test("build of a non-publishing plugin has no publishes block and no types member", async () => {
  const out = await buildPlugin("test/fixtures/hello", packagesDir);
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  assert.equal(manifest.publishes, undefined, "no publishes block when nothing is published");
  assert.equal(zip.getEntries().filter((e) => e.entryName.startsWith("types/")).length, 0);
});

test("build rejects the map form until contract resolution lands", async () => {
  await assert.rejects(
    () => buildPlugin("test/fixtures/publisher-mapform", packagesDir),
    /publishes map form is not yet supported/,
  );
});
```

Create `packages/cli/test/fixtures/publisher-mapform/` mirroring `publisher/` but with `"publishes": { "@community/contract": "^1.0.0" }` in package.json, and `src/plugin.ts` publishing `"@community/contract"`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/build.test.mjs`
Expected: FAIL — `manifest.publishes["@demo/publisher"]` is undefined (build currently copies `publishes` through verbatim)

- [ ] **Step 3: Write minimal implementation**

In `packages/cli/src/build.ts`, add imports at the top:

```typescript
import { assertPublishesTypes } from "./publish-gate.ts";
import { derivePublishes } from "./publishes.ts";
import { readFileSync as readFileSyncRaw } from "node:fs";
```

Add the gate at the top of `buildPlugin`, before the typecheck gate:

```typescript
  // --- publishes ⇒ types gate (before we spend cycles on tsc/esbuild) ---
  const pkgEarly: PluginPackageJson = JSON.parse(readFileSync(join(absDir, "package.json"), "utf8"));
  const gate = assertPublishesTypes(pkgEarly, absDir);
  if (!gate.ok) {
    throw new Error(`publish gate failed: ${gate.error}`);
  }
```

Replace the manifest's `publishes` passthrough (currently `if (publishes !== undefined) manifest.publishes = publishes;`) with:

```typescript
  // The map form needs a registry to resolve a range → a concrete contract version + its bytes
  // (design spec §4.6, §10 — out of scope for this slice). "self" resolves locally, so ship it now
  // and refuse the map form loudly rather than stamping a RANGE where a concrete version belongs.
  if (publishes !== undefined && typeof publishes !== "string") {
    throw new Error(
      `publishes map form is not yet supported (needs registry contract resolution); use "self"`,
    );
  }
  const derivedPublishes = derivePublishes(
    publishes as string | undefined,
    name,
    version,
    gate.typesPath,
  );
  if (Object.keys(derivedPublishes).length > 0) {
    manifest.publishes = derivedPublishes;
  }
```

Add the embedded copy after `zip.addFile("plugin.js", ...)`:

```typescript
  // --- Embedded verified copy (spec §4.5): redundant, hash-checked, NEVER authoritative.
  // core's read_s2sp reads manifest.json/plugin.js by_name and ignores every other member,
  // so this needs no loader change and can be dropped without breaking anyone.
  if (gate.typesPath !== null && Object.keys(derivedPublishes).length > 0) {
    const contract = readFileSyncRaw(gate.typesPath);
    for (const iface of Object.keys(derivedPublishes)) {
      const safe = iface.replace(/[^a-zA-Z0-9._-]/g, "_");
      zip.addFile(`types/${safe}.d.ts`, contract);
    }
  }
```

Also extend the `PluginPackageJson` interface's `s2script` block so `publishes` and `types` typecheck:

```typescript
  types?: string;
  s2script?: {
    // …existing fields…
    publishes?: string | Record<string, string>;
  };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd packages/cli && npm test`
Expected: PASS — the full CLI suite including 3 new build tests

- [ ] **Step 5: Commit**

```bash
git add packages/cli/src/build.ts packages/cli/test/build.test.mjs packages/cli/test/fixtures/publisher packages/cli/test/fixtures/publisher-mapform
git commit -m "feat(cli): derive the publishes manifest block + embed the verified contract copy"
```

---

### Task 7: Migrate zones to the new grammar

Spec §6. The dogfood: the plugin whose drift motivated the design.

**Files:**
- Create: `plugins/zones/api.d.ts` (moved from `packages/zones/index.d.ts`)
- Modify: `plugins/zones/package.json`
- Modify: `plugins/zones/src/plugin.ts:19-21` (imports), `:220` (the publish call)
- Delete: `packages/zones/`
- Modify: `examples/zones-consumer-demo/package.json`

**Interfaces:**
- Consumes: the grammar (Tasks 1, 6); host injection (Task 5).
- Produces: `plugins/zones/api.d.ts` exporting `Vec3`, `Zone`, `ZoneEvent`, `ZoneCreatedEvent`, `ZoneDeletedEvent`, and a `Zones` interface type naming every published method.

- [ ] **Step 1: Move the contract and add the impl-binding type**

```bash
git mv packages/zones/index.d.ts plugins/zones/api.d.ts
```

Edit `plugins/zones/api.d.ts`: replace the module doc comment's first line with:

```typescript
/**
 * @s2script/zones — the zone system's contract, implemented by the first-party zones plugin.
 * As a hard dependency it resolves to a producer-backed proxy that throws `InterfaceUnavailable`
 * while the plugin is unloaded (probe with a method call and defer subscribing if it throws — the
 * producer may load after the consumer). NO runtime code.
 */
```

Append to `plugins/zones/api.d.ts` — the type the implementation binds against, so tsc proves the impl satisfies the contract:

```typescript
/** The published surface, as one object type. The plugin's impl is declared `: Zones`,
 *  so `s2script build` fails if a method is missing or mistyped (spec §4.6). */
export interface Zones {
  createZone(name: string, min: Vec3, max: Vec3): boolean;
  deleteZone(name: string): boolean;
  getZones(): Zone[];
  isInZone(slot: number, name: string): boolean;
  zonesFor(slot: number): string[];
  getZonesByTag(tag: string): Zone[];
  setZoneTags(name: string, tags: string[]): boolean;
}
```

- [ ] **Step 2: Update package.json**

Replace `plugins/zones/package.json` with:

```json
{
  "name": "@s2script/zones",
  "version": "0.3.0",
  "private": true,
  "main": "src/plugin.ts",
  "types": "api.d.ts",
  "s2script": {
    "apiVersion": "1.x",
    "publishes": "self",
    "pluginDependencies": {
      "@s2script/commands": "^0.1.0",
      "@s2script/admin": "^0.2.0",
      "@s2script/chat": "^0.1.0",
      "@s2script/db": "^0.1.0",
      "@s2script/server": "^0.1.0",
      "@s2script/config": "^0.1.0",
      "@s2script/frame": "^0.1.0",
      "@s2script/interfaces": "^0.1.0",
      "@s2script/cs2": "^0.3.0",
      "@s2script/entity": "^0.2.0",
      "@s2script/math": "^0.1.0"
    }
  }
}
```

Version `0.3.0` supersedes **both** drifted numbers (npm stub 0.2.0, runtime publishes 0.1.0) with one. `private: true` stays — zones ships in the runtime zip; it is not an npm package (spec §6).

- [ ] **Step 3: Bind the impl to the contract and drop the version literal**

In `plugins/zones/src/plugin.ts`, add to the import block (after line 21):

```typescript
import type { Zones } from "../api";
```

Replace line 220's opening:

```typescript
  iface = publishInterface("@s2script/zones", "0.1.0", {
```

with:

```typescript
  const zonesImpl: Zones = {
```

Then find the end of that object literal — the `})` closing the `publishInterface(` call — and replace it with:

```typescript
  };
  iface = publishInterface("@s2script/zones", zonesImpl);
```

Locate the exact closing line first:

```bash
awk 'NR>=220 && /^  \}\);/ {print NR": "$0; exit}' plugins/zones/src/plugin.ts
```

**Leave `plugins/zones/src/plugin.ts`'s local `Vec3` (line 23) and `Zone` (line 24) alone — do not import them from `../api`.** They are structurally compatible with the contract's by design:
- local `Vec3 {x,y,z}` is identical to the contract's, so TS structural typing accepts it either way;
- local `Zone` is deliberately **richer** — it carries the runtime-only `inside: Set<number>` and `trigger: TriggerZoneHandle | null` fields that must never cross the interface wire. The impl's `getZones()` already maps it down to the contract's `{name, min, max, tags}` shape.

That split (internal record vs wire shape) is correct and this task preserves it. If tsc complains about the impl's return types, the fix is in the mapping, not in merging the two `Zone`s.

- [ ] **Step 4: Delete the npm stub and re-pin the consumer**

```bash
git rm -r packages/zones
```

In `examples/zones-consumer-demo/package.json`, change the zones pin to match the plugin's version:

```json
    "@s2script/zones": "^0.3.0"
```

- [ ] **Step 5: Verify the build and typecheck gates**

Run: `cd plugins/zones && npx s2script build`
Expected: a `.s2sp` at `plugins/zones/dist/_s2script_zones.s2sp`. Verify the manifest and the vanished literal:

```bash
cd /home/gkh/projects/s2script
unzip -p plugins/zones/dist/_s2script_zones.s2sp manifest.json | python3 -m json.tool
grep -n '"0\.1\.0"' plugins/zones/src/plugin.ts
```

Expected: `publishes` is `{"@s2script/zones": {"version": "0.3.0", "typesSha256": "<hex>"}}`; the grep finds **nothing**.

Run: `./scripts/check-plugins-typecheck.sh`
Expected: PASS — every plugin and example, including zones and zones-consumer-demo.

- [ ] **Step 6: Commit**

```bash
git add plugins/zones examples/zones-consumer-demo/package.json
git add -u packages/zones
git commit -m "refactor(zones)!: one version, contract as api.d.ts, host-injected publish

Collapses the drifted pair (npm stub 0.2.0 / runtime publishes 0.1.0) into a single
0.3.0. The contract moves to plugins/zones/api.d.ts and the impl is typed against it,
so tsc proves the published surface matches. Deletes the packages/zones npm stub."
```

---

### Task 8: Deprecate the npm stub + changeset

Spec §6. `@s2script/zones@0.2.0` is published; deleting the directory does not unpublish it.

**Files:**
- Create: `.changeset/<generated-name>.md`

**Interfaces:**
- Consumes: Task 7's deletion.
- Produces: a changeset covering `@s2script/cli` (minor: the grammar) and `@s2script/interfaces` (major: `publishInterface` signature).

- [ ] **Step 1: Write the changeset**

Run `npm run changeset` and select `@s2script/cli` (minor) and `@s2script/interfaces` (major), or write `.changeset/contract-grammar.md` directly:

```markdown
---
"@s2script/interfaces": major
"@s2script/cli": minor
---

Contract grammar: the host now injects an interface's version.

`publishInterface(name, impl)` no longer takes a version — the host reads it from the
plugin's manifest `publishes` map, and refuses to publish a name the manifest does not
declare. A plugin may no longer type a version string anywhere.

`s2script build` derives `publishes` as `{interface: {version, typesSha256}}` from the
authored `"self"` form, gates `publishes ⇒ types`, and embeds a hash-verified copy of the
contract in the `.s2sp`.

`@s2script/zones` is no longer published to npm — the zones contract ships with the plugin.
```

- [ ] **Step 2: Deprecate the published stub**

This is a manual step requiring npm auth — **do not run it unattended**. Record it in the PR body for the maintainer:

```bash
npm deprecate @s2script/zones@"<=0.2.0" "The zones contract now ships with the zones plugin; this stub is no longer published. See docs/superpowers/specs/2026-07-15-plugin-contract-distribution-design.md"
```

- [ ] **Step 3: Commit**

```bash
git add .changeset
git commit -m "chore: changeset for the contract grammar"
```

---

### Task 9: Full gate suite

CLAUDE.md's pre-PR requirement. No new code — this task is the gate.

**Files:** none modified (fix-forward only if something fails).

- [ ] **Step 1: Run the core suite**

Run: `cargo test -p s2script-core`
Expected: PASS. (Forced single-threaded via `.cargo/config.toml` — do not pass `--test-threads`.)

- [ ] **Step 2: Run the CLI suite**

Run: `cd packages/cli && npm test`
Expected: PASS

- [ ] **Step 3: Run the gate scripts**

```bash
make check-boundary
./scripts/check-plugins-typecheck.sh
./scripts/check-schema-generated.sh
./scripts/check-nav-generated.sh
./scripts/check-events-generated.sh
./scripts/check-csitem-generated.sh
./scripts/test-boundary-nameleak.sh
```
Expected: every one PASS.

- [ ] **Step 4: Build the base plugins**

Run: `./scripts/build-base-plugins.sh`
Expected: every plugin builds, zones included.

- [ ] **Step 5: Commit any fixes**

```bash
git commit -am "fix: gate suite fallout"   # only if a gate required changes
```

---

### Task 10: Live gate on the Docker CS2 server

CLAUDE.md + memory (`live-gate-boot-window`). The host must inject zones' version on a real server and a real consumer must bind through it.

**Files:** none.

**Interfaces:**
- Consumes: everything.

- [ ] **Step 1: Build deployable binaries**

Host glibc is too new — build in the bullseye container:

```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expected: `s2script.so` (GLIBC_2.14) + `libs2script_core.so` (GLIBC_2.30), repackaged into `dist/`.

**Gotcha (memory `cs2-admin-and-deploy`):** after a sniper build, recreate `dist/addons/s2script/configs` as `gkh` or the container's config auto-gen write fails.

- [ ] **Step 2: Deploy zones + the consumer demo**

Build both and copy only these two `.s2sp` files into the server's plugins dir. **Do not deploy `examples/*` wholesale** — stale bundles fail `onLoad` and a hard-dep consumer then spams every frame (memory `live-gate-boot-window`).

```bash
cd plugins/zones && npx s2script build && cd -
cd examples/zones-consumer-demo && npx s2script build && cd -
```

- [ ] **Step 3: Restart and wait out the boot window**

```bash
docker compose -f docker/docker-compose.yml restart cs2
```
**NOT** `--force-recreate` (resets `gameinfo.gi`), **NOT** `meta-reload`. Demos arm only after the boot window.

- [ ] **Step 4: Verify the host injected the version and the consumer bound**

```bash
docker logs s2script-cs2 2>&1 | grep -i "zones\|iface_publish\|InterfaceUnavailable"
python3 scripts/rcon.py "sm_zone_add livegate"
python3 scripts/rcon.py "sm_zones"
```

Expected:
- **No** `iface_publish` WARN — zones' manifest declares `@s2script/zones`, so the publish is accepted.
- **No** `InterfaceUnavailable` from the consumer — its `^0.3.0` range matches the host-injected `0.3.0`.
- `sm_zone_add` / `sm_zones` behave as before the migration.

**If the consumer reports a version mismatch:** check the injected version against the pin — `unzip -p plugins/zones/dist/_s2script_zones.s2sp manifest.json`. Note that `version_satisfies` is still major-only (out of scope, spec §10), so `^0.3.0` vs `0.3.0` matching proves nothing about minor drift; it only proves the version *arrived* from the manifest.

- [ ] **Step 5: Verify the negative — an undeclared publish is refused**

Temporarily prove the refusal path on the live host. Build a throwaway plugin that publishes a name its manifest does not declare:

```bash
mkdir -p /tmp/claude-1000/-home-gkh-projects-s2script/cc657ab4-8d55-447b-9e98-87cda0a7a327/scratchpad/undeclared/src
```

`package.json`:
```json
{ "name": "@demo/undeclared", "version": "0.1.0", "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x" }, "private": true }
```

`src/plugin.ts`:
```typescript
import { publishInterface } from "@s2script/interfaces";
publishInterface("@demo/never-declared", { ping: () => true });
```

Build it, drop the `.s2sp` in the plugins dir, and check the log:

```bash
docker logs s2script-cs2 2>&1 | grep "never-declared"
```
Expected: `WARN: iface_publish('@demo/never-declared'): plugin '@demo/undeclared' did not declare this interface in its manifest \`publishes\` — refusing`

Then remove the `.s2sp` (file-watch unloads it; no restart needed).

- [ ] **Step 6: Record the result**

Append a `docs/PROGRESS.md` entry for the slice: what it built, why, and the live-gate result.

```bash
git add docs/PROGRESS.md
git commit -m "docs: PROGRESS entry for the contract grammar slice"
```

---

### Task 11: PR

**Files:** none.

- [ ] **Step 1: Rebase onto current main**

```bash
git fetch origin && git rebase origin/main
```
Re-run `cargo test -p s2script-core` and `./scripts/check-plugins-typecheck.sh` after the rebase.

- [ ] **Step 2: Open the PR**

```bash
gh pr create --title "feat!: plugin contract grammar — host-injected interface versions" --body "$(cat <<'EOF'
Implements the freeze target of docs/superpowers/specs/2026-07-15-plugin-contract-distribution-design.md (§4.2, §4.3, §4.5, §4.8, §6).

## What

- **`publishes` grammar** (§4.2): authored as `"self"` or a `{interface: range}` map; derived into the manifest as `{interface: {version, typesSha256}}`. The interface name is decoupled from the package name — `@edge/mce` may publish `@community/mapchooser`.
- **Host-injected versions** (§4.3): `publishInterface(name, impl)` drops its version parameter. The host reads the version from the manifest and refuses to publish a name the manifest does not declare. **No version string may appear in TypeScript source.**
- **Two-producer rejection** (§4.8): a second live producer of an interface name is refused; implementations are alternatives.
- **Embedded verified copy** (§4.5): a publishing plugin's `.s2sp` carries a hash-checked copy of its contract. Redundant, never authoritative — `read_s2sp` ignores it.
- **Zones dogfood** (§6): the drifted pair (npm stub 0.2.0 / runtime `publishes` 0.1.0, with the tags API implemented but unversioned) collapses to a single 0.3.0. The contract moves to `plugins/zones/api.d.ts`, the impl is typed against it, and `packages/zones` is deleted.

## Why

`plugins/zones/src/plugin.ts:220` published the literal `"0.1.0"` while implementing the tags API the 0.2.0 npm stub documented. Three hand-maintained version numbers in three places, and `version_satisfies` being major-only meant nothing caught it. This removes two of the three sites and makes the third impossible to type.

## Breaking

- `publishInterface` is 2-arg. Every producer must move its version into the manifest via `publishes`. Pre-users; no migration path is provided by design.
- `@s2script/zones` is no longer published to npm.

## Follow-ons (deliberately out of scope, spec §10)

- **Semver unification** — `version_satisfies` is still major-only, so this grammar is correct but under-enforced. Hard follow-on.
- Registry contract artifacts, `s2script add`/`install`, virtual deps.
- `@s2script/*` stub consolidation; the `/npm/*` facade.

## Maintainer action required

`npm deprecate @s2script/zones@"<=0.2.0" "..."` — needs npm auth, not run here.

## Gates

- [ ] `cargo test -p s2script-core`
- [ ] `packages/cli && npm test`
- [ ] `make check-boundary` + every `scripts/check-*.sh`
- [ ] Live Docker CS2: zones publishes at the injected 0.3.0, consumer binds, undeclared publish refused

https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf
EOF
)"
```

---

### Task 12: Pin the semver hole for the follow-on spec

Spec §8, §10. `version_satisfies` stays major-only in this slice. Leave an executable record of exactly what is broken so the follow-on spec inherits a failing-test target rather than prose.

**Files:**
- Modify: `core/src/interfaces.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the characterization tests**

Add to `core/src/interfaces.rs`'s test module. These assert **current, wrong** behaviour and are the follow-on's starting point:

```rust
    // --- Characterization: version_satisfies is MAJOR-ONLY (design spec §10). ---
    // These document the hole this slice does NOT fix. The semver-unification spec
    // inverts every assertion below; until then they lock in what we know is wrong.

    #[test]
    fn characterize_major_only_matching_accepts_wrong_minors_pre_1_0() {
        // npm semantics: ^0.1.0 pins the minor, so 0.2.0 must NOT satisfy it.
        // We accept it. Pre-1.0, every range matches every version.
        assert!(version_satisfies("^0.1.0", "0.2.0"), "KNOWN WRONG: 0.x caret ignores the minor");
        assert!(version_satisfies("^0.1.0", "0.99.0"), "KNOWN WRONG");
        assert!(version_satisfies("^0.2.0", "0.1.0"), "KNOWN WRONG: this is the zones drift");
    }

    #[test]
    fn characterize_major_only_matching_accepts_older_minors_post_1_0() {
        // A consumer that typechecked against a 1.2.0 contract binds a 1.0.0 producer
        // that lacks the methods 1.2.0 promised → the proxy throws at call time.
        assert!(version_satisfies("^1.2.0", "1.0.0"), "KNOWN WRONG: older minor satisfies a caret");
    }

    #[test]
    fn characterize_major_mismatch_is_correctly_refused() {
        // The one thing major-only DOES get right.
        assert!(!version_satisfies("^1.0.0", "2.0.0"));
        assert!(!version_satisfies("^2.0.0", "1.0.0"));
    }
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p s2script-core characterize_`
Expected: PASS — all 3. They assert what the code does today, so they pass immediately; that is the point.

- [ ] **Step 3: Commit**

```bash
git add core/src/interfaces.rs
git commit -m "test(core): characterize the major-only version_satisfies hole

Executable record of what the contract grammar does NOT fix. The semver-unification
spec inverts these assertions."
```

---

## Spec Coverage

| Spec section | Task | Notes |
|---|---|---|
| §4.1 two artifact kinds | 6, 7 | The *plugin* half. Contract-as-publishable-artifact needs the registry → plan 2. |
| §4.2 `publishes` grammar | 1, 3, 6 | Authored + derived forms; `"self"` sugar; raw-bytes hashing. Map form refused (Task 6) pending registry resolution. |
| §4.3 host-injected version | 5 | Includes the `.d.ts` signature change. |
| §4.4 drift impossible | 1, 5, 6 | Mechanism (i) single version source + (iii) host injection. Mechanism (ii) registry hash *verification* → plan 2; this slice computes and stamps the hash. |
| §4.5 embedded verified copy | 6 | Plus Task 7's live proof. |
| §4.6 producer path (self) | 6, 7 | Producer path (other's contract) → plan 2. Author/operator paths → plan 2. |
| §4.7 virtual dependencies | — | **Plan 2.** Needs the registry resolve endpoint. |
| §4.8 two live producers | 4, 5 | Registry-level rejection + the host-level refusal. |
| §6 zones migration | 7, 8 | Including the npm deprecation as a flagged manual step. |
| §7 testing | 1–7, 9, 10 | Live gate covers the §7 "zones loads, publishes, consumer binds" line. |
| §8 risks | 12 | The semver risk gets executable characterization tests. |
| §10 out of scope | 12 | Pinned, not built. |
