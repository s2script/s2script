# Game-Package Boundary and Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Select a verified first-party game package from data, bootstrap it in every plugin context, and move CS2 function semantics onto the shared S1/S2 facilities without changing any public CS2 behavior.

**Architecture:** Packaging emits one deterministic manifest plus hashed JavaScript and gamedata artifacts under `addons/s2script/game-packages/`. The shim supplies only detected engine/game/platform tokens and the addon root; Rust uses its existing `sha2` dependency to select, confine, verify, normalize, and atomically register the package through S2, then iterates that registry for every context. CS2 JavaScript adapters run synchronously over generic S2 projected frames; core and shim retain only generic package, function, codec, lifetime, and receipt machinery.

**Tech Stack:** C++17 shim and stock Metamod KHook, Rust/V8 core (`sha2`, `serde`, `serde_json`), JavaScript game package, TypeScript declaration packages, Node.js packager/tests, JSON/JSONC, SHA-256.

**Spec:** `docs/superpowers/specs/2026-09-22-game-package-boundary-design.md`

## Global Constraints

- Isolated implementation may start after source and official-stock-host baselines are recorded. Required human/client, peer, map and script-reload evidence remains a merge/release gate. The user explicitly made the known whole-process shutdown-only SIGSEGV/139 non-blocking: preserve its existing evidence, do not relabel it as a pass, and do not add quit loops, shutdown investigation or a clean-exit wait. Native callback retirement and context/handle lifetime tests remain mandatory because they protect normal use and script reload.
- Base the implementation on integrated S1 and S2 commits; inherited merge/release evidence may still be pending under the ruling above. S1 owns checked resolution and KHook receipts. S2 owns function normalization, ABI/projection support, policy registration, status/provenance, permissions, and subscription teardown.
- Preserve `@s2script/cs2`, every public subpath/type, `Player`, `Pawn`, schema wrappers, damage, acquisition, HUD, base-plugin imports, null behavior, mutation/suppression, self-call behavior, unload behavior, and callback order.
- The deployed manifest syntax is exactly `schemaVersion`, `packages[].id`, `match.engine`, `match.game`, `gamedataOwner`, `bootstrap.{path,sha256}`, and `gamedata.{path,sha256}`. Digests are lowercase 64-character SHA-256 strings.
- Manifest paths are addon-relative and must resolve beneath `addons/s2script/game-packages/`. Reject absolute paths, `..`, symlink escapes, non-regular files, and duplicate resolved artifact paths.
- Preserve the two independent dimensions: descriptor `owner` says who declares and overrides it; descriptor `target` says which engine/game/platform it matches.
- `gamedataOwner: "cs2"` must continue to apply operator files from `addons/s2script/gamedata/cs2/custom/` after the verified shipped artifact. Existing operator repairs must not become invisible.
- Zero package matches leaves generic runtime services available and rejects plugins that require a game package. Multiple matches fail and name every candidate. Duplicate package ids or owner mappings fail before registration.
- Package selection/source/data registration lasts for one shim load. Per-context package instances, borrowed epochs, bindings, subscriptions, wrappers, and handles are owner-and-generation gated.
- Candidate preparation failures before plugin retirement preserve the running generation. Failure after bootstrap/activation starts tears down the candidate safely and reports unavailable; do not promise rollback.
- No package manifest field names or loads a `.so`. No new detour/scanner path is allowed. KHook remains the single interception provider.
- Consume S2's bounded Linux x86_64 scalar ABI vectors/fingerprints: receiver plus ordered `void/u8/i32/u32/i64/u64/f32/f64/ptr` atoms (`void` only for returns and `u8` only for the proven bool projection), with no varargs, aggregates, or unusual ABI. S2's private/static pinned libffi CIF/closures marshal values while stock KHook low-level callbacks retain detour ownership.
- Native projection code may implement bounded reusable codecs such as a callback-scoped struct view; it must not attach CS2 field meaning, expose a raw pointer, or add a gameplay-capability catalog.
- The synthetic second-game fixture proves only selection/bootstrap/owner isolation and one ordinary S2 function without a native rebuild. It is not evidence that another game runs.

## S2 prerequisite interface used by this plan

S3 starts by rebasing these names onto the integrated S2 implementation. If S2 lands an equivalent under another file/name, update this interface block and every S3 call site in one mechanical planning commit before implementation; do not add a parallel registry.

```rust
// S2-owned core/src/engine_functions/{contract,registry,policy,projection,binding,package_adapter}.rs
pub struct OwnerKey { pub id: String, pub generation: u64, pub kind: OwnerKind }
pub enum OwnerKind { Plugin, GamePackage }

pub fn prepare_owner(owner: OwnerKey, bundle: NormalizedBundle, overrides: OverrideSet)
    -> Result<PreparedOwnerReceipt, FunctionError>;
pub fn activate_owner(receipt: PreparedOwnerReceipt)
    -> Result<ActiveOwnerReceipt, FunctionError>;
pub fn drop_owner(owner: &OwnerKey);
pub fn status(owner: &OwnerKey, local_name: &str) -> FunctionStatus;

pub trait DispatchAdapter {
    fn pre(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<PreDecision, AdapterError>;
    fn post(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<(), AdapterError>;
}
pub struct PackageInstanceKey { pub parent: OwnerKey, pub package_owner: OwnerKey }
pub(crate) struct ImplementationManifestHash(pub String); // validated lowercase SHA-256 hex
pub struct PackageAdapterRegistration {
    pub instance: PackageInstanceKey,
    pub id: AdapterId,
    pub contract_hash: ContractHash,
    pub implementation_manifest_hash: ImplementationManifestHash,
    pub callbacks: PackageJsCallbacks,
}
pub struct SubscriberDelivery {
    pub action: HookResult,
    pub return_value: Option<NativeValue>,
    pub frame_revision: u64,
}
pub enum PreDecision {
    Continue,
    Changed,
    Suppress { action: SuppressAction, return_value: Option<NativeValue> },
}
pub enum SuppressAction { Handled, Stop }
```

`OwnerKind::GamePackage` can be minted only by the host's verified package path. S2's `NormalizedBundle` contains `NormalizedFunction`, `AbiSignature`, `ProjectionSpec`, and `PolicySpec`; binding/status/provenance use `FunctionBindingId`, `FunctionSubscriptionId`, `BindingStatus`, `BindingReceipt`, and `SubscriptionReceipt`. `core/src/engine_functions/package_adapter.rs` owns `DispatchAdapter`, `PackageAdapterRegistration`, `PackageJsCallbacks`, `AdapterDispatch`, `SubscriberCursor`, and `PackageInstanceKey { parent: OwnerKey, package_owner: OwnerKey }`; the hidden bootstrap token derives the instance key, never JavaScript arguments. `SubscriberCursor::invoke_next()` returns `Result<Option<SubscriberDelivery>, AdapterError>`, where delivery contains action, optional `NativeValue`, and frame revision. `PreDecision` is `Continue`, `Changed`, or `Suppress { action: Handled|Stop, return_value }`. `policy.rs` holds the process semantic `AdapterContract { id, version, contract_hash, visibility }`, whose hash covers canonical id/version/frame-delivery-decision/timing contract and remains unchanged across the Rust-to-JavaScript relocation. Implementation bytes use separate `implementation_manifest_hash` provenance. During a host-minted package bootstrap window only, `__s2_function_adapter_register(id, contractHash, {pre,post})` registers synchronous callbacks for the derived instance and returns a ledgered receipt. Package wrappers call `__s2_function_adapter_subscribe(bindingId, adapterId, phase, wrapper)` and receive an ordinary generation-gated `FunctionSubscription`. Damage uses generic codec `borrowed-record.v1`; its CS2 field schema/layout and instance hash stay in package data.

Multiple contexts may register the same id/hash under different instance keys. Reject only duplicate `(instance,id)`, a global id/version hash conflict, direct plugin calls, async callbacks, or registration after activation. Domain dispatch chooses the first active eligible instance deterministically, skipping unloaded/busy instances and the caller's instance for `bypass-own-hooks`; if subscribers remain but no instance is eligible, fail by name without deferral. Parent or package teardown removes only matching instances/subscriptions, so peer contexts survive.

## Proposed file responsibilities

| Path | Responsibility | Writer |
| --- | --- | --- |
| `games/cs2/game-package.jsonc` | Source manifest: package id, match, owner, ordered bootstrap inputs, gamedata source root | S3-PKG-01 |
| `games/cs2/adapters/contracts/{legacy.acquire.v1,legacy.hud-click.v1}.json` | Two byte-exact canonical contracts copied from S2; independent of JS source hashes | S3-ADAPT-05 |
| `games/cs2/gamedata/{master.gamedata.jsonc,game.cs2.jsonc}` | CS2-owned target/function/layout declarations formerly under root `gamedata/cs2` | S3-PKG-01 |
| `games/cs2/js/adapters/{acquire,hud-click,damage}.js`, `games/cs2/js/adapters/test-host.js` | CS2 public compatibility wrappers, semantic mapping, and one recording test host over S2 frames | S3-ADAPT-05/06 |
| `scripts/build-game-packages.mjs` | Deterministically concatenate inputs, canonicalize the gamedata bundle, hash artifacts, and emit the deployed manifest | S3-PKG-01 |
| `scripts/test-game-packages.mjs` | Packager determinism, exact schema/hash/path tests | S3-PKG-01 |
| `core/src/game_packages/{mod,manifest,tests}.rs` | Parse/select/confine/hash packages, map owner overrides, hold status/provenance and the process registration | S3-LOAD-02/S3-CORE-04 |
| `core/src/engine_functions/package_loader.rs` | S2-owned normalization of verified game-package bundles plus compatible owner overrides | S3-GD-03 |
| `core/src/ffi.rs`, `shim/include/s2script_core.h` | Narrow selection/status C ABI; shim passes detected tokens and addon root only | S3-CORE-04 |
| `core/src/v8host.rs`, `core/src/v8host/lifecycle.rs` | Iterate registered packages without a package-name literal and ledger per-context instances | S3-CORE-04 |
| `core/src/engine_functions/{package_adapter,policy,projection}.rs` | S2-owned package callback runner, exact adapter registry, and generic codec extension points | S3-ADAPT-05/06 |
| `games/fixture-source2/*`, `core/src/game_packages/tests.rs` | Test-only second package and ordinary S2 function harness | S3-PORT-07 |
| `scripts/check-game-package-boundary.sh` | Literal/inventory/manifest/source-layout regression gate | S3-GATE-08 |
| `docs/ARCHITECTURE.md`, `docs/BUILDING.md`, `docs/INSTALL.md` | Boundary, capability matrix, packaging and compatible owner override operations | S3-GATE-08 |

## Work-package DAG and exclusive ownership

```text
S3-PRE-00
  -> S3-PKG-01 -> S3-LOAD-02 -> S3-GD-03 -> S3-CORE-04
                                             |          |
                                             +----------+-> S3-ADAPT-05 -> S3-DMGAMMO-06
                                                        +-> S3-PORT-07
S3-ADAPT-05 + S3-DMGAMMO-06 + S3-PORT-07 -> S3-GATE-08 -> S3-LIVE-09
```

One coordinator owns integration and shared files. Serialize `shim/src/s2script_mm.cpp`, `shim/CMakeLists.txt`, `core/src/lib.rs`, `core/src/v8host/natives.rs`, `scripts/package-addon.sh`, and CI script edits. The listed writer owns each new focused file until its task is merged. One designated operator alone installs artifacts, restarts the server, drives RCON, and records live evidence.

---

### Task 1: S3-PRE-00 Pin prerequisites and baseline behavior

**Files:**
- Create: `docs/superpowers/plans/game-package-boundary/baseline.md`
- Test: existing PR A/S1/S2 acceptance artifacts and commands

**Interfaces:**
- Consumes: accepted PR A, S1, and S2 commits plus the prerequisite API above
- Produces: exact three commit ids, stock Metamod identity, S2 ABI atom bounds, and baseline CS2 callback observations used by every later task

- [ ] **Step 1: Record the dependency commits and prove the tree is clean**

```bash
git status --short --branch
git rev-parse HEAD
git log -1 --format='%H %s'
```

Write the exact PR A/S1/S2 commits and evidence paths into `baseline.md`; do not accept branch names or “latest” as dependencies.

- [ ] **Step 2: Run the inherited engine-free retirement and API baselines**

```bash
bash scripts/test-khook-shutdown.sh
cd packages/sdk && npm test
```

Expected: the engine-free retirement fixture and SDK tests pass. Keep missing real-client/peer observations pending. Preserve existing known shutdown-only SIGSEGV/139 as diagnostic/non-blocking; this step does not run or require a server quit test.

- [ ] **Step 3: Inventory hardcoded selection and bespoke capability paths**

```bash
rg -n '@s2script/cs2|pawn\.js|Cs2JsPath|s2script_core_register_package' core shim scripts
rg -n 'dispatch_damage|damage_read_|damage_write_|is_acquire|legacy\.hud-click|legacy\.acquire' core shim
```

Paste the result into `baseline.md`. It must name the current hardcoded sites and every capability-specific damage/acquisition/HUD branch that later tasks remove.

- [ ] **Step 4: Commit the prerequisite receipt**

```bash
git add docs/superpowers/plans/game-package-boundary/baseline.md
git commit -m "docs: pin game package boundary prerequisites"
```

### Task 2: S3-PKG-01 Emit deterministic package artifacts and manifest

**Files:**
- Create: `games/cs2/game-package.jsonc`
- Move: `gamedata/cs2/master.gamedata.jsonc` -> `games/cs2/gamedata/master.gamedata.jsonc`
- Move: `gamedata/cs2/game.cs2.jsonc` -> `games/cs2/gamedata/game.cs2.jsonc`
- Create: `scripts/build-game-packages.mjs`
- Create: `scripts/test-game-packages.mjs`
- Modify: `scripts/package-addon.sh`
- Modify: `games/cs2/js/eslint.config.mjs`
- Modify: `scripts/check-gamedata-owners.sh`, `scripts/check-gamedata-sigs.sh`
- Modify: `packages/sdk/src/hookgen/gen.ts`, `packages/sdk/src/hookgen/emit-dts.ts`
- Modify: `packages/sdk/test/cs2-engine-calls.test.mjs`, `games/cs2/js/pawn.js` (source-path references only)

**Interfaces:**
- Consumes: current ordered JS inputs in `scripts/package-addon.sh` and current CS2 owner tree
- Produces: `dist/addons/s2script/game-packages.json`, `game-packages/cs2/index.js`, and `game-packages/cs2/gamedata.json`

- [ ] **Step 1: Write failing tests for exact manifest and deterministic bytes**

```js
test("emits the frozen deployed schema and lowercase SHA-256", async () => {
  const out = await buildFixture("csgo");
  assert.deepEqual(Object.keys(out.manifest), ["schemaVersion", "packages"]);
  assert.deepEqual(Object.keys(out.manifest.packages[0]),
    ["id", "match", "gamedataOwner", "bootstrap", "gamedata"]);
  assert.match(out.manifest.packages[0].bootstrap.sha256, /^[0-9a-f]{64}$/);
  assert.equal(out.manifest.packages[0].bootstrap.sha256,
    sha256(out.files.get("game-packages/cs2/index.js")));
});

test("same inputs produce byte-identical manifest and artifacts", async () => {
  assert.deepEqual(await buildFixture("csgo"), await buildFixture("csgo"));
});
```

- [ ] **Step 2: Run the tests and verify the packager is absent**

Run: `node --test scripts/test-game-packages.mjs`

Expected: FAIL because `build-game-packages.mjs` does not exist.

- [ ] **Step 3: Add the source manifest and canonical bundle format**

```jsonc
{
  "schemaVersion": 1,
  "id": "@s2script/cs2",
  "match": { "engine": "source2", "game": "csgo" },
  "gamedataOwner": "cs2",
  "bootstrapInputs": [
    "js/schema.generated.js", "js/nav.generated.js", "js/activity.js",
    "js/csitem.generated.js", "js/weapon.js", "js/pawn.js", "js/camera.js",
    "js/ui.js", "js/components.js", "js/hudinput.js", "js/menuhud.js", "js/voterail.js"
  ],
  "gamedataRoot": "gamedata"
}
```

The emitted `gamedata.json` is canonical JSON with `{schemaVersion:1,owner:"cs2",files:[{path,document}]}`. Preserve master order and all engine/game/platform variants; parse JSONC comments away, sort object keys, retain array order, and end every emitted file with one newline. Production packaging selects the intended first-party package inputs explicitly; synthetic packages require a separate test opt-in and never enter the default release manifest.

Move hook generation, static gates and the listed source-path assertions to `games/cs2/gamedata/` in the same commit as the source files. Regenerate and check hook declarations here so no integrated task points at removed inputs. Keep core/sdkhooks owner trees in place and the deployed `gamedata/cs2/custom/` operator path unchanged. S3-GD-03 consumes this completed relocation; it does not own a second move.

- [ ] **Step 4: Replace the shell concatenation with the packager**

```bash
node scripts/build-game-packages.mjs --out "$DIST/s2script"
```

Delete the `js/pawn.js` output and its conditional branch. Update ESLint to read `bootstrapInputs` from the source manifest instead of scraping shell text.

- [ ] **Step 5: Verify hashes, determinism, and owner relocation**

```bash
node --test scripts/test-game-packages.mjs
bash scripts/check-gamedata-owners.sh
bash scripts/check-gamedata-sigs.sh
bash scripts/check-hooks-generated.sh
find dist/addons/s2script/game-packages -type f -print | sort
test ! -e dist/addons/s2script/js/pawn.js
test ! -d gamedata/cs2
```

Expected: tests pass; exactly `cs2/index.js` and `cs2/gamedata.json` are emitted below the package root.

- [ ] **Step 6: Commit**

```bash
git add games/cs2 scripts/build-game-packages.mjs scripts/test-game-packages.mjs scripts/package-addon.sh
git add games/cs2/js/eslint.config.mjs gamedata/cs2
git add scripts/check-gamedata-owners.sh scripts/check-gamedata-sigs.sh packages/sdk/src/hookgen packages/sdk/test/cs2-engine-calls.test.mjs
git commit -m "build: emit verified game package artifacts"
```

### Task 3: S3-LOAD-02 Select, confine, and verify the deployed manifest

**Files:**
- Create: `core/src/game_packages/mod.rs`
- Create: `core/src/game_packages/manifest.rs`
- Create: `core/src/game_packages/tests.rs`
- Modify: `core/src/lib.rs`

**Interfaces:**
- Consumes: addon root, detected `{engine:"source2", game:DetectModDir()}`, platform, deployed manifest bytes
- Produces: `PreparedSelection { id, gamedata_owner, bootstrap_bytes, gamedata_bytes, bootstrap_sha256, gamedata_sha256, provenance }` or named `Missing/Ambiguous/Invalid/HashMismatch` status

- [ ] **Step 1: Write filesystem-backed Rust tests with concrete helpers**

```rust
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
struct TestDir(PathBuf);
impl Drop for TestDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn fixture() -> TestDir {
    let path = std::env::temp_dir().join(format!("s2-game-package-{}-{}",
        std::process::id(), NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&path).unwrap();
    TestDir(path)
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn rejects_traversal_before_read() {
    let root = fixture();
    write(&root.0.join("game-packages.json"), traversal_manifest().as_bytes());
    let err = prepare_selection(&root.0, "source2", "fixture", "linuxsteamrt64").unwrap_err();
    assert_eq!(err.code(), "invalid-path");
}

#[test]
fn names_every_ambiguous_candidate_in_sorted_order() {
    let root = complete_fixture(ambiguous_manifest());
    let err = prepare_selection(root.path(), "source2", "csgo", "linuxsteamrt64").unwrap_err();
    assert_eq!(err.candidates(), &["@fixture/a", "@fixture/b"]);
}
```

Define `traversal_manifest`, `ambiguous_manifest`, and `complete_fixture` in the same test module. `complete_fixture` writes both artifacts, computes their expected digests with `sha2::Sha256`, and writes canonical manifest JSON. Also cover missing manifest, schema version, exact 64-lowercase hashes, zero matches, duplicate ids, duplicate owner mappings, absolute paths, `..`, symlink escape, directory input, non-UTF-8 bootstrap, tampered artifact, and exact manifest field sets.

- [ ] **Step 2: Verify red**

Run: `cargo test -p s2script-core game_packages::tests -- --nocapture`

Expected: compile failure because `game_packages` and `prepare_selection` do not exist.

- [ ] **Step 3: Implement selection with existing Rust dependencies**

```rust
pub(crate) fn prepare_selection(addon_root: &Path, engine: &str, game: &str, platform: &str)
    -> Result<PreparedSelection, PackageError>;
```

Use `serde(deny_unknown_fields)` for the frozen deployed structures and `sha2::Sha256` already present in `core/Cargo.toml`. Reject absolute/parent components before I/O; canonicalize existing root and artifact paths; require each artifact to be a regular-file descendant of canonical `addonRoot/game-packages`; validate expected hash grammar before hashing; compare the 32-byte decoded expected digest with the computed digest. Sort ambiguous ids before formatting status. The platform argument travels in provenance/capability checks; matching remains exactly engine+game per the approved schema.

- [ ] **Step 4: Verify green**

```bash
cargo test -p s2script-core game_packages::tests -- --nocapture
cargo test -p s2script-core --no-run
```

- [ ] **Step 5: Commit**

```bash
git add core/src/game_packages core/src/lib.rs
git commit -m "feat: verify and select game package manifests"
```

### Task 4: S3-GD-03 Merge verified package gamedata with compatible owner overrides

**Files:**
- Create: `core/src/engine_functions/package_loader.rs`
- Create: `core/src/engine_functions/tests/package_loader.rs`
- Modify: `core/src/engine_functions/mod.rs`
- Modify: `core/src/game_packages/mod.rs`
- Read/check: relocated hookgen/static gates and source-path assertions already integrated in S3-PKG-01

**Interfaces:**
- Consumes: verified bundle bytes, `gamedata_owner`, engine/game/platform, `addon_root/gamedata/<owner>/custom`, and S2's normalizer/`OverrideSet`
- Produces: `(NormalizedBundle, OverrideSet, PackageGamedataProvenance)` with verified files selected in master order and sorted compatible overrides last

- [ ] **Step 1: Add failing bundle/override/orthogonality tests with real temporary files**

```rust
#[test]
fn applies_compatible_owner_overrides_after_verified_files() {
    let fx = PackageFixture::new("cs2", "source2", "csgo", "linuxsteamrt64");
    fx.write_override("90-fix.jsonc", repair_for("canAcquire", "operator-pattern"));
    let (_, overrides, provenance) = load_package_bundle(
        fx.bundle(), fx.addon_root(), "cs2", fx.target()).unwrap();
    assert_eq!(overrides.target_for("canAcquire").unwrap().pattern, "operator-pattern");
    assert_eq!(provenance.applied_paths.last().unwrap(), "gamedata/cs2/custom/90-fix.jsonc");
}

#[test]
fn owner_and_target_are_independent() {
    let core = normalize_fixture("core", target("source2", "csgo"), "coreFact");
    let cs2 = normalize_fixture("cs2", target("source2", "csgo"), "packageFact");
    assert_eq!(core.owner(), "core");
    assert_eq!(cs2.owner(), "cs2");
    assert_eq!(core.target(), cs2.target());
}
```

`PackageFixture`, `repair_for`, `target`, and `normalize_fixture` are test helpers defined in this module using the same standard-library `TestDir` pattern, `serde_json`, and S2's normalization constructors. Also assert malformed bundle path, duplicate embedded path, owner mismatch, selected-file parse failure, absent `custom/`, stale base hash, conflicting sorted overrides, and attempts to change ABI/projection/policy/requirement.

- [ ] **Step 2: Verify red**

Run: `cargo test -p s2script-core engine_functions::tests::package_loader -- --nocapture`

Expected: compile failure because `load_package_bundle` is undefined.

- [ ] **Step 3: Feed the package artifact through S2's single normalizer**

```rust
pub(crate) fn load_package_bundle(
    bundle_bytes: &[u8], addon_root: &Path, gamedata_owner: &str, target: TargetKey,
) -> Result<(NormalizedBundle, OverrideSet, PackageGamedataProvenance), FunctionError>;
```

Do not materialize bundle files or add a second declaration/override grammar. Select embedded documents in master order, then feed them and sorted external owner overrides through S2's existing parser, normalizer, contract-hash checks, and `OverrideSet` validation. Record both owner and target on every function receipt.

- [ ] **Step 4: Verify the integrated source-path relocation**

Verify S3-PKG-01 already moved hook generation and static gates to `games/cs2/gamedata/game.cs2.jsonc`. Keep core-owned and sdkhooks-owned source trees in `gamedata/`; do not infer ownership from the `game.cs2` target filename. Any additional source-path consumer found here is an integration fix, not permission to use a removed path until this task.

- [ ] **Step 5: Verify**

```bash
cargo test -p s2script-core engine_functions::tests::package_loader -- --nocapture
bash scripts/check-gamedata-owners.sh
bash scripts/check-gamedata-sigs.sh
bash scripts/check-hooks-generated.sh
rg -n 'gamedata/cs2/game\.cs2\.jsonc' games packages scripts
```

Expected: the final search returns no source-file references; `addons/s2script/gamedata/cs2/custom/` remains documented as the operator compatibility path.

- [ ] **Step 6: Commit**

```bash
git add core/src/engine_functions core/src/game_packages
git commit -m "feat: merge packaged gamedata with owner overrides"
```

### Task 5: S3-CORE-04 Register and bootstrap the selected package generically

**Files:**
- Modify: `core/src/game_packages/mod.rs`
- Modify: `core/src/lib.rs`
- Modify: `core/src/ffi.rs`
- Modify: `core/src/v8host.rs`
- Modify: `core/src/v8host/lifecycle.rs`
- Modify: `core/src/v8host/tests.rs`
- Modify: `shim/include/s2script_core.h`
- Modify: `shim/src/s2script_mm.cpp`

**Interfaces:**
- Consumes: `prepare_selection`, `load_package_bundle`, S2 `prepare_owner`/`activate_owner`, and shim-supplied addon/target strings
- Produces: one reserved process registration and per-context package generations; `require(selected_id)` resolves only when that context bootstrap succeeded

- [ ] **Step 1: Write failing atomicity, iteration, and teardown tests**

```rust
#[test]
fn context_bootstraps_the_registered_id_without_a_cs2_literal() {
    register_fixture("@fixture/two", "fixture-owner", "globalThis.__fixture={ok:true}");
    create_plugin_context("consumer");
    assert!(eval_in_context_bool("consumer", "require('@fixture/two').ok"));
}

#[test]
fn failed_registration_leaves_no_source_or_function_owner() {
    assert!(register_with_invalid_functions("@fixture/bad").is_err());
    assert_eq!(game_packages::selected_status().code, "registration-failed");
  assert!(engine_functions::status(&bad_owner, "probe").is_unavailable());
}
```

Cover duplicate manifest owner/id, exact semantic contract hash, separate implementation manifest hash, bootstrap throw cleanup, stale closure, reload of one consumer, unload inside callback deferred until frame unwind, map-scoped handle invalidation, and terminal shutdown after all contexts retire. Per-context adapter duplicates are tested under S3-ADAPT-05: identical id/hash across distinct `PackageInstanceKey`s is valid.

- [ ] **Step 2: Verify red**

Run: `cargo test -p s2script-core game_packages -- --nocapture`

Expected: FAIL because the registry does not exist.

- [ ] **Step 3: Add a narrow copying C ABI**

```c
int s2script_core_select_game_package(const char* addon_root, const char* engine,
                                      const char* game, const char* platform);
const char* s2script_core_game_package_status_json(void);
void s2script_core_clear_game_package(void);
```

Core copies strings before returning and performs all file access after validating UTF-8. It prepares the verified selection and normalized bundle, constructs `OwnerKey { id: reserved_owner_id, generation, kind: OwnerKind::GamePackage }`, calls `prepare_owner`, validates the reserved adapter contracts as metadata, calls `activate_owner`, and only then atomically publishes bootstrap source plus the active process receipt. Executable JS adapters register during each context's token-scoped bootstrap below; process preparation does not invent callbacks without a context. Any failure calls `drop_owner(&owner)` and publishes no id. The status JSON contains code, id, owner, manifest path, both hashes, function status summary, and error; shim logs this JSON once after selection so live acceptance has a query-free status receipt. `clear` is terminal-only: it first requires all contexts/in-flight frames retired, drops S2 owner receipts, then clears source/data/provenance.

- [ ] **Step 4: Iterate registry records during context creation**

Replace the literal lookup in `create_plugin_context` with:

```rust
for package in crate::game_packages::context_bootstraps() {
    let instance = prepare_context_instance(&package, plugin_owner)?;
    // Owns the provisional ledger entry and token; failure starts ordered retirement.
    instance.with_bootstrap_token(scope, |scope| {
        run_prelude(scope, &package.id, &package.bootstrap_js)
    })?;
    instance.activate_and_publish_require(scope, &package.id)?;
}
```

This is lifecycle pseudocode; implement its helpers using S2's actual integrated owner/instance API. Before evaluation, mint and provisionally ledger `PackageInstanceKey { parent: plugin OwnerKey, package_owner: reserved package OwnerKey }`. Its hidden bootstrap token is current only while package code evaluates, enabling registration inside the prelude. On throw or activation/publication failure, retire the provisional instance in this order: block new dispatch, detach its adapter callbacks/subscriptions, let active synchronous frames unwind (defer finalization through the ledger if one is still active), invalidate its epochs/views/wrappers/handles, release its receipts, then dispose V8 globals. Failure never publishes `require(id)` or affects peer instances, and an ordinary destructor must not free callbacks while a frame still uses them. Publish only after successful instance activation. The final ledger entry uses that same ordered retirement for plugin unload, without touching peer instances.

- [ ] **Step 5: Replace shim registration and crash schema hashing**

Delete `Cs2JsPath`, the `@s2script/cs2` literals, old source/gamedata registration calls, the static `s_gdGame` package load, and the CS2-specific crash hash read. Pass `AddonRoot()`, `"source2"`, `DetectModDir()`, and `"linuxsteamrt64"` to the new selector. Use selected artifact hashes/provenance for crash identity. Call terminal `clear` only after context shutdown succeeds.

- [ ] **Step 6: Verify targeted and boundary tests**

```bash
cargo test -p s2script-core game_packages -- --nocapture
cargo test -p s2script-core v8host -- --nocapture
bash scripts/check-core-boundary.sh
```

- [ ] **Step 7: Commit**

```bash
git add core/src/game_packages core/src/{lib.rs,ffi.rs,v8host.rs} core/src/v8host/{lifecycle.rs,tests.rs}
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp
git commit -m "feat: bootstrap selected game packages generically"
```

### Task 6: S3-ADAPT-05 Relocate acquisition and HUD compatibility policies

**Files:**
- Create: `games/cs2/js/adapters/acquire.js`
- Create: `games/cs2/js/adapters/acquire.test.js`
- Create: `games/cs2/js/adapters/hud-click.js`
- Create: `games/cs2/js/adapters/hud-click.test.js`
- Create: `games/cs2/js/adapters/test-host.js`
- Create: `games/cs2/adapters/contracts/legacy.acquire.v1.json`
- Create: `games/cs2/adapters/contracts/legacy.hud-click.v1.json`
- Modify: `games/cs2/game-package.jsonc`
- Modify: `games/cs2/gamedata/game.cs2.jsonc`
- Modify: `games/cs2/js/pawn.js`
- Modify: `games/cs2/js/hudinput.js`
- Modify: S2's accepted `core/src/engine_functions/package_adapter.rs`
- Modify: S2's accepted `core/src/engine_functions/policy.rs` only to replace its temporary CS2 implementations

**Interfaces:**
- Consumes: S2 generic binding/view/subscription APIs and package-only policy registration
- Produces: `legacy.acquire.v1` and `legacy.hud-click.v1` contracts owned by `game-package:@s2script/cs2`

- [ ] **Step 1: Freeze observable compatibility in failing adapter tests**

```js
import test from "node:test";
import assert from "node:assert/strict";
import { mountAcquire, mountHud } from "./test-host.js";

test("acquire preserves action precedence and first-deny order", () => {
  const votes = mountAcquire().run([
    { action: "Changed", result: 6 },
    { action: "Handled", result: 2 },
    { action: "Handled", result: 3 }
  ], 0);
  assert.equal(votes.effectiveResult, 2);
  assert.equal(votes.skippedOriginal, true);
  assert.equal(votes.view.player.slot, 3);
});

test("HUD compatibility post callback still runs before engine original", () => {
  const order = [];
  mountHud().run({ callback: () => order.push("callback"), original: () => order.push("original") });
  assert.deepEqual(order, ["callback", "original"]);
});
```

`test-host.js` exports both mounts and constructs a recording `ProjectedFrame`, original-call closure, owner key, and receipt disposer without engine pointers. Add implicit-deny, Changed-deny versus engine-Allow, Changed-Allow versus engine-deny, first deny among equal-strength actions, result mutation, nested `giveNamedItem`, stale receiver, post result, copied HUD strings, unresolved clicker `slot=-1`, re-entry, suppression, and unload-during-callback cases.

- [ ] **Step 2: Verify red against pure generic S2 policy**

Run: `node --test games/cs2/js/adapters/*.test.js`

Expected: acquisition fold/order tests fail until package policy contracts are registered.

- [ ] **Step 3: Declare explicit policy contracts and ordinary functions**

```jsonc
"adapters": {
  "legacy.acquire.v1": {
    "version": 1, "contract": "adapters/contracts/legacy.acquire.v1.json", "source": "js/adapters/acquire.js"
  },
  "legacy.hud-click.v1": {
    "version": 1, "contract": "adapters/contracts/legacy.hud-click.v1.json", "source": "js/adapters/hud-click.js"
  }
}
```

Copy each S2 canonical contract document byte-for-byte to its matching separate file under `adapters/contracts/`; their id/version/public frame-delivery-decision/timing digests must equal the locked S2 baseline before deleting Rust implementations. Do not wrap the two documents into a new aggregate and call its hash the old contract hash. The packager verifies each document independently and puts the stable id-to-hash mapping in `globalThis.__s2_adapter_contracts`. JavaScript source hashes contribute only to `implementation_manifest_hash` and the manifest's bootstrap integrity hash; changing implementation language or source without changing behavior does not change `contract_hash`. Each adapter calls `__s2_function_adapter_register("legacy.acquire.v1", globalThis.__s2_adapter_contracts["legacy.acquire.v1"], {pre,post})` (or the HUD id); package wrappers call `__s2_function_adapter_subscribe(bindingId, adapterId, phase, wrapper)`. Acquisition's `pre(&mut AdapterDispatch)` walks `SubscriberCursor.invoke_next()` and implements the current fold exactly: Handled/Stop votes precede Changed, registration order stays stable within strength, any deny beats Allowed, first deny wins between denies, unwritten Handled/Stop implies `1`, and engine result participates only if original ran. HUD owns its compatibility callback-before-original decision.

- [ ] **Step 4: Move wrapper semantics into focused adapters**

`acquire.js` performs the item-services-to-pawn/controller hop and constructs the existing `CanAcquireView`. `hud-click.js` selects receiver/arguments, copies strings before callback, and exports the existing `CustomHudClickedView`. `pawn.js` and `hudinput.js` delegate without changing public names.

- [ ] **Step 5: Delete temporary executable implementations and shape branches**

Remove S2's temporary built-in executable implementations for these ids and current `plan.shape == 3`/HUD-shape semantic branching only after the JS conformance fixture proves identical `SubscriberDelivery` sequences and final `PreDecision` values under the unchanged contract hashes. Preserve the reserved id/hash/visibility registration and trusted-owner enforcement required by S2's v1 compatibility window; replacing the implementation must not make these contracts community-selectable. Keep the generic synchronous callback runner, projected frame, action/order facts, id/hash verification, and package-owner authorization. No acquire code, receiver hop, copy rule, or HUD timing choice remains native.

- [ ] **Step 6: Verify compatibility and fan-out**

```bash
node --test games/cs2/js/adapters/*.test.js games/cs2/js/hudinput.test.js
cd packages/sdk && node --test test/cs2-ui.test.mjs test/cs2-engine-calls.test.mjs
cargo test -p s2script-core engine_functions -- --nocapture
```

The Rust suite must include the real-V8 cross-context case: owner A calls a live hooked target while A's package instance/subscription is bypassed or busy; owner B's instance runs synchronously and invokes B's wrapper. Add reload tests proving two contexts may register the same id/hash, duplicate `(instance,id)` fails, retiring A leaves B active, and a subscriber set with no eligible adapter instance fails by name without deferral.

- [ ] **Step 7: Commit**

```bash
git add games/cs2 core/src/engine_functions/package_adapter.rs core/src/engine_functions/policy.rs
git commit -m "refactor: move CS2 compatibility policies into the game package"
```

### Task 7: S3-DMGAMMO-06 Put damage and ammo on shared S1/S2 facilities

**Files:**
- Create: `games/cs2/js/adapters/damage.js`
- Create: `games/cs2/js/adapters/damage.test.js`
- Modify: `games/cs2/js/adapters/test-host.js`
- Modify: `games/cs2/gamedata/game.cs2.jsonc`
- Modify: `games/cs2/js/weapon.js`
- Modify: `packages/sdk/damage.d.ts`
- Modify: `packages/sdk/sdkhooks.d.ts`
- Modify: `core/src/sdkhooks.rs`
- Modify: `core/src/engine_functions/projection.rs`
- Modify: `core/src/v8host.rs`
- Modify: `core/src/v8host/natives.rs`
- Modify: `core/src/ffi.rs`
- Modify: `shim/include/s2script_core.h`
- Modify: `shim/src/engine_hooks.{h,cpp}`
- Modify: `shim/src/s2script_mm.cpp`

**Interfaces:**
- Consumes: ordinary S2 function `dispatchTraceAttack`, generic bounded codec `borrowed-record.v1`, ordinary schema fields/function declarations
- Produces: unchanged `DamageInfo`, `SDKHook(OnTakeDamage/OnTakeDamagePost)`, and `Weapon.setAmmo` behavior without bespoke damage installer/dispatch natives

- [ ] **Step 1: Write red tests for borrowed epochs and public parity**

```js
import test from "node:test";
import assert from "node:assert/strict";
import { mountDamage, fixtureWeapon } from "./test-host.js";

test("DamageInfo keeps CS2 names but expires after synchronous callback", () => {
  let saved;
  const damage = mountDamage({ damage: 50 });
  damage.fire((view) => { saved = view; view.damage /= 2; });
  assert.equal(damage.nativeDamage(), 25);
  assert.throws(() => saved.damage, /expired borrowed view/);
});

test("setAmmo uses schema/function facilities and stale weapons fail", () => {
  const weapon = fixtureWeapon();
  assert.equal(weapon.setAmmo(30), true);
  weapon.ref.invalidate();
  assert.equal(weapon.setAmmo(10), false);
});
```

Add post-readonly, block-to-zero, attacker/inflictor/weapon handle adoption, `await` escape, map invalidation, reload, nested damage, and subscription receipt fan-out tests.

- [ ] **Step 2: Verify red**

```bash
node --test games/cs2/js/adapters/damage.test.js
cargo test -p s2script-core damage -- --nocapture
```

Expected: tests expose current bespoke globals/dispatch and non-epoch view.

- [ ] **Step 3: Register the bounded generic codec**

The shim codec reads/writes only declared scalar slots from an opaque callback-frame pointer. S2 creates a dispatch epoch and exposes typed numeric slots, copied values, `EntityRef`, or registered handles. It rejects post-callback access and async retention. `borrowed-record.v1` is capability-neutral; all `CTakeDamageInfo` offsets, field names, mutability, and meaning stay in CS2 data/code.

```jsonc
"dispatchTraceAttack": {
  "projection": { "codec": "borrowed-record.v1", "layout": {
    "damage": { "offsetKey": "CTakeDamageInfo_m_flDamage", "type": "f32", "mutable": "pre" },
    "damageType": { "offsetKey": "CTakeDamageInfo_m_bitsDamageType", "type": "i32" }
  }},
  "surfaces": ["pre", "post"]
}
```

- [ ] **Step 4: Rebuild public wrappers over S2 views**

`damage.js` gives generic slots their existing `DamageInfo` meaning and maps victim/attacker handles through host liveness. Keep `.damage` writable only during pre. Route `SDKHook` compatibility through the S2 subscription receipt and retain exact collapse/order behavior.

- [ ] **Step 5: Keep ammo ordinary**

Use generated schema access for `clip1`. Preserve today's contract in which `reserve` is accepted but ignored because its layout is deferred. `Weapon.setAmmo` remains the public wrapper and performs numeric/stale guards. Add no `ammo_*` native and no S2 capability enum entry.

- [ ] **Step 6: Remove bespoke capability plumbing**

Delete `s2script_core_dispatch_damage*`, `__s2_damage_*`, damage-specific engine-op fields, dedicated install/dispatch branches, and damage-specific subscriber stores only after parity tests pass. The production inventory must contain the generic codec/function names, not old installers.

- [ ] **Step 7: Verify**

```bash
node --test games/cs2/js/adapters/damage.test.js
cargo test -p s2script-core damage -- --nocapture
cargo test -p s2script-core engine_functions -- --nocapture
cargo test -p s2script-core sdkhooks -- --nocapture
bash scripts/test-hook-dispatch.sh
bash scripts/check-hook-shapes.sh
```

- [ ] **Step 8: Commit**

```bash
git add games/cs2 packages/sdk core/src shim/src shim/include/s2script_core.h
git commit -m "refactor: route damage and ammo through shared functions"
```

### Task 8: S3-PORT-07 Prove selection and owner isolation with a synthetic second package

**Files:**
- Create: `games/fixture-source2/game-package.jsonc`
- Create: `games/fixture-source2/js/index.js`
- Create: `games/fixture-source2/gamedata/{master.gamedata.jsonc,game.fixture.jsonc}`
- Modify: `core/src/game_packages/tests.rs`
- Modify: `core/src/game_packages/mod.rs`
- Modify: `scripts/test-game-packages.mjs`

**Interfaces:**
- Consumes: manifest selector, atomic registry, ordinary S2 scalar call/hook
- Produces: proof that native binaries need no fixture-specific name, path, branch, or rebuild

- [ ] **Step 1: Add the failing fixture test**

First implement any generic `FixtureHost`/registry test seam in the listed native test files, without fixture-specific production names or branches. Freeze that generic native build before the package-only proof below. The fixture package is explicitly opted into test packaging; default production packaging still emits only the supported CS2 package, and a regression checks that exclusion.

```rust
#[test]
fn synthetic_package_selects_and_uses_an_ordinary_function() {
    let host = FixtureHost::with_game("fixture");
    assert_eq!(host.selected_id(), Some("@fixture/source2"));
    assert_eq!(host.eval("require('@fixture/source2').twice(21)"), 42);
    assert!(!host.has_owner("game-package:@s2script/cs2"));
}
```

- [ ] **Step 2: Verify red, then add only package files**

Run before and after: `cargo test -p s2script-core synthetic_package_selects -- --nocapture`

Expected before: generic harness is ready but no fixture package is selected. Expected after: PASS with only fixture package data/JS added and no further native build or changes in `core/` or `shim/` for fixture identity/semantics. Record the same native artifact identity before and after this package-only step.

- [ ] **Step 3: Assert the scope of the proof**

Add a test/readme assertion: `"fixture proves package boundary only; it is not a supported live game"`. The function uses an S2 test target, not a fake claim that an actual Source 2 server ABI was exercised.

- [ ] **Step 4: Commit**

```bash
git add games/fixture-source2 core/src/game_packages scripts/test-game-packages.mjs
git commit -m "test: prove game package selection without native literals"
```

### Task 9: S3-GATE-08 Add static gates, capability matrix, and full CI

**Files:**
- Create: `scripts/check-game-package-boundary.sh`
- Modify: `scripts/ci-native.sh`
- Modify: `scripts/ci-js.sh`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/BUILDING.md`
- Modify: `docs/INSTALL.md`
- Modify: `docs/PROGRESS.md`

**Interfaces:**
- Consumes: all prior tasks
- Produces: repeatable static/full-CI evidence and accurate support claims

- [ ] **Step 1: Write the boundary gate before cleanup**

```bash
#!/usr/bin/env bash
set -euo pipefail
production=(core/src shim/src shim/include)
test_exclusions=(--glob '!**/tests/**' --glob '!**/tests.rs' --glob '!**/*_test.cpp')
if rg -n "${test_exclusions[@]}" '@s2script/cs2|pawn\.js|Cs2JsPath' "${production[@]}"; then
  echo 'game-package literal leaked into core/shim' >&2; exit 1
fi
if rg -n "${test_exclusions[@]}" 's2script_core_dispatch_damage|__s2_damage_|InstallDamage' "${production[@]}"; then
  echo 'bespoke damage path remains' >&2; exit 1
fi
node --test scripts/test-game-packages.mjs
```

Run: `bash scripts/check-game-package-boundary.sh`

Expected initially: FAIL and name every remaining leak.

Keep identity-specific fixtures in explicit test files, not inline production modules. Reconcile the exclusions with actual compiled production inputs; never exempt an entire production module to silence a match. Test that a production-source literal is rejected and an explicitly test-only fixture literal is allowed, so the gate covers shipped paths without rejecting its own acceptance fixtures.

- [ ] **Step 2: Document support without widening claims**

Publish a table of actual Source 2 interface versions, `linuxsteamrt64`, bounded S2 ABI atoms/vector exclusions, registered projection codecs, and rejection messages. State that CS2 is the only live-supported package and the fixture is structural evidence only. Document `gamedataOwner` compatibility and override provenance. Preserve existing shutdown-only 139 observations as non-blocking diagnostics, separate from actual startup/plugin/map/reload results; add no new shutdown recorder or quit gate.

- [ ] **Step 3: Run targeted static gates**

```bash
bash scripts/check-game-package-boundary.sh
bash scripts/check-core-boundary.sh
bash scripts/check-gamedata-owners.sh
bash scripts/check-call-descriptors.sh
bash scripts/check-hook-shapes.sh
```

- [ ] **Step 4: Run full JavaScript and native CI**

```bash
bash scripts/ci-js.sh
bash scripts/ci-native.sh
```

Expected: both pass. A Docker-only absence is recorded as infrastructure-unavailable, never converted into S3 completion.

- [ ] **Step 5: Commit**

```bash
git add scripts/check-game-package-boundary.sh scripts/ci-native.sh scripts/ci-js.sh docs
git commit -m "docs: gate and publish the game package boundary"
```

### Task 10: S3-LIVE-09 Build and prove the integrated CS2 release

**Files:**
- Create: `docs/superpowers/plans/game-package-boundary/live-acceptance.md`
- Create: evidence files referenced from that receipt
- Modify: none in production unless a failing gate produces a reviewed fix task

**Interfaces:**
- Consumes: integrated PR A+S1+S2+S3 commit and stock host identity
- Produces: deployable sniper artifact and honest final acceptance receipt

- [ ] **Step 1: Build the deployable artifact**

```bash
sudo docker run --rm -v "$PWD:/repo" -w /repo \
  -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/build-base-plugins.sh
bash scripts/package-addon.sh
```

Record commit, core/shim/package hashes, manifest, selected package status, Metamod identity, and all default `.s2sp` hashes.

- [ ] **Step 2: Install once and run selection/bootstrap checks**

```bash
sudo docker compose -f docker/docker-compose.yml restart cs2
python3 scripts/rcon.py "meta list"
sudo docker logs s2script-cs2 2>&1 | rg 'game-package-status.*@s2script/cs2'
```

Expected: stock host lists s2script; status names `@s2script/cs2`, owner `cs2`, manifest path, matching hashes, and active S2 functions/policies. No `pawn.js` fallback is logged.

- [ ] **Step 3: Run map, compatibility, reload, and teardown cases**

Exercise acquisition allow/deny/most-restrictive fold, HUD click callback-before-original order, damage pre mutation/block and post observation, ammo write, ordinary community function sharing a physical binding, nested calls, stale borrowed view, stale closure, map change, repeated `.s2sp` reload, unload requested inside callback, and another plugin remaining subscribed. Record actual callback order and statuses.

- [ ] **Step 4: Verify the running server after the runtime cases**

Record active default plugins, map and plugin error/crash observations after the runtime cases, and leave the server running. Callbacks into disposed contexts, receipt leaks, use-after-free, or crashes during plugin use/reload remain failures. Known whole-process shutdown-only 139 is diagnostic/non-blocking; do not add a quit test or manufacture a successful terminal record. A necessary deployment stop may incidentally supply child status without becoming a separate acceptance gate.

- [ ] **Step 5: Mark acceptance only from complete evidence**

In `live-acceptance.md`, list every spec acceptance criterion with its command/evidence path. Mark unavailable human/client or platform evidence pending. Do not write “S3 complete” while any required row is pending.

- [ ] **Step 6: Commit evidence**

```bash
git add docs/superpowers/plans/game-package-boundary
git commit -m "test: record game package live acceptance"
```

## Final self-review checklist

- [ ] Every S3 acceptance criterion maps to S3-LOAD-02 through S3-LIVE-09.
- [ ] The placeholder scan required by the writing-plans skill returns no matches.
- [ ] The C ABI, Rust structs, manifest fields, policy ids, codec id, owner id, and teardown order use the same spelling in every task.
- [ ] The plan preserves the `cs2/custom` operator path while relocating shipped CS2 data.
- [ ] The plan distinguishes target from owner with both a core-owned CS2 target and a CS2-owned target test.
- [ ] The plan removes package literals and bespoke damage/adapter paths only after behavior tests exist.
- [ ] The fixture claim remains limited to the boundary and ordinary S2 function path.
- [ ] Full CI, deployable sniper, live plugin/map/reload, callback lifetime and required client evidence remain acceptance gates; known whole-process shutdown-only 139 is diagnostic/non-blocking.
