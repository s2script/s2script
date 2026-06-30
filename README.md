# s2script

s2script is a TypeScript plugin framework for Source 2 engine games (Counter-Strike 2 first), loaded via Metamod:Source, that aims to be what SourceMod was to Source 1: the single, unified runtime that every server plugin loads into. Plugin authors write TypeScript against one standard library; the framework owns every engine touchpoint and multiplexes all plugins onto it. The core is engine-generic (knows Source 2, not any specific game); game-specific knowledge lives in per-game packages (`@s2script/cs2`). See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full design.

**This is Slice 0 — boot handshake.** Scope: host a V8 isolate inside CS2 via Metamod, acquire and log the core Source 2 interfaces, run `console.log`, and tear down cleanly. Everything past this slice is explicitly out of scope. Design spec: [`docs/superpowers/specs/2026-06-30-slice-0-boot-handshake-design.md`](docs/superpowers/specs/2026-06-30-slice-0-boot-handshake-design.md).

---

## Prerequisites

- **clang / clang++** (any recent version; used by the CMake shim build)
- **cmake >= 3.20**
- **cargo / rustc** (stable; tested with 1.77+)
- **docker** (for the live gate; see [Docker verification runbook](#docker-verification-runbook))
- **Linux x86-64** — the only supported build target this slice. Windows is a documented TODO.

> `cargo test -p s2script-core -- --test-threads=1` is required: the V8 platform is process-global and initialized exactly once, so parallel tests race that init.

---

## Reproduce from scratch

```bash
# 1. Clone and pull submodules
git clone https://github.com/gabriel-gkh/s2script.git
cd s2script
git submodule update --init --recursive

# 2. Build the Rust core (cdylib; V8 embedded)
#    First run: downloads the v8 prebuilt (~130 MB). Subsequent runs are instant.
make core        # cargo build --release -> target/release/libs2script_core.so

# 3. Build the Metamod plugin
make shim        # cmake -S shim -B build/shim + build -> build/shim/s2script.so

# 4. Package the addon directory
make package     # assembles dist/addons/

# 5. (Optional) verify the core/games boundary
make check-boundary
```

**v8 prebuilt pin:** the `v8` crate is pinned to **149.4.0** because its prebuilt was compiled with `v8_monolithic_for_shared_library=true`, which is required to link V8 into a `-shared` object (our `dlopen`'d Metamod plugin `.so`). Upgrading to v150+ requires a source build: `V8_FROM_SOURCE=1 GN_ARGS=v8_monolithic_for_shared_library=true cargo build`. See [Known findings](#known-findings--constraints) for the full TLS context.

After `make package` the artifact tree is:

```
dist/addons/
  metamod/
    s2script.vdf
  s2script/
    bin/
      linuxsteamrt64/
        libs2script_core.so
        s2script.so
    gamedata/
      core.gamedata.jsonc
```

---

## Vendored SDKs (hl2sdk, Metamod:Source)

Two upstream SDKs are vendored as pinned git submodules under `third_party/`:

| Submodule | Remote | Branch | Pinned SHA |
|---|---|---|---|
| `third_party/hl2sdk` | https://github.com/alliedmodders/hl2sdk | `cs2` | `9ab16fa9fcdeeb30565dfdbf6fbb312356978a0b` |
| `third_party/metamod-source` | https://github.com/alliedmodders/metamod-source | `master` | `a5f4cca5824c0c5f13e8fa100dd15df164d2db22` |

Note: the upstream metamod-source repo has no `dev` branch; `master` is the active development branch.

### Updating a submodule to a new upstream commit

```bash
git -C third_party/hl2sdk fetch
git -C third_party/hl2sdk checkout <newsha>
# then stage and commit the submodule pointer bump:
git add third_party/hl2sdk
git commit -m "chore: bump hl2sdk to <newsha>"
```

Same pattern applies for `third_party/metamod-source`.

### Patch workflow (hl2sdk)

hl2sdk occasionally lags Valve SDK updates, so we carry local patches ahead of upstream.

- Make changes directly in `third_party/hl2sdk`.
- Export the patch: `git -C third_party/hl2sdk diff HEAD > patches/hl2sdk/NNNN-description.patch`
  Note: use `diff HEAD` to capture both staged and unstaged changes; otherwise staged hunks may be silently dropped.
- On a fresh checkout, patches in `patches/hl2sdk/` are re-applied in order via `make apply-patches` (added when the first patch is needed).
- Each patch is reviewed and tracked in the update-day fire drill.

**Interface version strings** live in `gamedata/core.gamedata.jsonc` — never hardcoded in C++ or Rust. When a game update changes a version string, fix the gamedata file. Confirm the current string against the live binary with `meta interfaces`; the values there are ground truth, not the SDK headers.

---

## Docker verification runbook

This runbook uses `joedwards32/cs2` to confirm the Metamod:Source plugin
(`dist/addons/s2script/bin/linuxsteamrt64/s2script.so`) loads, acquires interfaces, boots V8,
and unloads cleanly on a real CS2 dedicated server.

> **IMPORTANT:** The `docker compose up` below triggers a ~30 GB CS2 download on first run.
> This gate is **not automated** — a human operator must execute it and record the output.

**Confirmed image paths** (inspected from `joedwards32/cs2`, `STEAMAPPDIR=/home/steam/cs2-dedicated`):

| What | Path |
|---|---|
| Game directory | `/home/steam/cs2-dedicated/game/csgo` |
| `gameinfo.gi` | `/home/steam/cs2-dedicated/game/csgo/gameinfo.gi` |
| Addons root | `/home/steam/cs2-dedicated/game/csgo/addons/` |
| Metamod dir | `/home/steam/cs2-dedicated/game/csgo/addons/metamod/` |
| Plugin binary | `addons/s2script/bin/linuxsteamrt64/s2script.so` |
| VDF `file` key | `addons/s2script/bin/linuxsteamrt64/s2script` |

The `file` key has no extension. Metamod resolves it in `MetamodSource::GetFullPluginPath`
(`third_party/metamod-source/core/metamod.cpp`): on Linux x86_64 it first tries `<file>.x64.so`;
if that file does not exist it falls back to `<file>.so`. Because we ship `s2script.so` (not
`s2script.x64.so`), the plugin loads via the `.so` fallback — the `.x64.so` probe is benign.

### Prerequisites

**1. Build and package (if not done already):**
```bash
make core && make shim && make package
```

**2. Install Metamod:Source 2.0 into `docker/metamod/`:**

Download the latest CS2-compatible MM:S build from
<https://www.sourcemm.net/downloads.php?branch=dev> (the "dev" branch is the Source 2 / CS2
build). Extract and copy the contents of its `csgo/addons/metamod/` directory to `docker/metamod/`:

```bash
tar xzf metamod_*.tar.gz
cp -r package/csgo/addons/metamod/* docker/metamod/
# docker/metamod/ should now contain: metamod.vdf  bin/  (etc.)
```

### Bring up the server

**Step 1 — Start the server** (first run downloads CS2, ~30 GB):
```bash
docker compose -f docker/docker-compose.yml up -d
docker logs -f s2script-cs2    # watch for "Starting CS2 Dedicated Server"
```

**Step 2 — Patch `gameinfo.gi`** (run once after first download):
```bash
docker exec s2script-cs2 /patch-gameinfo.sh
```

This inserts `Game    csgo/addons/metamod` as the first SearchPath entry. The script is idempotent.

**Step 3 — Restart so the engine re-reads `gameinfo.gi`:**
```bash
docker compose -f docker/docker-compose.yml restart cs2
docker logs -f s2script-cs2
```

**Step 4 — Connect via RCON and run the live gate checks:**
```bash
rcon -a 127.0.0.1:27015 -p s2script meta list
rcon -a 127.0.0.1:27015 -p s2script meta unload s2script
rcon -a 127.0.0.1:27015 -p s2script meta load  addons/s2script/bin/linuxsteamrt64/s2script
```

Or attach to the server console directly:
```bash
docker attach s2script-cs2
```
Then type at the `>` prompt: `meta list`, `meta unload s2script`, `meta load addons/s2script/bin/linuxsteamrt64/s2script`.

### Expected console output

Lines expected during Step 3 startup (order within the block is deterministic; exact interface
results depend on the live binary version):

```
[s2script] interface OK: Source2Server (Source2Server001)
[s2script] interface OK: EngineCvar (VEngineCvar007)
[s2script] interface OK: NetworkServerService (NetworkServerService_001)
[s2script] NOTE: SchemaSystem acquisition deferred — schemasystem module factory not yet wired
[s2script] Load(): initializing V8 core
[s2script] hello from V8 in CS2
```

If a version string in `gamedata/core.gamedata.jsonc` does not match the live binary, the
corresponding line reads `WARN: interface MISSING: <name> (<version>)` — this is non-fatal;
V8 still boots. Fix the version string in the gamedata file (confirm with `meta interfaces`)
and reload.

`meta list` should show:
```
Listing 1 plugin:
  [01] s2script (0.0.0-slice0)  by <author>
```

`meta unload s2script` should show:
```
[s2script] Unload(): shutting down V8 core
```
and the server must **not** crash.

`meta load addons/s2script/bin/linuxsteamrt64/s2script` should reprint the full startup block:
```
[s2script] interface OK: Source2Server (Source2Server001)
...
[s2script] hello from V8 in CS2
```
**without a server restart.** This is the sharpest check of the §5 platform-persistence posture
(see [Known findings](#known-findings--constraints)).

**Degradation sub-test (interface version string):** to verify non-fatal degradation, temporarily
change one version string in `gamedata/core.gamedata.jsonc` to a deliberately wrong value
(e.g. `"Source2Server": "Source2Server_BAD"`), rebuild, and remount. The startup log should show
`WARN: interface MISSING: Source2Server (Source2Server_BAD)` for that interface but still print
`hello from V8 in CS2` — confirming that a broken interface string never crashes or silences V8.
Restore the correct string when done.

---

## Acceptance checklist

Operator-run live gate. The "operator confirms" column is left unchecked; fill it in when executing the Docker runbook above.

| # | Criterion | Expected result | Operator confirms |
|---|---|---|---|
| 1 | Builds for Linux x86-64 | `make core && make shim && make package` produces `s2script.so` + `libs2script_core.so` + gamedata in `dist/addons/`; `make check-boundary` prints `core boundary OK` | [ ] |
| 2 | Loads on live CS2; `meta list` shows it with a version; `meta unload` no crash | `meta list` shows `s2script (0.0.0-slice0)`; `meta unload` prints `[s2script] Unload(): shutting down V8 core` and the server keeps running | [ ] |
| 3 | Per-interface acquisition logged; missing interface = named non-fatal warning | Startup log shows `interface OK: <name>` per acquired interface; a deliberately wrong version string produces `WARN: interface MISSING: <name>` and V8 still boots | [ ] |
| 4 | V8 embedded; `console.log` → server console | `[s2script] hello from V8 in CS2` appears in the server console during load | [ ] |
| 5 | Clean teardown; subsequent `meta load` reprints hello without server restart | After `meta unload`, `meta load` reprints the full startup block including `hello from V8 in CS2` — **no restart required.** This is the sharpest validation of the §5 resident-cdylib + platform-once posture | [ ] |
| 6 | Reproduces from this README | A clean checkout following this README end-to-end reaches criterion 5 with no undocumented steps | [ ] |

---

## Known findings / constraints

**V8 local-exec TLS and the `v8 = 149.4.0` pin.** The stock V8 prebuilt for v150+ uses local-exec
TLS (`R_X86_64_TPOFF32`), which the linker rejects when building a `-shared` object (`cannot be
used with -shared`). The v149.4.0 prebuilt was built with `v8_monolithic_for_shared_library=true`
and links cleanly into our cdylib. To advance past v149.4.0 without building V8 from source:
watch for a prebuilt that restores the `monolithic_for_shared_library` flag, or build from source
with `V8_FROM_SOURCE=1 GN_ARGS=v8_monolithic_for_shared_library=true cargo build`.

**§5 resident-cdylib + platform-once + reload safety.** `libs2script_core.so` is built with
`-Wl,-z,nodelete` so it stays mapped for the process lifetime even when the C++ shim (`s2script.so`)
is unloaded by Metamod. `s2script_core_init` initializes the V8 `Platform` exactly once (guarded);
`s2script_core_shutdown` disposes only the `Isolate + Context`, never the platform. A `meta load`
after `meta unload` creates a fresh Isolate on the still-live resident platform. Criterion 5 in
the acceptance checklist validates this posture against Metamod's actual `dlclose`/`dlopen`
semantics on the live server — if Metamod's behavior diverges from this model, **that finding is
itself a primary deliverable** of this slice and must be documented here before Slice 1 begins.

**SchemaSystem acquisition is deferred.** `SchemaSystem` is obtained via the `schemasystem` module
factory (a separate `dlopen`/`GetProcAddress` into `schemasystem.so`), not via the standard
`GetEngineFactory` / `GetServerFactory` path. Wiring this requires module-loading helpers not yet
available. The startup log prints `[s2script] NOTE: SchemaSystem acquisition deferred` — this is
expected and non-fatal.

**Interface version strings are best-effort data, not code.** The strings in `gamedata/core.gamedata.jsonc`
were confirmed against the live CS2 binary at the time of writing but will drift as Valve ships
updates. On any live-gate failure showing `WARN: interface MISSING`, run `meta interfaces` on the
server to see the actual strings, update `gamedata/core.gamedata.jsonc`, and rebuild — never
hardcode a version string in C++ or Rust.

**Gamedata path assumes `csgo/` as the server working directory.** The shim loads
`addons/s2script/gamedata/core.gamedata.jsonc` as a path relative to the game root (`csgo/`).
This is the standard CS2 dedicated server layout. If the cwd differs, the gamedata load will fail
with a named warning and interface acquisition is skipped — V8 still boots.
