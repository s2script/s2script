# Plugin contract distribution — design spec

**Date:** 2026-07-15
**Status:** design (pending approval) → implementation plan next
**Scope:** the plugin *contract* layer — `manifest.publishes` grammar, contract-as-artifact, host-injected interface versions, CLI author/operator paths, registry deploy invariants. Amends the branch design at `docs/superpowers/specs/2026-07-12-registry-orgs-publish-design.md` (on `cursor/registry-orgs-publish-design-6c6d`).

**Depends on:** [semver unification](#10-out-of-scope-separate-specs) (separate spec) — this design is correct without it but under-enforced until it lands.

## 1. Goal

Let a community developer publish a plugin that **exposes an API**, such that a consumer gets the runtime dependency *and* its types with **one version number and no possible drift**, while keeping the **contract** (a compile-time `.d.ts`) and the **implementation** (a runtime `.s2sp`) as separately addressable artifacts.

The concrete north-star pattern, taken from SourceMod and already present in our own base-plugin suite: `mapchooser.inc` is a contract implemented by **both** stock `mapchooser` and `mapchooser_extended`; `nominations` and `rtv` depend on the *interface*, which is exactly why MCE is a drop-in replacement. We ship `nominations`, `rockthevote`, and `nextmap` today (`disabled/`). Any model in which "publish a contract" requires shipping a plugin to carry it fails our own acceptance suite.

## 2. The problem, with evidence

**Today a first-party interface has three version numbers in three places, and nothing reconciles them.**

| # | Site | Value today | Form |
|---|---|---|---|
| 1 | `packages/zones/package.json` (npm) | `0.2.0` | package.json field |
| 2 | `plugins/zones/package.json` `s2script.publishes` | `0.1.0` | package.json field |
| 3 | `plugins/zones/src/plugin.ts:220` `publishInterface("@s2script/zones", "0.1.0", …)` | `0.1.0` | **string literal in TS source** |

They have **already drifted**: `plugins/zones/src/plugin.ts:250,253` implement `getZonesByTag`/`setZoneTags` — the tags API the `0.2.0` npm stub documents — while the plugin still publishes `"0.1.0"`.

Site 3 is the decisive one: `core/src/interfaces.rs` compares a consumer's declared range against the **published entry version**, which comes from the JS call, not the manifest. Sites 1 and 2 are bystanders at runtime. `core/src/v8host.rs:8985-8987` carries a TODO admitting that manifest-vs-runtime `publishes` cross-validation does not exist.

Nothing catches the drift because `core/src/interfaces.rs:50 version_satisfies` is **major-only**:

> A concrete `version` satisfies `range` iff `range` is `*`, OR both have a leading major and the majors match.

While the ecosystem is pre-1.0, `^0.1.0`, `^0.2.0` and `0.99.0` all mutually satisfy. `examples/zones-consumer-demo` pins `^0.1.0` (tracking site 3) while npm serves `0.2.0` (site 1). Both resolve. Neither is checked.

**Root cause:** the implementation ships in the runtime zip and the contract ships on npm — two channels, two hand-maintained versions. The drift is the model leaking, not a slip. The branch design doc reached this same conclusion independently and rejected the split-brain model by name; `@s2script/zones` is the last artifact still living in it.

**Secondary:** the `@s2script/*` npm scope conflates two contracts that look identical at the import site — always-present runtime builtins (`@s2script/entity`, backed by a core prelude) and a plugin-published interface whose presence depends on a producer being loaded (`@s2script/zones`). A consumer cannot tell them apart by looking.

## 3. Prior art — most of this is built

On `cursor/s2script-install-lockfile-5823` (which descends from `cursor/registry-orgs-publish-design-6c6d`):

- **`packages/cli/src/publish-gate.ts:37 assertPublishesTypes`** — if `s2script.publishes` is non-empty, `package.json` `types` must point at an existing non-empty `.d.ts`. Wired into `build.ts:44-51` before tsc/esbuild.
- **`packages/cli/src/types-pack.ts packTypesTarball`** — hand-rolled gzip+ustar packer producing `package/package.json` + `package/api.d.ts`, already stamping `s2script.kind: "interface"` (line 64).
- **`packages/cli/src/registry/deploy.ts`** — runs the gate, builds the `.s2sp`, packs types from the *same* `package.json` name+version, POSTs `{manifest, s2sp, types}` as one multipart.
- **`website/src/lib/server/registry/deploy.ts:44-49`** — server re-enforces: `publishes` set but no types tarball → `400 publishes_requires_types`.
- **`website/src/routes/api/v1/deploy/+server.ts:27-29`** — `s2sp file field required`; types is optional.
- **`packages/cli/src/registry/add.ts`** (author path: types → `.s2script/types/<pkg>/api.d.ts`), **`install.ts`** (operator path: BFS closure, sha256, `s2script-lock.json`), **`lockfile.ts`**, **`builtins.ts`** (mirrors `core/src/loader.rs` BUILTIN_MODULES).
- **`typecheck.ts` change** — deps with a local `.s2script/types/<pkg>/` tree resolve to the real `.d.ts`; the rest fall back to today's ambient `declare module "<dep>";` (`any`) stub.

**The branch's dual-artifact instinct was right.** This spec keeps it and adds three things it lacks: a contract **hash** in the manifest, a **standalone-contract** publish path, and **host-injected** interface versions.

## 4. Design

### 4.1 Two artifact kinds

| | **Contract** | **Plugin** |
|---|---|---|
| Content | `package.json` (`s2script.kind: "interface"`) + `index.d.ts` | `manifest.json` + `plugin.js` (+ optional `types/<iface>.d.ts`) |
| Address | `@scope/name@version` | `@scope/name@version` |
| Canonical for | the API | the implementation |
| Authored | standalone, **or** derived from a plugin's `api.d.ts` when `publishes: "self"` | always |
| Consumed by | plugin authors (compile time) | operators (runtime) |

A contract is publishable **with no plugin behind it**. This requires relaxing `website/src/routes/api/v1/deploy/+server.ts:27-29` (which currently hard-requires an `.s2sp`) and adding a `kind` discriminator server-side.

### 4.2 The `publishes` grammar — the freeze target

**Two forms.** The *authoring* format (`package.json`, hand-written) carries a **range**; the *manifest* (derived into the `.s2sp` by the CLI, never hand-authored) carries the **resolved concrete version + hash**:

```jsonc
// package.json — AUTHORED
"s2script": { "publishes": { "@community/mapchooser": "^1.2.0" } }
```
```jsonc
// manifest.json — DERIVED (inside the .s2sp)
"publishes": {
  "@community/mapchooser": {              // the INTERFACE name — not the package name
    "version": "1.2.0",                   // the CONTRACT's resolved version, not the plugin's
    "typesSha256": "<sha256 of the contract's index.d.ts>"
  }
}
```

`typesSha256` is the sha256 of the contract `index.d.ts`'s **raw published bytes — no normalization** (no line-ending or whitespace canonicalization). The registry stores the contract's bytes and hashes them the same way; any normalization step would be a second source of truth and is therefore forbidden.

Three properties, each load-bearing:

1. **Interface name is decoupled from package name.** `@edge/mapchooser-extended@3.1.0` may publish `@community/mapchooser@1.2.0`. This is what makes drop-in alternatives expressible. **The core already supports this** — `InterfaceRegistry` keys entries by interface name with `producer_id` as a separate field, and `publishes` is already a map. Only the branch's packaging layer collapsed name-equals-name (§3 of that doc); we are undoing a decision, not adding a mechanism.
2. **`version` is the contract's**, so it moves only when the API moves — not on every implementation bugfix. This is strictly better semantics for `version_satisfies` to target.
3. **`typesSha256` anchors the contract without carrying it.** The CLI hashes the exact `.d.ts` the typecheck gate read; the registry verifies at deploy that the claimed contract's stored bytes hash to the same value. **Any copy of the `.s2sp`, anywhere, can prove which contract it implements without embedding it.**

**Sugar for the dominant case:** `"publishes": "self"` → interface name = package name, version = package version, contract = the repo's `api.d.ts`. Every plugin we ship today (zones) uses this and keeps a one-repo / one-version / one-command flow.

`"self"` is sugar for exactly one self-contract and **does not compose** — a plugin that publishes its own contract *and* implements someone else's, or publishes two contracts, must use the map form and name itself explicitly. The CLI expands `"self"` to the map form before deriving the manifest, so the manifest has exactly one shape.

**Known tension with `"self"`:** it ties the contract version to the *package* version, so a pure bugfix release bumps the contract version even though the API did not move — the thing property 2 exists to avoid. This is accepted: caret ranges absorb it, and the alternative (a second hand-maintained version in the same repo) is precisely the failure in §2. A plugin that outgrows this — one whose API stabilizes while its implementation churns — should graduate to the map form and version its contract independently. That graduation is a package.json edit, not a migration.

### 4.3 Host-injected interface version

`publishInterface` **loses its version parameter**:

```ts
publishInterface("@community/mapchooser", { /* impl */ });   // no version string, ever
```

The host reads the version from the baked manifest and **fails the load** if a plugin publishes a name absent from its `publishes` map. This closes site 3 (the hand-typed literal) and the `v8host.rs:8985-8987` TODO in one move. After this, **no version string appears in TypeScript source anywhere.**

### 4.4 Why drift is then impossible

Three compounding mechanisms; contract and implementation stay separately addressable throughout:

| Mechanism | Kills | Where enforced |
|---|---|---|
| Single version source + atomic dual publish | authoring drift (the zones failure) | `deploy.ts` CLI + `deploy.ts` server (`publishes_requires_types`, already built) |
| `typesSha256` in the manifest | smuggling a mismatched contract past the registry, or into a shared file | registry at deploy; any consumer of a loose `.s2sp` |
| Host-injected version | runtime drift (site 3) | core, at load |

**Owning the hosting is what makes this work.** The registry is the single choke point every publish passes through, which converts the first two from CLI courtesy into ecosystem law. No artifact-level cleverness achieves that.

### 4.5 Embedded verified copy (optional, derived, reversible)

The `.s2sp` **may** carry `types/<iface>.d.ts` as a redundant copy, hash-checked against the manifest at build and by any consumer of the file. This makes a forum-attachment `.s2sp` self-describing *and* self-proving, enabling `s2script add ./thing.s2sp` offline.

It is **never authoritative** — the contract artifact is. `core/src/loader.rs:103-131 read_s2sp` reads `manifest.json` and `plugin.js` `by_name` and ignores all other members, so **the loader needs zero changes** and this is additive/reversible at any time.

**Explicitly rejected:** making the embedded copy canonical. A runtime deployable is the wrong home for a compile-time contract; it forecloses the mapchooser pattern (§1), forces a closed-source plugin to upload its implementation to publish an API, and does not even buy registry-optionality — the normal author path (`s2script add @scope/x` from the registry) requires the registry regardless.

### 4.6 Paths

**Producer** (self-contract — every plugin we ship today):
```
plugins/zones/
  package.json    name=@s2script/zones, version=1.2.0, types=api.d.ts,
                  s2script.publishes="self"
  api.d.ts        the contract
  src/plugin.ts   import type * as Api from "../api";
                  const impl: Api.Zones = { … };          // tsc proves the impl satisfies the contract
                  publishInterface("@s2script/zones", impl);   // no version
$ s2script deploy   # gate → build → hash → atomic {contract, .s2sp} upload
```

**Producer** (implementing someone else's contract):
```jsonc
// @edge/mapchooser-extended@3.1.0
"s2script": { "publishes": { "@community/mapchooser": "^1.2.0" } }
```
The CLI resolves the range to a concrete contract, downloads its bytes, typechecks the impl against them, and stamps `{version, typesSha256}` into the manifest. **Publishing an implementation of someone else's contract is open; publishing the contract itself requires scope ownership.** Unforgeable either way, because the hash must match the registry's stored bytes.

**Consumer (author):**
```
$ s2script add @community/mapchooser@^1     # CONTRACT ONLY — never an .s2sp
    → .s2script/types/@community/mapchooser/index.d.ts
    → pluginDependencies["@community/mapchooser"] = "^1.2.0"   (real resolved range)
    → tsconfig paths cover .s2script/types/                    (editor == gate, one location)
$ s2script add ./thing.s2sp                 # offline: extract embedded copy, verify vs manifest hash
```

**Operator:**
```
$ s2script install @edge/mapchooser-extended   # .s2sp closure + lockfile; branch code as-is
```

### 4.7 Virtual dependencies

Once a dep may name an interface, `@community/mapchooser` no longer identifies an `.s2sp`. `install.ts`'s BFS needs a **provides-index** and a policy (Debian virtual-package semantics):

- exactly one implementation → take it;
- several → the operator must choose (error listing candidates);
- `publishes: "self"` plugins → degenerates to today's behavior, so nothing we ship changes.

### 4.8 Two live producers of one interface

`InterfaceRegistry::publish` currently overwrites an existing entry while preserving subscribers — correct for republish-on-reload, **wrong for a genuinely different producer**. Second-load of a *different* `producer_id` publishing a live interface name must be **rejected with a named error**. Implementations are alternatives: you run mapchooser *or* MCE, never both.

## 5. Boundary & charter

- **Engine-generic.** Interfaces, the ledger, `publishes`, and version matching are Source2-agnostic. No game names enter core. `check-core-boundary.sh` needs no change.
- **`package.json` is the authoring format.** `publishes` lives under the `s2script` block, not in npm fields. The `.s2sp` consumes a **derived minimal manifest** — `publishes` gaining structure is exactly that field carrying engine facts, as intended.
- **"Never overload npm's `exports`"** is untouched — we add no `exports` semantics.
- **The ledger stays the teardown authority.** Contract identity changes what a dep *names*; it does not change ledgering or reverse-dependency unload.
- **Typecheck-gate every load.** Consumers now gate against the **real** contract bytes rather than an ambient `any` stub — strictly stronger.
- **Base plugins are the acceptance test.** Zones is the dogfood (§6); mapchooser-style alternatives are the pattern the model must express.

## 6. Migration — zones is the dogfood

1. `packages/zones/index.d.ts` → `plugins/zones/api.d.ts`. Delete `packages/zones`; deprecate `@s2script/zones@0.2.0` on npm.
2. `plugins/zones/package.json`: drop `private`, set `types: "api.d.ts"`, `publishes: "self"`, single version.
3. `plugins/zones/src/plugin.ts`: type the impl against `../api`; drop the `"0.1.0"` literal from `publishInterface` (:220).
4. Re-pin `examples/zones-consumer-demo` via `s2script add`.
5. Remove `@s2script/zones` from any builtin list; it is a registry artifact now, not a builtin.

**After this, `@s2script/*` on npm contains only always-present runtime builtins** — the §2 conflation is gone **by construction, with no rename.**

## 7. Testing

- **Unit:** `publishes` grammar parse (`"self"` sugar, explicit map, malformed); hash computation/verification; `version_satisfies` (deferred to the semver spec, but this design's tests pin the table).
- **Gate:** `publishes` naming an interface absent from the manifest → load fails with a named error; second producer of a live interface → rejected; manifest hash ≠ contract bytes → deploy rejected.
- **In-isolate (`cargo test -p s2script-core`):** host-injected version reaches the registry entry; a consumer's range resolves against the contract version.
- **CLI:** `s2script add` from registry and from a local `.s2sp`; tsconfig paths make editor and gate read one location; virtual-dep resolution (one impl / several / none).
- **Live gate (Docker CS2):** zones loads, publishes, and `zones-consumer-demo` binds and calls through — the existing zones live path, re-run post-migration.

## 8. Risks

| Risk | Mitigation |
|---|---|
| **Under-enforced until semver lands.** Major-only matching means a wrong contract version still binds. | This design is correct independently; the semver spec is a hard follow-on. Sequence it next, not later. |
| **`builtins.ts` mirrors `core/src/loader.rs` BUILTIN_MODULES** — a second copy of a list already stale (missing `translations`, `usercmd`, `net`). | Out of scope here, but note it: two hand-maintained copies of one list is the same class of bug as the three version sites. Flag for the semver/consolidation spec. |
| Registry must unzip/verify hashes at deploy. | ~10 lines; it already parses the manifest. |
| Virtual deps add a resolve-endpoint concept. | `publishes: "self"` degenerates to current behavior; nothing shipped today changes. |
| Contract-artifact publish path relaxes an `.s2sp`-required invariant. | Replace with a `kind` discriminator enforced server-side; contract publish requires scope ownership. |

## 9. What must be decided now

**The `publishes` grammar (§4.2) plus contract-as-artifact (§4.1) and host-injected versions (§4.3).** That grammar is baked into every `.s2sp` ever redistributed and every consumer import. Registries, websites, the semver fix, and embedded-vs-sibling packaging are all revisable behind an API; the bytes in a community artifact are not.

Everything else in this spec is reversible.

## 10. Out of scope (separate specs)

- **Consumer contract resolution (`s2script add`) — a DEBT this design creates.** §6 deletes `packages/zones`, which is correct, but it removes the only way a consumer resolved `@s2script/zones`'s types. Until `s2script add` extracts a contract to `.s2script/types/<iface>/index.d.ts` (§4.6), a consumer's imports from a plugin-published interface are `any`:
  - `examples/zones-consumer-demo` types its VALUE imports (`on`, `getZones`) as `any` via the gate's ambient stub, and reaches across the monorepo (`../../../plugins/zones/api`) for its TYPE imports — legal and zero-drift, but only because it lives in this repo. A real consumer has neither option.
  - An ambient `declare module` **cannot** substitute: TypeScript forbids relative re-exports inside one (TS2439, and `skipLibCheck` swallows the error), so the only ambient option is hand-copying the contract — which is the drift this design exists to eliminate. `examples/greeter-consumer/src/greeter.d.ts` is exactly that hand-copy, and is now at least gate-compiled.
  - **This is the first thing plan 2 must build.** It is not optional polish; the consumer story is `any` until it lands.

- **Semver unification** — replace `core/src/interfaces.rs:50` major-only matching with real caret semantics *including the npm `^0.x` minor-pin rule*, specified once and table-tested against both core and `website/src/lib/server/registry/semver.ts` (whose caret is currently wrong for 0.x in the npm-incompatible direction, accepting `0.2.0` for `^0.1.0`). Also fixes `install.ts updatePackages` widening every pin to `^<major>.0.0`. **Hard follow-on to this spec.**
- **`@s2script/*` stub consolidation (29 → 1 `s2script` with subpaths)** — after §6, this is pure ergonomics, not correctness. If wanted, it must land **before the registry launches, never after**.
- **The `/npm/*` facade + `.npmrc` writing** — park. It buys editor resolution obtainable via tsconfig paths, and today causes a dual-copy seam (gate reads `.s2script/types/`, editor reads `node_modules/`) that can silently disagree. Revisit only if third-party tooling interop (Renovate, typedoc) is demanded.
