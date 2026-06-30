# s2script

## Building the Rust core

```bash
cargo build --release          # builds libs2script_core.so (cdylib, V8 embedded)
cargo test -p s2script-core -- --test-threads=1
```

`--test-threads=1` is required: the V8 platform is process-global and
initialized exactly once, so parallel tests race that init.

The `v8` crate is pinned to **149.4.0** because its prebuilt binary was compiled
with `v8_monolithic_for_shared_library=true`, which is required to link V8 into a
`-shared` object (our `dlopen`'d Metamod plugin `.so`). The stock `v8 = 150.0.0`
prebuilt uses local-exec TLS and fails to link a cdylib with
`R_X86_64_TPOFF32 ... cannot be used with -shared`. To move to v150+, build from
source: `V8_FROM_SOURCE=1 GN_ARGS=v8_monolithic_for_shared_library=true cargo build`.

## Vendored SDKs (hl2sdk, Metamod:Source)

Two upstream SDKs are vendored as pinned git submodules under `third_party/`:

| Submodule | Remote | Branch | Pinned SHA |
|---|---|---|---|
| `third_party/hl2sdk` | https://github.com/alliedmodders/hl2sdk | `cs2` | `9ab16fa9fcdeeb30565dfdbf6fbb312356978a0b` |
| `third_party/metamod-source` | https://github.com/alliedmodders/metamod-source | `master` | `a5f4cca5824c0c5f13e8fa100dd15df164d2db22` |

Note: the upstream metamod-source repo has no `dev` branch; `master` is the active development branch.

### Fresh checkout

```bash
git submodule update --init --recursive
```

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

---

## Docker verification runbook

This runbook uses `joedwards32/cs2` to confirm the Metamod:Source plugin
(`build/shim/s2script.so`) loads and unloads cleanly on a real CS2 dedicated
server — without any V8 wiring (the shim is log-only at this stage).

**Confirmed image paths** (inspected from `joedwards32/cs2`, `STEAMAPPDIR=/home/steam/cs2-dedicated`):

| What | Path |
|---|---|
| Game directory | `/home/steam/cs2-dedicated/game/csgo` |
| `gameinfo.gi` | `/home/steam/cs2-dedicated/game/csgo/gameinfo.gi` |
| Addons root | `/home/steam/cs2-dedicated/game/csgo/addons/` |
| Metamod dir | `/home/steam/cs2-dedicated/game/csgo/addons/metamod/` |
| Plugin binary | `addons/s2script/bin/linuxsteamrt64/s2script.so` |
| VDF `file` key | `addons/s2script/bin/linuxsteamrt64/s2script` |

The `file` key has no extension. Metamod resolves it in
`MetamodSource::GetFullPluginPath` (`third_party/metamod-source/core/metamod.cpp`):
on Linux x86_64 it first tries `<file>.x64.so`; if that file does not exist on
disk it falls back to `<file>.so`. It does **not** append `/linuxsteamrt64` —
that subdirectory is already part of the VDF `file` value itself
(`addons/s2script/bin/linuxsteamrt64/s2script`). Because we ship
`s2script.so` (not `s2script.x64.so`), the plugin loads via the `.so` fallback;
MM:S may probe `s2script.x64.so` first and not find it — that is benign.

### Prerequisites

**1. Build the plugin and package the addon:**
```bash
make core        # cargo build --release -> target/release/libs2script_core.so
make shim        # cmake build/shim      -> build/shim/s2script.so
make package     # assembles dist/addons/
```

Expected `dist/addons/` tree:
```
dist/addons/
  metamod/
    s2script.vdf
  s2script/
    bin/
      linuxsteamrt64/
        libs2script_core.so
        s2script.so
    gamedata/               (empty until the interface-acquisition task, Task 7)
```

**2. Install Metamod:Source 2.0 into `docker/metamod/`:**

Download the latest CS2-compatible MM:S build from
<https://www.sourcemm.net/downloads.php?branch=dev> (the "dev" branch is
the Source 2 / CS2 build). Extract the package and copy the contents of
its `csgo/addons/metamod/` directory to `docker/metamod/`:
```bash
# Example: after downloading package.tar.gz
tar xzf metamod_*.tar.gz
cp -r package/csgo/addons/metamod/* docker/metamod/
# docker/metamod/ should now contain: metamod.vdf  bin/  (etc.)
```

### Bring up the server (operator-run live gate)

> **IMPORTANT:** The `docker compose up` below triggers a ~30 GB CS2 download on
> first run. This gate is **not automated** — a human operator must execute
> it and record the output. Autonomous offline validation (packaging, YAML
> parsing, gameinfo patch) was completed in Task 4; the live gate is deferred
> to a separate operator session.

**Step 1 — Start the server** (first run downloads CS2):
```bash
docker compose -f docker/docker-compose.yml up -d
docker logs -f s2script-cs2    # watch for "Starting CS2 Dedicated Server"
```

Wait until the first full start is complete (SteamCMD download finishes and
the server prints the startup banner).

**Step 2 — Patch `gameinfo.gi`** (run once after first download):
```bash
docker exec s2script-cs2 /patch-gameinfo.sh
```

This inserts `Game    csgo/addons/metamod` as the first SearchPath entry —
what metamod requires to be discovered by the Source 2 engine. The script is
idempotent; re-running it is safe.

**Step 3 — Restart so the engine re-reads `gameinfo.gi`:**
```bash
docker compose -f docker/docker-compose.yml restart cs2
docker logs -f s2script-cs2
```

**Step 4 — Connect via RCON and run the live gate checks:**
```bash
# Using the rcon tool (or any Source RCON client):
rcon -a 127.0.0.1:27015 -p s2script meta list
rcon -a 127.0.0.1:27015 -p s2script meta unload s2script
rcon -a 127.0.0.1:27015 -p s2script meta load  addons/s2script/bin/linuxsteamrt64/s2script
```

Or attach to the server console directly:
```bash
docker attach s2script-cs2
```
Then type at the `>` prompt:
```
meta list
meta unload s2script
meta load addons/s2script/bin/linuxsteamrt64/s2script
```

### Expected console output (current log-only shim)

The shim at this stage is **log-only** (V8 is not yet wired up). Expected
lines after Step 3 startup:

```
[s2script] Load(): boot handshake (no V8 yet)
```

`meta list` should show:
```
Listing 1 plugin:
  [01] s2script (0.0.0-slice0)  by <author>
```

`meta unload s2script` should show:
```
[s2script] Unload()
```
and the server must **not** crash.

`meta load addons/s2script/bin/linuxsteamrt64/s2script` should show:
```
[s2script] Load(): boot handshake (no V8 yet)
```
with no crash.

If any of these checks fail, the boot path is broken — fix before
proceeding to V8 integration (Task 6+).

### Gate summary

| Check | Expected | Confirmed |
|---|---|---|
| `meta list` shows plugin | `s2script 0.0.0-slice0` | operator |
| Load log line | `[s2script] Load(): boot handshake (no V8 yet)` | operator |
| `meta unload` — no crash | `[s2script] Unload()` | operator |
| `meta load` — hot reload | `[s2script] Load()...` again | operator |
