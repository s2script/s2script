# Slice 0 — Boot Handshake — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Host a V8 isolate inside a live CS2 dedicated server via Metamod:Source, run `console.log` to the server console, and tear it down cleanly without restarting the server.

**Architecture:** A thin C++ `ISmmPlugin` shim (engine-generic) owns the Metamod lifecycle, reads interface version strings from gamedata, acquires Source 2 interfaces, and calls a Rust core over a 3-function C ABI. The Rust core (a resident `cdylib`) embeds V8 via the `v8`/rusty_v8 crate, initializes the V8 platform exactly once for the process, and disposes only the isolate+context on shutdown so a `meta unload`/`meta load` cycle is safe. An empty `games/cs2` crate establishes the engine-generic/per-game boundary, mechanically enforced by a CI check.

**Tech Stack:** C++ (Metamod:Source `dev`/2.0 + hl2sdk `cs2`), Rust (`v8 = "150"`), CMake (shim), Cargo (workspace), Docker (`joedwards32/cs2`) for live verification, GitHub Actions for CI.

## Global Constraints

Every task's requirements implicitly include these (copied from the spec):

- **Target Linux x86-64 only.** Windows is a documented TODO, not built this slice.
- **No interface version string, offset, or signature is hardcoded in C++ or Rust.** They live in `gamedata/core.gamedata.jsonc` and are read at runtime.
- **`core` (and the eventual `@s2script/std`) must have zero dependency on any `games/*` crate.** CI fails the build on violation.
- **FFI surface is exactly 3 functions + 1 logger callback.** `s2script_core_init`, `s2script_core_eval`, `s2script_core_shutdown`, and the `s2_log_fn` callback. Nothing more.
- **No panic may unwind across the FFI boundary.** Every `extern "C"` entry point is wrapped in `std::panic::catch_unwind` and returns an error code.
- **V8 platform is initialized exactly once per process (guarded); `shutdown` disposes only isolate+context, never the platform.** `libs2script_core.so` is built resident (`-Wl,-z,nodelete`).
- **The core is called only from the engine main thread.** Per-thread state (isolate, logger pointer) is held in `thread_local!`; this is sound because Metamod invokes `Load`/`Unload`/frame callbacks on one OS thread.
- **Degrade per-step, never crash the server.** A missing interface or malformed gamedata logs a named error and continues; V8 still boots.
- **Stop at Slice 0.** No multiplexer, no detour/hook, no schema *walking* (the `SchemaSystem` interface is acquired but never walked), no entity wrappers, no lifecycle/ledger, no `.s2sp`, no inter-plugin layer, no registry, no TS transpile (eval a hardcoded JS string), no config/convar systems, no threadpool/async. Note later needs as TODO comments and stop.
- **rusty_v8 API note:** The Rust code below targets the pinned `v8 = "150.0.0"` crate. Before implementing each V8 step, confirm the exact signatures (`HandleScope`, `FunctionCallbackArguments`, `ReturnValue<…>` generics, `new_default_platform` arity) against docs.rs for the pinned version and adjust mechanically if they differ.
- **Commits are signed** (local ed25519 signing key is already configured) and frequent — one per step where a step says "Commit".

---

## File Structure

```
Cargo.toml                       # workspace root: members core, games/cs2
.cargo/config.toml               # (optional) shared rustflags
Makefile                         # top-level orchestration
.gitignore
CLAUDE.md                        # architecture §5 guardrails (Appendix A of this plan)
README.md                        # reproduce-from-scratch runbook (grown across tasks)
docs/ARCHITECTURE.md             # architecture §1–3 (durable record)
docs/superpowers/specs/2026-06-30-slice-0-boot-handshake-design.md   # the spec (exists)
docs/superpowers/plans/2026-06-30-slice-0-boot-handshake.md          # this plan (exists)

core/
  Cargo.toml                     # cdylib s2script_core; dep v8; NO games/*
  build.rs                       # emits -Wl,-z,nodelete; (optional) cbindgen
  src/lib.rs                     # module wiring + re-exports
  src/ffi.rs                     # the 3 extern "C" fns + catch_unwind + logger storage
  src/v8host.rs                  # platform-once, Host{isolate,context}, console.log, eval

games/cs2/
  Cargo.toml                     # empty placeholder crate
  src/lib.rs                     # doc comment only

shim/
  CMakeLists.txt                 # builds s2script.so against hl2sdk + metamod, links libcore
  src/s2script_mm.h              # ISmmPlugin subclass declaration
  src/s2script_mm.cpp            # ISmmPlugin impl: Load/Unload, gamedata, interfaces, core calls
  src/gamedata.h / .cpp          # reads gamedata/core.gamedata.jsonc
  include/s2script_core.h        # C ABI header (consumed by shim; committed)
  third_party/json.hpp           # nlohmann/json single header (JSONC parsing)

gamedata/
  core.gamedata.jsonc            # interface version strings

scripts/
  check-core-boundary.sh         # CI: core must not depend on games/*
  package-addon.sh               # assembles the addons/ tree from build outputs

docker/
  docker-compose.yml             # joedwards32/cs2 with addon + metamod mounted
  patch-gameinfo.sh              # injects the metamod SearchPath line on container start
  s2script.vdf                   # metamod plugin registration

third_party/                     # git submodules
  hl2sdk/                        # branch cs2, pinned, patch-capable
  metamod-source/                # dev (2.0) branch, pinned

.github/workflows/ci.yml         # Linux: cargo build + boundary check + cmake build
```

---

## Task 1: Workspace scaffold, governing docs, and the boundary CI check

**Files:**
- Create: `Cargo.toml`, `core/Cargo.toml`, `core/src/lib.rs`, `core/build.rs`, `games/cs2/Cargo.toml`, `games/cs2/src/lib.rs`
- Create: `scripts/check-core-boundary.sh`
- Create: `Makefile`, `.gitignore`
- Create: `CLAUDE.md` (content = Appendix A), `docs/ARCHITECTURE.md` (content = Sections 1–3 of the s2script architecture document, copied verbatim)
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: a buildable Cargo workspace with crates `s2script-core` (cdylib, lib name `s2script_core`) and `s2script-cs2`; `scripts/check-core-boundary.sh` (exit 0 = OK, exit 1 = violation); `make check-boundary` target.

- [ ] **Step 1: Create the workspace and crate manifests**

`Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["core", "games/cs2"]
```

`core/Cargo.toml`:
```toml
[package]
name = "s2script-core"
version = "0.0.0"
edition = "2021"
publish = false

[lib]
name = "s2script_core"
crate-type = ["cdylib"]

[dependencies]
v8 = "150.0.0"
```

`games/cs2/Cargo.toml`:
```toml
[package]
name = "s2script-cs2"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
# Intentionally empty in Slice 0. CS2-specific code arrives in Slice 3+.
```

`core/build.rs`:
```rust
fn main() {
    // Keep libs2script_core.so resident for the process lifetime so the V8
    // platform survives a Metamod `meta unload` / `meta load` cycle (see ARCHITECTURE §2.1 / spec §5).
    println!("cargo:rustc-link-arg=-Wl,-z,nodelete");
}
```

`core/src/lib.rs`:
```rust
//! s2script engine-generic core. Embeds V8 and exposes a tiny C ABI.
//! MUST NOT depend on any game package (enforced by scripts/check-core-boundary.sh).

mod ffi;
mod v8host;
```

`games/cs2/src/lib.rs`:
```rust
//! Placeholder for the @s2script/cs2 game package.
//! Empty in Slice 0 — exists to establish the engine-generic/per-game boundary.
```

- [ ] **Step 2: Write the boundary check script**

`scripts/check-core-boundary.sh`:
```bash
#!/usr/bin/env bash
# Fails (exit 1) if s2script-core depends, directly or transitively, on any games/* crate.
set -euo pipefail
cd "$(dirname "$0")/.."

# Names of all packages whose manifest lives under games/
mapfile -t GAME_PKGS < <(
  cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import sys,json; m=json.load(sys.stdin); [print(p["name"]) for p in m["packages"] if "/games/" in p["manifest_path"]]'
)

# The full normal-dependency closure of s2script-core
DEPS="$(cargo tree -p s2script-core --edges normal --prefix none | awk '{print $1}' | sort -u)"

violation=0
for g in "${GAME_PKGS[@]}"; do
  if grep -qx "$g" <<<"$DEPS"; then
    echo "BOUNDARY VIOLATION: s2script-core depends on game package '$g'" >&2
    violation=1
  fi
done

if [ "$violation" -ne 0 ]; then exit 1; fi
echo "core boundary OK: s2script-core depends on no games/* crate"
```
Make it executable in Step 5's commit (`chmod +x`).

- [ ] **Step 3: Verify the build and the boundary check pass**

Run:
```bash
chmod +x scripts/check-core-boundary.sh
cargo build
./scripts/check-core-boundary.sh
```
Expected: `cargo build` compiles both crates (downloads + builds the `v8` crate's prebuilt binary — this is slow the first time, that's normal); the script prints `core boundary OK: …` and exits 0.

- [ ] **Step 4: Prove the boundary check actually catches a violation (negative test)**

Temporarily add to `core/Cargo.toml` under `[dependencies]`:
```toml
s2script-cs2 = { path = "../games/cs2" }
```
Run:
```bash
./scripts/check-core-boundary.sh; echo "exit=$?"
```
Expected: prints `BOUNDARY VIOLATION: s2script-core depends on game package 's2script-cs2'` and `exit=1`.
Then **revert** the edit (remove the line) and re-run the script to confirm it returns to exit 0. This step has no commit of its own — it validates the guard, then restores the clean state.

- [ ] **Step 5: Create governing docs, Makefile, gitignore, CI, and commit**

`.gitignore`:
```gitignore
/target
/build
/dist
*.so
docker/cs2-data/
```

`Makefile`:
```makefile
.PHONY: all core shim package check-boundary docker-test clean

all: core shim package

core:
	cargo build --release

check-boundary:
	./scripts/check-core-boundary.sh

shim:
	cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release
	cmake --build build/shim -j

package:
	./scripts/package-addon.sh

docker-test:
	docker compose -f docker/docker-compose.yml up

clean:
	cargo clean
	rm -rf build dist
```

`CLAUDE.md`: create with the full content of **Appendix A** of this plan (the architecture §5 standing conventions & guardrails).

`docs/ARCHITECTURE.md`: create with **Sections 1–3** of the s2script architecture document, copied verbatim (the durable design record; the operator has the canonical source document).

`.github/workflows/ci.yml`:
```yaml
name: ci
on: [push, pull_request]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }
      - uses: dtolnay/rust-toolchain@stable
      - name: cargo build
        run: cargo build --release
      - name: core boundary check
        run: ./scripts/check-core-boundary.sh
      # The CMake shim build is added in Task 3 (needs vendored SDKs).
```

Commit:
```bash
chmod +x scripts/check-core-boundary.sh
git add Cargo.toml core games .gitignore Makefile scripts CLAUDE.md docs/ARCHITECTURE.md .github
git commit -m "chore: scaffold workspace, governing docs, and core/game boundary check"
```

---

## Task 2: Vendor hl2sdk + metamod-source as pinned, patch-capable submodules

**Files:**
- Create: `.gitmodules` (via `git submodule add`)
- Add: `third_party/hl2sdk` (branch `cs2`), `third_party/metamod-source` (branch `dev`)
- Modify: `README.md` (add the "SDK vendoring & patch workflow" section)

**Interfaces:**
- Consumes: nothing from prior tasks.
- Produces: vendored headers on disk at `third_party/hl2sdk/**` and `third_party/metamod-source/**`, pinned to specific commits; a documented patch workflow.

- [ ] **Step 1: Add the submodules and pin them**

Run:
```bash
git submodule add -b cs2 https://github.com/alliedmodders/hl2sdk third_party/hl2sdk
git submodule add -b dev https://github.com/alliedmodders/metamod-source third_party/metamod-source
git submodule update --init --recursive
```

- [ ] **Step 2: Record the exact pinned commits**

Run:
```bash
git -C third_party/hl2sdk rev-parse HEAD
git -C third_party/metamod-source rev-parse HEAD
git submodule status
```
Expected: each prints a commit SHA; `git submodule status` shows both at those SHAs. Note both SHAs — they go in the README in Step 4.

- [ ] **Step 3: Verify the headers needed later are present**

Run:
```bash
ls third_party/hl2sdk/public/ISmmPlugin.h 2>/dev/null || ls third_party/metamod-source/core/ISmmPlugin.h
ls third_party/hl2sdk/public/tier1/interface.h
ls third_party/hl2sdk/public/eiface.h
ls third_party/metamod-source/core/sourcehook/sourcehook.h
```
Expected: the `ISmmPlugin.h`, `interface.h` (defines `CreateInterfaceFn`), `eiface.h`, and `sourcehook.h` paths resolve. If a path differs in the pinned tree, record the actual path — Task 3's CMake include dirs depend on it. (`ISmmPlugin.h` ships in metamod-source, not hl2sdk; confirm which.)

- [ ] **Step 4: Document the vendoring + patch workflow in the README, and commit**

Append to `README.md` a section **"Vendored SDKs (hl2sdk, Metamod:Source)"** stating:
- The two submodules and their pinned SHAs (from Step 2).
- How to update: `git -C third_party/hl2sdk fetch && git -C third_party/hl2sdk checkout <newsha>`, then commit the submodule bump.
- The **patch workflow** (hl2sdk lags Valve, so we carry local patches ahead of upstream):
  > Make changes directly in `third_party/hl2sdk`, then `git -C third_party/hl2sdk diff > patches/hl2sdk/NNNN-description.patch`. On a fresh checkout, patches in `patches/hl2sdk/` are re-applied in order via `make apply-patches` (added when the first patch is needed). Each patch is reviewed and tracked in the update-day fire drill.

Commit:
```bash
git add .gitmodules third_party/hl2sdk third_party/metamod-source README.md
git commit -m "chore: vendor hl2sdk (cs2) and metamod-source (dev) as pinned submodules"
```

---

## Task 3: Minimal C++ `ISmmPlugin` shim that builds (no V8)

This is the "prove the boot path before V8" checkpoint. It produces a loadable Metamod plugin that does nothing but log on load/unload. No Rust, no interfaces, no gamedata yet.

**Files:**
- Create: `shim/CMakeLists.txt`, `shim/src/s2script_mm.h`, `shim/src/s2script_mm.cpp`
- Modify: `.github/workflows/ci.yml` (add the CMake build step)

**Interfaces:**
- Consumes: vendored headers from Task 2.
- Produces: `build/shim/s2script.so`, a Metamod plugin exporting the MM:S entry point with plugin info (name `s2script`, a version string), and `Load`/`Unload` that log via `META_CONPRINTF`.

> **Reference, do not reinvent:** Model `s2script_mm.{h,cpp}` and the CMake define/include set on **CounterStrikeSharp `src/mm_plugin.cpp`** and the Metamod:Source **`sample_mm`** plugin in `third_party/metamod-source`. Copy the exact `ISmmPlugin` virtual signatures, `PLUGIN_EXPOSE`/`PLUGIN_GLOBALVARS`/`PLUGIN_SAVEVARS` macros, and the required preprocessor defines from there — they are version-pinned in your submodules.

- [ ] **Step 1: Write the shim header**

`shim/src/s2script_mm.h`:
```cpp
#pragma once
#include <ISmmPlugin.h>

class S2ScriptPlugin : public ISmmPlugin {
public:
    bool Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) override;
    bool Unload(char* error, size_t maxlen) override;

    // Plugin info
    const char* GetAuthor() override      { return "s2script"; }
    const char* GetName() override        { return "s2script"; }
    const char* GetDescription() override { return "TypeScript plugin runtime for Source 2"; }
    const char* GetURL() override         { return "https://s2script.com"; }
    const char* GetLicense() override     { return "TBD"; }
    const char* GetVersion() override     { return "0.0.0-slice0"; }
    const char* GetDate() override        { return __DATE__; }
    const char* GetLogTag() override      { return "S2SCRIPT"; }
};

extern S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_GLOBALVARS();
```

- [ ] **Step 2: Write the shim implementation (logging only)**

`shim/src/s2script_mm.cpp`:
```cpp
#include "s2script_mm.h"

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();
    META_CONPRINTF("[s2script] Load(): boot handshake (no V8 yet)\n");
    return true;
}

bool S2ScriptPlugin::Unload(char* error, size_t maxlen) {
    META_CONPRINTF("[s2script] Unload(): clean teardown\n");
    return true;
}
```
> Confirm `PLUGIN_SAVEVARS`/`PLUGIN_EXPOSE`/`META_CONPRINTF` spellings against the pinned metamod headers; mirror `sample_mm` exactly.

- [ ] **Step 3: Write the CMake build**

`shim/CMakeLists.txt`:
```cmake
cmake_minimum_required(VERSION 3.20)
project(s2script_shim CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

set(HL2SDK ${CMAKE_SOURCE_DIR}/../third_party/hl2sdk)
set(MMS    ${CMAKE_SOURCE_DIR}/../third_party/metamod-source)

add_library(s2script SHARED
    src/s2script_mm.cpp
)

target_include_directories(s2script PRIVATE
    src
    include
    ${MMS}/core
    ${MMS}/core/sourcehook
    ${HL2SDK}/public
    ${HL2SDK}/public/engine
    ${HL2SDK}/public/tier0
    ${HL2SDK}/public/tier1
)

# Source 2 / CS2 build defines. Confirm/extend against CounterStrikeSharp's build script.
target_compile_definitions(s2script PRIVATE
    _LINUX POSIX COMPILER_GCC PLATFORM_64BITS
    META_NO_HL2SDK
    stricmp=strcasecmp strnicmp=strncasecmp _stricmp=strcasecmp _vsnprintf=vsnprintf
)

# Output a bare s2script.so (no lib prefix) to match Metamod's expectations.
set_target_properties(s2script PROPERTIES PREFIX "" OUTPUT_NAME "s2script")
```
> The define set above is the common Source 2 baseline; reconcile it with `third_party/metamod-source` and CSS until it compiles. Add `tier0`/`tier1` static libs from hl2sdk to `target_link_libraries` only if the linker reports undefined symbols (the minimal logging plugin may not need them).

- [ ] **Step 4: Build the shim and verify the output**

Run:
```bash
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release && cmake --build build/shim -j
ls -la build/shim/s2script.so
nm -D --defined-only build/shim/s2script.so | grep -i CreateInterface_MMS
```
Expected: `s2script.so` exists; `nm` shows the exported Metamod entry symbol (`CreateInterface_MMS` or the macro-generated export — confirm the exact name from `PLUGIN_EXPOSE`). If the build fails, the fix is almost always a missing include dir or define — adjust Step 3 against the reference, do not stub anything out.

- [ ] **Step 5: Add the shim build to CI and commit**

In `.github/workflows/ci.yml`, append after the boundary check:
```yaml
      - name: build shim
        run: |
          cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release
          cmake --build build/shim -j
```
Commit:
```bash
git add shim .github/workflows/ci.yml
git commit -m "feat(shim): minimal ISmmPlugin that loads and logs (pre-V8)"
```

---

## Task 4: Docker verification harness + first live gate (empty shim loads)

Builds the operator-driven harness and uses it to confirm the empty shim loads/unloads on a real server **before** any V8 is added — the cheapest possible boot-risk retirement.

**Files:**
- Create: `docker/docker-compose.yml`, `docker/patch-gameinfo.sh`, `docker/s2script.vdf`
- Create: `scripts/package-addon.sh`
- Modify: `README.md` (add the "Docker verification runbook" section)

**Interfaces:**
- Consumes: `build/shim/s2script.so` from Task 3.
- Produces: a `make docker-test` flow that mounts `dist/addons/` into `joedwards32/cs2`, injects the metamod SearchPath, and exposes the server console/RCON; a runbook with exact `meta` commands.

> **Must confirm against the image:** the exact game directory inside `joedwards32/cs2` (commonly `/home/steam/cs2-dedicated/game/csgo`), the addons subpath, and the MM:S 2.0 `.vdf` `file` path format for Source 2. Verify by shelling into the container (`docker run --rm -it joedwards32/cs2 bash`) and inspecting `game/csgo/`.

- [ ] **Step 1: Write the addon packaging script**

`scripts/package-addon.sh`:
```bash
#!/usr/bin/env bash
# Assembles dist/addons/ from build outputs for mounting into the CS2 server.
set -euo pipefail
cd "$(dirname "$0")/.."

DIST=dist/addons
rm -rf "$DIST"
mkdir -p "$DIST/s2script/bin/linuxsteamrt64"
mkdir -p "$DIST/s2script/gamedata"

cp build/shim/s2script.so "$DIST/s2script/bin/linuxsteamrt64/s2script.so"
# libs2script_core.so is added in Task 6; copy if present.
[ -f target/release/libs2script_core.so ] && cp target/release/libs2script_core.so "$DIST/s2script/bin/linuxsteamrt64/"
[ -f gamedata/core.gamedata.jsonc ] && cp gamedata/core.gamedata.jsonc "$DIST/s2script/gamedata/"
cp docker/s2script.vdf "$DIST/metamod/s2script.vdf" 2>/dev/null || { mkdir -p "$DIST/metamod"; cp docker/s2script.vdf "$DIST/metamod/s2script.vdf"; }

echo "packaged: $DIST"
```

- [ ] **Step 2: Write the metamod plugin registration and gameinfo patcher**

`docker/s2script.vdf`:
```
"Metamod Plugin"
{
    "alias"  "s2script"
    "file"   "addons/s2script/bin/s2script"
}
```
> Metamod appends the platform/extension to `file`. Confirm the exact path convention (with or without `bin/linuxsteamrt64`) against a working MM:S 2.0 CS2 install; adjust to match where `package-addon.sh` places the `.so`.

`docker/patch-gameinfo.sh`:
```bash
#!/usr/bin/env bash
# Injects the Metamod SearchPath into csgo/gameinfo.gi (idempotent). Run inside the container on start.
set -euo pipefail
GI="${1:-/home/steam/cs2-dedicated/game/csgo/gameinfo.gi}"
if grep -q "csgo/addons/metamod" "$GI"; then
  echo "gameinfo.gi already patched"; exit 0
fi
# Insert the metamod Game path as the first entry under SearchPaths.
sed -i '/SearchPaths$/,/}/{s#\(Game[[:space:]]*csgo\)#Game	csgo/addons/metamod\n\t\t\t\1#}' "$GI"
echo "patched $GI"; grep -n "metamod" "$GI" || true
```
> The `sed` target depends on the exact `gameinfo.gi` whitespace/structure in the image — verify the inserted line lands inside the `SearchPaths { }` block and before the stock `Game csgo` line. Adjust the pattern after inspecting the real file.

- [ ] **Step 3: Write the compose file**

`docker/docker-compose.yml`:
```yaml
services:
  cs2:
    image: joedwards32/cs2
    container_name: s2script-cs2
    stdin_open: true
    tty: true
    environment:
      CS2_SERVERNAME: "s2script-slice0"
      CS2_LAN: "1"
      CS2_RCONPW: "s2script"
      CS2_PORT: "27015"
      SRCDS_TOKEN: ""        # not required for LAN
    ports:
      - "27015:27015/udp"
      - "27015:27015/tcp"
    volumes:
      - ./cs2-data:/home/steam/cs2-dedicated
      - ../dist/addons/s2script:/home/steam/cs2-dedicated/game/csgo/addons/s2script:ro
      - ../dist/addons/metamod/s2script.vdf:/home/steam/cs2-dedicated/game/csgo/addons/metamod/s2script.vdf:ro
      - ../third_party/metamod-source-build/metamod:/home/steam/cs2-dedicated/game/csgo/addons/metamod
      - ./patch-gameinfo.sh:/patch-gameinfo.sh:ro
```
> Two things to settle against reality: (1) Metamod itself must be installed into `csgo/addons/metamod` — either mount a built MM:S `dev` package there, or `make`-build it from the submodule into `third_party/metamod-source-build/`. Document whichever you choose. (2) The image may already run an entrypoint; you may need to invoke `patch-gameinfo.sh` once after first boot (the env var `CS2_CFG`/exec hook or a manual `docker exec s2script-cs2 /patch-gameinfo.sh`). Capture the exact step in the runbook.

- [ ] **Step 4: Package, run, and execute the first live gate**

Run:
```bash
make shim
make package
docker compose -f docker/docker-compose.yml up -d
# First boot downloads CS2 (slow). Then patch and load:
docker exec s2script-cs2 /patch-gameinfo.sh
docker compose -f docker/docker-compose.yml restart
docker attach s2script-cs2   # or: connect via RCON
```
In the server console run, and record output:
```
meta list
meta unload s2script
meta load addons/s2script/bin/s2script
```
**Live gate (record results in the README runbook):**
- `meta list` shows `s2script 0.0.0-slice0`.
- The console shows `[s2script] Load(): boot handshake (no V8 yet)`.
- `meta unload` shows the Unload line and does **not** crash the server.
- `meta load` brings it back without a restart.

If any of these fail, stop and fix here — the entire slice rests on this boot path. This is the moment the spec's §5 unknown first gets exercised (minus V8).

- [ ] **Step 5: Document the runbook and commit**

Append to `README.md` a **"Docker verification runbook"** section with the exact commands from Step 4, the expected console lines, and any image-specific paths you confirmed (game dir, metamod install method, vdf path, the gameinfo patch step).
```bash
chmod +x scripts/package-addon.sh docker/patch-gameinfo.sh
git add docker scripts/package-addon.sh README.md
git commit -m "feat(verify): dockerized CS2 harness; empty shim loads/unloads on a live server"
```

---

## Task 5: Rust V8 core + C ABI (TDD)

The one genuinely unit-testable unit. Build it test-first with `cargo test`; no CS2 needed.

**Files:**
- Create/modify: `core/src/v8host.rs`, `core/src/ffi.rs`, `core/src/lib.rs`
- Create: `shim/include/s2script_core.h` (the committed C header)
- Test: inline `#[cfg(test)]` module in `core/src/ffi.rs`

**Interfaces:**
- Consumes: nothing from prior tasks (the core is standalone).
- Produces the C ABI (also written to `shim/include/s2script_core.h`):
  ```c
  typedef void (*s2_log_fn)(int level, const char* utf8_msg);
  int  s2script_core_init(s2_log_fn logger);    // 0 = ok
  int  s2script_core_eval(const char* utf8_js); // 0 = ok; non-zero on JS error
  void s2script_core_shutdown(void);            // dispose isolate+context only
  ```
  Rust-side types: `pub type LogFn = extern "C" fn(c_int, *const c_char);`

- [ ] **Step 1: Write the failing test (capture console.log through the full init→eval→shutdown cycle)**

In `core/src/ffi.rs`, add at the bottom:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};
    use std::sync::Mutex;

    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn test_logger(_level: c_int, msg: *const c_char) {
        let s = unsafe { CStr::from_ptr(msg) }.to_string_lossy().into_owned();
        CAPTURED.lock().unwrap().push(s);
    }

    #[test]
    fn init_eval_console_log_shutdown_and_reinit() {
        CAPTURED.lock().unwrap().clear();

        assert_eq!(s2script_core_init(Some(test_logger)), 0);
        assert_eq!(s2script_core_eval(b"console.log('hello from V8 in CS2')\0".as_ptr() as *const c_char), 0);
        s2script_core_shutdown();

        // platform must survive shutdown: a second cycle works without re-init of the platform
        assert_eq!(s2script_core_init(Some(test_logger)), 0);
        assert_eq!(s2script_core_eval(b"console.log('second cycle')\0".as_ptr() as *const c_char), 0);
        s2script_core_shutdown();

        let got = CAPTURED.lock().unwrap().clone();
        assert!(got.iter().any(|m| m.contains("hello from V8 in CS2")), "got: {:?}", got);
        assert!(got.iter().any(|m| m.contains("second cycle")), "got: {:?}", got);
    }

    #[test]
    fn eval_with_js_exception_returns_nonzero_and_does_not_panic() {
        assert_eq!(s2script_core_init(Some(test_logger)), 0);
        let rc = s2script_core_eval(b"throw new Error('boom')\0".as_ptr() as *const c_char);
        assert_ne!(rc, 0);
        s2script_core_shutdown();
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p s2script-core`
Expected: FAIL — `s2script_core_init`/`_eval`/`_shutdown` don't exist yet (compile error). That is the expected "red".

- [ ] **Step 3: Implement the V8 host**

`core/src/v8host.rs`:
```rust
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::Once;

pub type LogFn = extern "C" fn(c_int, *const c_char);

static PLATFORM_INIT: Once = Once::new();

thread_local! {
    static LOGGER: std::cell::Cell<Option<LogFn>> = std::cell::Cell::new(None);
    static HOST: std::cell::RefCell<Option<Host>> = std::cell::RefCell::new(None);
}

/// Initialize the V8 platform exactly once for the process. Never torn down
/// (the cdylib is resident via -Wl,-z,nodelete), so a meta unload/reload cycle is safe.
fn ensure_platform() {
    PLATFORM_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

struct Host {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
}

fn console_log(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let msg = if args.length() > 0 {
        args.get(0).to_rust_string_lossy(scope)
    } else {
        String::new()
    };
    LOGGER.with(|l| {
        if let Some(f) = l.get() {
            if let Ok(c) = CString::new(msg) {
                f(0, c.as_ptr());
            }
        }
    });
}

pub fn init(logger: LogFn) -> Result<(), String> {
    ensure_platform();
    LOGGER.with(|l| l.set(Some(logger)));

    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let context = {
        let scope = &mut v8::HandleScope::new(&mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let scope = &mut v8::ContextScope::new(scope, context);

        // Install global `console = { log }`.
        let global = context.global(scope);
        let console = v8::Object::new(scope);
        let log_key = v8::String::new(scope, "log").unwrap();
        let log_fn = v8::Function::new(scope, console_log).unwrap();
        console.set(scope, log_key.into(), log_fn.into());
        let console_key = v8::String::new(scope, "console").unwrap();
        global.set(scope, console_key.into(), console.into());

        v8::Global::new(scope, context)
    };

    HOST.with(|h| *h.borrow_mut() = Some(Host { isolate, context }));
    Ok(())
}

pub fn eval(src: &str) -> Result<(), String> {
    HOST.with(|h| {
        let mut borrow = h.borrow_mut();
        let host = borrow.as_mut().ok_or_else(|| "core not initialized".to_string())?;
        let scope = &mut v8::HandleScope::with_context(&mut host.isolate, &host.context);
        let tc = &mut v8::TryCatch::new(scope);
        let code = v8::String::new(tc, src).ok_or_else(|| "failed to allocate source string".to_string())?;
        let script = match v8::Script::compile(tc, code, None) {
            Some(s) => s,
            None => return Err(exception_message(tc)),
        };
        match script.run(tc) {
            Some(_) => Ok(()),
            None => Err(exception_message(tc)),
        }
    })
}

pub fn shutdown() {
    // Drop the isolate + context. NEVER dispose the platform.
    HOST.with(|h| { let _ = h.borrow_mut().take(); });
}

fn exception_message(tc: &mut v8::TryCatch<v8::HandleScope>) -> String {
    if let Some(ex) = tc.exception() {
        let scope = tc.escape_or_throw(); // see note below
        return ex.to_rust_string_lossy(scope);
    }
    "unknown JavaScript error".to_string()
}
```
> rusty_v8 caveats to reconcile against docs.rs for `v8 = "150"`: the exact `ReturnValue` generic, `Context::new`/`ContextOptions` arity, and how to format an exception message from a `TryCatch`. The simplest robust `exception_message` is: `tc.exception().map(|e| e.to_rust_string_lossy(tc)).unwrap_or_else(|| "unknown JS error".into())` — `TryCatch` derefs to the scope, so pass `tc` where a scope is needed. Replace the placeholder `escape_or_throw` line with that pattern.

- [ ] **Step 4: Implement the FFI shims with `catch_unwind`**

`core/src/ffi.rs` (above the test module):
```rust
use crate::v8host::{self, LogFn};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;

#[no_mangle]
pub extern "C" fn s2script_core_init(logger: Option<LogFn>) -> c_int {
    catch_unwind(|| {
        let Some(logger) = logger else { return -2 };
        match v8host::init(logger) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
    .unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_eval(src: *const c_char) -> c_int {
    catch_unwind(|| {
        if src.is_null() { return -2; }
        let s = match unsafe { std::ffi::CStr::from_ptr(src) }.to_str() {
            Ok(s) => s,
            Err(_) => return -3,
        };
        match v8host::eval(s) {
            Ok(()) => 0,
            Err(_) => -1, // JS error already surfaced via console/logger path in later slices
        }
    })
    .unwrap_or(-99)
}

#[no_mangle]
pub extern "C" fn s2script_core_shutdown() {
    let _ = catch_unwind(|| v8host::shutdown());
}
```
And ensure `core/src/lib.rs` has `mod ffi; mod v8host;` (from Task 1).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p s2script-core`
Expected: both tests PASS (the cycle captures both log lines; the exception test returns non-zero without panicking).
> If `cargo test` complains about V8 being initialized twice across tests, force single-threaded test execution: `cargo test -p s2script-core -- --test-threads=1`. Add that note to the README. (V8 platform init-once is process-global; parallel tests must not race it.)

- [ ] **Step 6: Write the C header and commit**

`shim/include/s2script_core.h`:
```c
#ifndef S2SCRIPT_CORE_H
#define S2SCRIPT_CORE_H
#ifdef __cplusplus
extern "C" {
#endif

/* level: 0=info (reserved for warn/error in later slices) */
typedef void (*s2_log_fn)(int level, const char* utf8_msg);

/* Returns 0 on success, negative on error. */
int  s2script_core_init(s2_log_fn logger);
int  s2script_core_eval(const char* utf8_js);
void s2script_core_shutdown(void);

#ifdef __cplusplus
}
#endif
#endif /* S2SCRIPT_CORE_H */
```
Commit:
```bash
git add core/src shim/include/s2script_core.h
git commit -m "feat(core): V8 host + 3-fn C ABI with console.log, catch_unwind, platform-once (TDD)"
```

---

## Task 6: Wire shim → core, V8 hello end-to-end, second live gate

Connects the C++ shim to the Rust core: on load, init the core (passing a logger that routes to `META_CONPRINTF`) and eval the hardcoded hello; on unload, shut the core down. Then verify on the Docker server — including the unload/reload cycle that validates the §5 platform-persistence posture.

**Files:**
- Modify: `shim/CMakeLists.txt` (link `libs2script_core.so`), `shim/src/s2script_mm.cpp` (call the core), `Makefile` (build core before shim), `scripts/package-addon.sh` (already copies libcore)
- Modify: `README.md` (record the V8 live gate)

**Interfaces:**
- Consumes: `s2script_core_init/_eval/_shutdown` (Task 5), `s2script_core.h` (Task 5).
- Produces: `s2script.so` that boots V8 and prints `hello from V8 in CS2` on a live server.

- [ ] **Step 1: Call the core from the shim**

Edit `shim/src/s2script_mm.cpp`. Add the include and a C logger callback, and call the core in Load/Unload:
```cpp
#include "s2script_mm.h"
#include "s2script_core.h"

S2ScriptPlugin g_S2ScriptPlugin;
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

static void s2_logger(int level, const char* msg) {
    META_CONPRINTF("[s2script] %s\n", msg);
}

bool S2ScriptPlugin::Load(PluginId id, ISmmAPI* ismm, char* error, size_t maxlen, bool late) {
    PLUGIN_SAVEVARS();
    META_CONPRINTF("[s2script] Load(): initializing V8 core\n");

    if (s2script_core_init(&s2_logger) != 0) {
        META_CONPRINTF("[s2script] ERROR: V8 core init failed (plugin stays loaded for diagnosis)\n");
        return true; // degrade, do not fail the load (spec §7)
    }
    s2script_core_eval("console.log('hello from V8 in CS2')");
    return true;
}

bool S2ScriptPlugin::Unload(char* error, size_t maxlen) {
    META_CONPRINTF("[s2script] Unload(): shutting down V8 core\n");
    s2script_core_shutdown();
    return true;
}
```

- [ ] **Step 2: Link the core into the shim**

In `shim/CMakeLists.txt`, after the `add_library` block:
```cmake
# Link the resident Rust core (built by `cargo build --release`).
target_link_libraries(s2script PRIVATE
    ${CMAKE_SOURCE_DIR}/../target/release/libs2script_core.so
)
target_include_directories(s2script PRIVATE include)
# Ensure the runtime can find libs2script_core.so next to the plugin.
set_target_properties(s2script PROPERTIES
    BUILD_RPATH "$ORIGIN"
    INSTALL_RPATH "$ORIGIN"
)
```
And make `make shim` depend on `make core` — in the `Makefile`, change:
```makefile
shim: core
	cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release
	cmake --build build/shim -j
```

- [ ] **Step 3: Build everything and verify linkage**

Run:
```bash
make core
make shim
ldd build/shim/s2script.so | grep s2script_core || true
nm -D build/shim/s2script.so | grep -i s2script_core_init
```
Expected: the shim builds; `s2script.so` references `libs2script_core.so` (or has it via rpath) and the `s2script_core_init` symbol resolves. The two `.so` files ship together (package-addon.sh copies both into `bin/linuxsteamrt64/`).

- [ ] **Step 4: Package and run the second live gate (the real Slice 0 milestone)**

Run:
```bash
make package
docker compose -f docker/docker-compose.yml restart
docker attach s2script-cs2
```
In the console:
```
meta list
meta unload s2script
meta load addons/s2script/bin/s2script
```
**Live gate (record in README):**
- On boot/load: `[s2script] Load(): initializing V8 core` then `[s2script] hello from V8 in CS2` appear in the console.
- `meta list` shows the plugin.
- `meta unload` runs `Unload()` (and `shutdown()`) with **no crash**.
- `meta load` boots it again and **`hello from V8 in CS2` prints a second time without a server restart** — this is the direct validation of the spec §5 resident-cdylib / platform-once posture.

If `meta load` after `meta unload` crashes or fails to re-print, that is the §5 unknown materializing. Capture the exact failure in the README "Findings" subsection and treat it as a Slice 0 result (it informs the fix in this slice or a documented constraint for Slice 1) — do not paper over it.

- [ ] **Step 5: Commit**

```bash
git add shim Makefile README.md
git commit -m "feat: wire shim to V8 core; hello-from-V8 end-to-end with clean unload/reload"
```

---

## Task 7: gamedata-driven interface acquisition + per-interface logging

Adds the data-driven interface layer: read version strings from `gamedata/core.gamedata.jsonc`, acquire the Source 2 interfaces via the engine/server factories, log success/failure per interface, and degrade (named warning, no crash) on any miss. Independent of V8 (spec §4) — done last so a failure here can't mask the V8 milestone.

**Files:**
- Create: `gamedata/core.gamedata.jsonc`, `shim/src/gamedata.h`, `shim/src/gamedata.cpp`, `shim/third_party/json.hpp`
- Modify: `shim/CMakeLists.txt` (add `gamedata.cpp`), `shim/src/s2script_mm.cpp` (acquire + log interfaces in Load)

**Interfaces:**
- Consumes: `ISmmAPI*` (from `Load`), the vendored `interface.h` `CreateInterfaceFn`.
- Produces: `LoadGamedata(path) -> map<string,string>`; per-interface acquisition logging in `Load`.

> **Reference, do not reinvent:** the exact factories and version strings for `Source2Server`, `SchemaSystem`, the cvar system, and the network/engine server service come from **CounterStrikeSharp `src/mm_plugin.cpp`** (`ismm->GetEngineFactory()`, `ismm->GetServerFactory()`, and the `schemasystem` module factory for `SchemaSystem`). Read it before writing Step 3.

- [ ] **Step 1: Write the gamedata file**

`gamedata/core.gamedata.jsonc`:
```jsonc
{
  // Source 2 engine interface version strings. Confirmed against the live CS2
  // binaries at build time — never hardcoded in C++/Rust (ARCHITECTURE §2.7).
  "interfaces": {
    "Source2Server": "Source2Server001",
    "SchemaSystem": "SchemaSystem_001",
    "EngineCvar": "VEngineCvar007",
    "NetworkServerService": "NetworkServerService_001"
  }
}
```
> Confirm each version string against the live server (CSS references / `meta` diagnostics). `VEngineCvar007` and `NetworkServerService_001` in particular drift — fix them here, in data, not in code.

- [ ] **Step 2: Add the JSON parser and gamedata loader**

Vendor `shim/third_party/json.hpp` (nlohmann/json single-header release). Then:

`shim/src/gamedata.h`:
```cpp
#pragma once
#include <map>
#include <string>

// Reads interface version strings from a gamedata .jsonc file.
// Returns an empty map (and leaves `error` set) on failure — caller degrades, never crashes.
std::map<std::string, std::string> LoadInterfaceVersions(const std::string& path, std::string& error);
```

`shim/src/gamedata.cpp`:
```cpp
#include "gamedata.h"
#include "../third_party/json.hpp"
#include <fstream>

std::map<std::string, std::string> LoadInterfaceVersions(const std::string& path, std::string& error) {
    std::map<std::string, std::string> out;
    std::ifstream f(path);
    if (!f) { error = "gamedata file not found: " + path; return out; }
    try {
        // ignore_comments = true → JSONC support
        auto j = nlohmann::json::parse(f, nullptr, true, true);
        for (auto& [k, v] : j.at("interfaces").items()) {
            out[k] = v.get<std::string>();
        }
    } catch (const std::exception& e) {
        error = std::string("gamedata parse error: ") + e.what();
        out.clear();
    }
    return out;
}
```

- [ ] **Step 3: Acquire and log interfaces in Load (before core init)**

In `shim/src/s2script_mm.cpp`, add `#include "gamedata.h"` and, at the top of `Load` (before the V8 init), insert:
```cpp
    // --- Interface acquisition (data-driven, degrade-never-crash) ---
    std::string gdError;
    // Resolve the gamedata path relative to the plugin/install (confirm the runtime cwd on the server).
    auto versions = LoadInterfaceVersions("addons/s2script/gamedata/core.gamedata.jsonc", gdError);
    if (!gdError.empty()) {
        META_CONPRINTF("[s2script] WARN: %s — skipping interface acquisition\n", gdError.c_str());
    } else {
        auto tryGet = [&](const char* key, CreateInterfaceFn factory) {
            auto it = versions.find(key);
            if (it == versions.end()) {
                META_CONPRINTF("[s2script] WARN: no version string for %s in gamedata\n", key);
                return;
            }
            int ret = 0;
            void* iface = factory ? factory(it->second.c_str(), &ret) : nullptr;
            if (iface && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: %s (%s)\n", key, it->second.c_str());
            } else {
                META_CONPRINTF("[s2script] WARN: interface MISSING: %s (%s)\n", key, it->second.c_str());
            }
        };
        tryGet("Source2Server",        ismm->GetServerFactory(false));
        tryGet("EngineCvar",           ismm->GetEngineFactory(false));
        tryGet("NetworkServerService", ismm->GetEngineFactory(false));
        // SchemaSystem comes from the schemasystem module factory, not engine/server — see CSS.
        // tryGet("SchemaSystem", <schemasystem module factory>);
        META_CONPRINTF("[s2script] NOTE: SchemaSystem acquisition deferred — wire the schemasystem module factory per CSS\n");
    }
    // --- end interface acquisition ---
```
> Confirm `GetServerFactory`/`GetEngineFactory` signatures (some MM:S versions take a `bool` arg) and the `CreateInterfaceFn` return-by-pointer convention against the pinned headers. The `SchemaSystem` factory specifically is obtained differently (module factory) — leave the explicit NOTE until wired, rather than faking success.

- [ ] **Step 4: Add gamedata.cpp to the build and rebuild**

In `shim/CMakeLists.txt`, add `src/gamedata.cpp` to the `add_library(s2script SHARED …)` sources. Run:
```bash
make core && make shim && make package
```
Expected: builds clean; `dist/addons/s2script/gamedata/core.gamedata.jsonc` is present (package-addon.sh copies it).

- [ ] **Step 5: Live gate — interface logging + degradation**

Run the Docker restart + attach (as Task 6 Step 4) and confirm in console:
- `[s2script] interface OK: Source2Server (Source2Server001)` (and the others that resolve), or a named `WARN: interface MISSING:` line — **either is acceptable**; the requirement is a named, non-fatal result per interface.
- Then deliberately break one: edit `gamedata/core.gamedata.jsonc` to `"Source2Server": "Source2Server999"`, re-`make package`, restart. Confirm a `WARN: interface MISSING: Source2Server (Source2Server999)` line and **no server crash**. Restore the correct value afterward.

- [ ] **Step 6: Commit**

```bash
git add gamedata shim
git commit -m "feat(shim): data-driven Source 2 interface acquisition with per-interface logging"
```

---

## Task 8: Finalize the README runbook and run the full acceptance pass

**Files:**
- Modify: `README.md` (complete reproduce-from-scratch + acceptance checklist)

**Interfaces:**
- Consumes: everything built in Tasks 1–7.
- Produces: a README that reproduces the whole slice from a clean checkout, and a recorded pass of all six acceptance criteria.

- [ ] **Step 1: Write the complete reproduce-from-scratch README**

Ensure `README.md` covers, in order, with exact commands:
1. Clone + `git submodule update --init --recursive`.
2. Prerequisites (clang, cmake, cargo, docker) and the `--test-threads=1` note for `cargo test`.
3. `cargo build --release` (note the one-time `v8` crate download).
4. `make shim` (and the hl2sdk/metamod include/define reconciliation pointer).
5. `make package`.
6. Docker: bring up `joedwards32/cs2`, install Metamod, run `patch-gameinfo.sh`, mount the addon.
7. The `meta list` / `meta unload` / `meta load` runbook with expected console output.
8. The hl2sdk patch workflow (from Task 2).
9. A "Known findings / constraints" subsection capturing the §5 unload/reload result.

- [ ] **Step 2: Run the full acceptance checklist on a clean checkout**

In a fresh clone, execute the README end-to-end and confirm each spec §12 criterion, recording PASS/FAIL + evidence:
1. [ ] Builds for Linux x86-64 (`s2script.so` + `libs2script_core.so` produced).
2. [ ] Loads on the live CS2 server; `meta list` shows it with version; `meta unload` doesn't crash.
3. [ ] Per-interface acquisition success/failure logged; a missing interface is a named non-fatal warning.
4. [ ] V8 isolate embedded; `console.log` → `hello from V8 in CS2` appears in the console.
5. [ ] `meta unload` disposes cleanly; subsequent `meta load` works with no restart.
6. [ ] Every step reproduces from the README on a clean checkout.

- [ ] **Step 3: Commit and conclude the slice**

```bash
git add README.md
git commit -m "docs: complete Slice 0 reproduce-from-scratch runbook and acceptance results"
```
Stop here. Note any out-of-scope follow-ups (multiplexer, schema pipeline, lifecycle, etc.) as GitHub issues or TODO entries — do not begin Slice 1.

---

## Appendix A — `CLAUDE.md` content (architecture §5 standing conventions & guardrails)

Create `CLAUDE.md` (Task 1, Step 5) with exactly this content:

```markdown
# s2script — standing conventions & guardrails

- **The core owns every engine touchpoint.** Plugins never get raw detours; they get named, typed, multiplexed events + the single `HookResult` contract. Only exception: the explicit `unsafe` module.
- **Core is engine-generic; games are packages. Dependencies point one way: game → core, never core → game.** The core knows Source 2, never a specific game. Game classes, gamedata, descriptor bindings, team/weapon APIs live in `@s2script/cs2` (and future `@s2script/<game>`). A CI check fails the build if core/std imports a game package. Litmus test: *would it still be true on a different Source 2 game?* If no → it's a game package, not core.
- **Never expose a raw pointer or raw cross-plugin reference across time.** Entities, shorter-than-plugin resources, and inter-plugin interfaces are handle/proxy-backed and host-invalidated; safe accessors return `T | null`; raw-live views are block-scoped and cannot cross `await`. Entity refs on the inter-plugin wire use the same `EntityRef`/`T | null` type as the entity system.
- **Cross-plugin comms are typed, versioned interfaces.** Methods = natives, events = forwards, one object, semver-governed. Hard deps return a proxy that throws on producer-unload; optional deps return `Interface | null`. All imports ledgered; unload resolves in reverse-dependency order.
- **`package.json` is the authoring format; reuse npm standards.** Standard fields for what npm models; the `s2script` block for engine facts (`apiVersion`, `publishes`, `pluginDependencies`/`optionalPluginDependencies`, `requiresGamedata`, `permissions`, `config`). `dependencies` = npm build-deps only; inter-plugin deps under `s2script`. Never overload npm's `exports`. The runtime consumes a derived minimal manifest baked into the `.s2sp`, never the full `package.json`.
- **npm scope taxonomy, one reserved official scope.** `@s2script/*` = first-party (engine-generic std lib `@s2script/std`; per-game `@s2script/cs2`/`@s2script/<game>` which ship that game's schema types; base plugins). `@<community>/*` = verified third-party; unscoped allowed. Never name a package `@s2script/core` ("core" is the native layer). Reserve `@s2script` everywhere from day one.
- **Layout is data, semantics are code.** Offsets/signatures/struct positions/interface strings live in regenerable `gamedata`/schema files; behavioral facts and name mappings in reviewed code. A field-offset change must never require a code change.
- **hl2sdk is a pinned, vendored, patch-capable dependency** and part of the update-day treadmill — it lags Valve, so own your schema/offset layer rather than trusting the SDK's game-class fields.
- **Contracts are versioned: engine `.d.ts`, host `apiVersion`, plugin semver.** Breaking any is a major bump that fails fast at the typecheck gate and again at load — never a silent runtime drift.
- **Degrade per-descriptor, never crash globally.** A broken signature/offset/field disables *that* descriptor with a named reason; the framework keeps running.
- **The ledger is the teardown authority.** Every persistent resource (including imported interfaces and exported-interface consumers) is auto-ledgered; teardown walks the ledger and doesn't depend on the plugin's own cleanup code running correctly.
- **Typecheck-gate every load and reload** against the shipped `.d.ts` *and* declared dependency interfaces. A failing file-watch reload leaves the running version untouched.
- **Lock the package contract early, build the registry late.** The `package.json`/manifest contract is designed in now so `s2script.com` is a distribution layer over an existing model, not a retrofit.
- **The base-plugin suite is the std lib's acceptance test.** The std lib isn't done until the `@s2script/base*` SourceMod-parity plugins build cleanly on it; awkwardness there is a std lib bug. They're CS2 plugins (depend on `@s2script/cs2`), registry-distributed std-lib consumers, not built into the runtime.
- **Build by risk, not by layer.** Thin vertical slices to a working end-to-end thread before breadth. Resist building breadth on an unproven spine.
- **The maintenance treadmill is a first-class feature.** Per-update gamedata/schema/hl2sdk regeneration + validation tooling is the moat. Design for green-within-48h-of-every-patch.

## Current state
Slice 0 (boot handshake). Scope is strictly: host V8 inside CS2 via Metamod, `console.log`, clean teardown. See `docs/superpowers/specs/2026-06-30-slice-0-boot-handshake-design.md` and `docs/ARCHITECTURE.md`. Do not build past Slice 0.
```

## Self-Review (completed during planning)

- **Spec coverage:** §1 purpose → Tasks 5–6. §2 forks → reflected (Rust core T5, Docker T4/T6/T7). §3 components (shim/core/cs2) → Tasks 1,3,5. §4 boot sequence → Task 6 (+T7 interfaces). §5 platform persistence → T1 build.rs (`-z,nodelete`) + T5 `ensure_platform`/`shutdown` + T6 reload gate. §6 C ABI → T5. §7 gamedata + degradation → T7 (+T6 degrade-on-init-fail). §8 repo layout → T1. §9 build + boundary → T1. §10 Docker verify → T4/T6/T7/T8. §11 testing → T5 (cargo) + T1 (boundary) + Docker gates. §12 acceptance → T8. §13 out-of-scope → Global Constraints + T8 close. §14 deliverables → all tasks. §15 open items (version strings, dlclose behavior, image layout) → flagged in T2/T4/T6/T7. No spec section is unmapped.
- **Placeholder scan:** No "TBD/TODO" left as gaps in steps. The explicit "confirm against CSS/headers" notes are external-SDK binding guidance (the signatures are version-pinned in the submodules), not deferred work — each names the exact symbol/file to check. The `SchemaSystem` factory and one `GetLicense` string are intentionally surfaced as NOTEs rather than faked.
- **Type consistency:** C ABI `s2script_core_init(s2_log_fn)`/`_eval(const char*)`/`_shutdown(void)` identical across the header (T5/6), the Rust `extern "C"` defs (T5), and the C++ caller (T6). `LogFn`/`s2_log_fn` signature `(int, const char*)` consistent everywhere. `LoadInterfaceVersions` signature identical in `gamedata.h` and its caller (T7).
```
