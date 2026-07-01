# Task 8 Report: @s2script/cli + @s2script/std + @s2script/cs2

## Status: COMPLETE

## What was built

### packages/cli/

**`src/build.ts`** — `buildPlugin(dir: string): Promise<string>`:
1. Reads `dir/package.json`, extracts `name`, `version`, `s2script.apiVersion`, `s2script.pluginDependencies`, `s2script.publishes`, and the entry point (`s2script.main ?? main`).
2. Runs `esbuild.build({ bundle:true, platform:"neutral", format:"cjs", external:["@s2script/std","@s2script/cs2"], target:"es2020", write:false })` → `outputFiles[0].text` (CJS string).
3. Derives `manifest.json` object with keys matching `loader::Manifest`.
4. Creates a zip with `adm-zip` containing `manifest.json` + `plugin.js`.
5. Writes to `dir/dist/<sanitized-id>.s2sp`, returns the path.

**`src/cli.ts`** — parses `s2script build <dir>`, calls `buildPlugin`, prints the path.

**`build.mjs`** — esbuild driver: bundles `src/cli.ts` → `dist/cli.js` (ESM, node24, `external: ['esbuild','adm-zip']`, shebang, chmod 755).

### Manifest keys matched to loader::Manifest

From `core/src/loader.rs` (lines 23–28):
```rust
pub struct Manifest {
    pub id: String,
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
}
```

The emitted `manifest.json` uses:
- `"id"` — matches `pub id: String`
- `"version"` — matches `pub version: String`
- `"apiVersion"` — matches `#[serde(rename = "apiVersion")]` on `api_version`
- `"pluginDependencies"` and `"publishes"` — ignored by serde (forward-compatible extra fields)

### packages/std/index.d.ts

Type stubs only (no runtime code):
- `OnGameFrame.subscribe(fn, opts?)`
- `delay(ms)`, `nextTick()`, `nextFrame()`, `threadSleep(ms)`
- `console`

### packages/cs2/index.d.ts

Type stubs only (no runtime code):
- `interface Pawn { health: number }`
- `const Pawn: { forSlot(slot): Pawn | null }`

(CS2-specific identifiers live here, not in core or std, per convention.)

## Test

**`test/build.test.mjs`** (node built-in test runner):
- Builds fixture `test/fixtures/hello` (`@demo/hello`, `apiVersion:"1.x"`)
- Unzips the `.s2sp` with `adm-zip`
- Asserts `manifest.id === "@demo/hello"`, `manifest.apiVersion === "1.x"`, `manifest.version === "0.1.0"`
- Asserts `plugin.js` contains `require("@s2script/std")` (CJS external require)

Run via: `node --experimental-strip-types --no-warnings test/build.test.mjs`
(Node 24.11.1 native TS stripping; no tsx or tsc required)

### Exact test output

```
> @s2script/cli@0.1.0 test
> node --experimental-strip-types --no-warnings test/build.test.mjs

✔ build produces a .s2sp with derived manifest + cjs plugin.js (18.221314ms)
ℹ tests 1
ℹ suites 0
ℹ pass 1
ℹ fail 0
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 21.848065
```

### CLI smoke test

```
$ node packages/cli/dist/cli.js build packages/cli/test/fixtures/hello
.../test/fixtures/hello/dist/_demo_hello.s2sp

# manifest.json inside:
{ "id": "@demo/hello", "version": "0.1.0", "apiVersion": "1.x", "pluginDependencies": {} }

# plugin.js contains:
var import_std = require("@s2script/std");
```

### Boundary checks

```
bash scripts/check-core-boundary.sh     → core boundary OK: s2script-core depends on no games/* crate
bash scripts/test-boundary-nameleak.sh  → PASS (CS2 name-leak gate + include_str!/games/ gate)
```

## Concerns

None. The `.s2sp` output matches `read_s2sp` requirements exactly:
- zip entries named `manifest.json` and `plugin.js` (exact filenames `by_name` looks for)
- `apiVersion` key matches `#[serde(rename = "apiVersion")]`
- `plugin.js` is CJS with `var import_std = require("@s2script/std")` (external preserved)

---

## Prior content of this file (task-8 slot was previously used for README work)

All commands run from `/home/gkh/projects/s2script` on branch `slice-0-boot-handshake`.

### `make core && make shim && make package`

```
cargo build --release
    Finished `release` profile [optimized] target(s) in 0.09s
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release && cmake --build build/shim -j
[100%] Built target s2script
./scripts/package-addon.sh
packaged: dist/addons
dist/addons/metamod/s2script.vdf
dist/addons/s2script/bin/linuxsteamrt64/libs2script_core.so
dist/addons/s2script/bin/linuxsteamrt64/s2script.so
dist/addons/s2script/gamedata/core.gamedata.jsonc
```

### `make check-boundary`

```
./scripts/check-core-boundary.sh
core boundary OK: s2script-core depends on no games/* crate
```

### `find dist -type f`

```
/home/gkh/projects/s2script/dist/addons/metamod/s2script.vdf
/home/gkh/projects/s2script/dist/addons/s2script/gamedata/core.gamedata.jsonc
/home/gkh/projects/s2script/dist/addons/s2script/bin/linuxsteamrt64/s2script.so
/home/gkh/projects/s2script/dist/addons/s2script/bin/linuxsteamrt64/libs2script_core.so
```

### `nm -D s2script.so | grep CreateInterface`

```
0000000000005840 T CreateInterface
```

Both `.so`s and gamedata are present. `CreateInterface` is exported. Boundary check passes.

## README structure (consolidated)

The existing three sections (Building the Rust core / Vendored SDKs / Docker verification
runbook) were merged into a single top-to-bottom document with these sections:

1. Project intro + Slice 0 scope note (links to ARCHITECTURE.md and spec)
2. Prerequisites (clang, cmake, cargo, docker; Linux x86-64; `--test-threads=1` note)
3. Reproduce from scratch (ordered commands: clone → submodule → `make core` → `make shim` → `make package`; v8 pin note)
4. Vendored SDKs + patch workflow (existing content, kept verbatim with minor additions)
5. Docker verification runbook — UPDATED: removed "log-only" language; added per-interface
   acquisition lines, SchemaSystem deferred NOTE, and `hello from V8 in CS2` to expected
   output; added degradation sub-test; updated unload/reload expected output
6. Acceptance checklist (6-row table, all criteria from spec §12, "operator confirms" column unchecked)
7. Known findings / constraints (v8 TLS pin; §5 resident-cdylib + reload posture; SchemaSystem
   deferred; version strings as data; gamedata cwd assumption)

## Live gate status

Criterion 5 (`meta load` reprints hello without restart) is operator-deferred per the task brief.
No criterion is pre-marked as passed. All six rows in the acceptance checklist have unchecked
`[ ]` operator-confirms cells.
