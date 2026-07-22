# Building s2script from source

Everything a contributor needs: build the runtime, run the gate suite, work with the vendored
SDKs, and drive the Docker live gate.

If you only want to *run* s2script on a server, you do not need this file — grab a
[release zip](https://github.com/s2script/s2script/releases) and follow
[`INSTALL.md`](INSTALL.md).

> **Read this first.** A build made on your host machine will **not load on a real CS2 server**.
> The host build is for local development and tests only; anything you deploy must come from the
> [sniper build](#building-for-the-server-steam-runtime--glibc-231).

---

## Repository layout

```
core/         Rust engine core (cdylib, embeds V8). Engine-generic — never imports games/*.
shim/         C++ Metamod plugin. Owns every Source 2 touchpoint: sigscan, SourceHooks,
              detours, protobuf reflection, vtable RTTI.
games/cs2/    CS2 game-package prelude (generated schema + nav accessors).
packages/     npm-published: @s2script/sdk (types + the `s2s` CLI), @s2script/cs2, eslint-plugin.
plugins/      The base-plugin suite; plugins/disabled/ holds the opt-in ones.
examples/     Worked examples (not shipped) — see README.md.
tools/        Dev/treadmill tooling: schema-dump, s2bench, crash-test (not shipped).
gamedata/     Regenerable engine facts: signatures, offsets, schema/event/item catalogs.
scripts/      Build, gate (check-*.sh), sniper build, rcon.py, package/release.
docker/       CS2 dev server + database sidecars.
third_party/  Vendored hl2sdk + Metamod:Source submodules (pinned, patch-capable).
```

---

## Prerequisites

- **Linux x86-64** — the only supported target. Windows is not supported.
- **clang / clang++** — any recent version (used by the CMake shim build)
- **cmake ≥ 3.20**
- **cargo / rustc** — stable
- **Node ≥ 22.14.0** — for the SDK/CLI and plugin builds
- **docker** — for the sniper build, the live gate, and `make ci-js` (`scripts/test-gate.sh` asserts on the compose files)

---

## Build

```bash
git clone https://github.com/s2script/s2script.git
cd s2script
git submodule update --init --recursive   # vendored hl2sdk + Metamod:Source

make all      # = core + shim + package
```

The individual targets:

| Target | What it does |
|---|---|
| `make core` | `cargo build --release` → `target/release/libs2script_core.so` (Rust cdylib, embeds V8). **First run downloads the V8 prebuilt, ~130 MB**; later runs are fast. |
| `make shim` | `cmake -S shim -B build/shim` + build → `build/shim/s2script.so` (the Metamod plugin) |
| `make package` | `scripts/package-addon.sh` → assembles `dist/addons/` |
| `make clean` | `cargo clean` + removes `build/` and `dist/` |

After `make package`:

```
dist/addons/
  metamod/
    s2script.vdf
  s2script/
    bin/linuxsteamrt64/
      libs2script_core.so
      s2script.so
    gamedata/
      core.gamedata.jsonc
    js/
      pawn.js
    plugins/            # base .s2sp plugins (release) / drop zone
    configs/            # empty — must be writable at runtime
    data/               # empty — must be writable at runtime
```

### Tests

```bash
cargo test -p s2script-core        # core unit + in-isolate suite
```

Single-threading is already forced by `RUST_TEST_THREADS = "1"` in `.cargo/config.toml` — the
in-isolate frame tests share process-global capture buffers, so **do not** pass `--test-threads`
yourself.

---

## The gate suite

Run it before every PR. These are exactly the two scripts CI runs — local green means CI green.

```bash
make ci           # both suites
make ci-native    # scripts/ci-native.sh — boundary + nameleak + sigscan + licenses, cargo build/test, shim
make ci-js        # scripts/ci-js.sh — codegen freshness, plugin typecheck, activity/antiflood/gate tests
```

`.github/workflows/ci-native.yml` and `ci-js.yml` each run one of those two scripts and nothing
else, so a new gate is added to the script, never to the workflow YAML. `npm ci` (the
`package-lock.json` drift guard) is CI-only — run `CI=1 make ci-js` to include it.
`make check-boundary` still runs the core→games boundary check on its own.

The boundary checks are the load-bearing ones: the core is engine-generic and must never learn a
CS2 name. Dependencies point one way — game → core, never core → game.

---

## Building for the server (Steam Runtime / glibc 2.31)

The CS2 dedicated server runs under **Steam Runtime 3 "sniper"** (Debian 11, glibc 2.31). Binaries
built on a modern host link against `GLIBC_2.34`+, and Metamod refuses to load them:

```
version 'GLIBC_2.32' not found ... [META] Loaded 0 plugins
```

Build inside a matching-glibc container instead:

```bash
docker run --rm -v "$PWD:/repo" -w /repo \
  -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```

`scripts/build-sniper.sh` installs g++/cmake, rebuilds `core` + `shim`, repackages `dist/`, and
prints the resulting GLIBC requirement (must be ≤ 2.31 — currently `s2script.so` needs only
`GLIBC_2.14` and `libs2script_core.so` `GLIBC_2.30`). The named cargo volume avoids re-downloading
the V8 prebuilt on every run.

**This is the canonical build for anything that touches a real server.**

---

## Building plugins

```bash
npx @s2script/sdk build <dir>      # from a plugin dir → dist/<id>.s2sp
./scripts/build-base-plugins.sh    # every first-party plugin under plugins/
```

`build-base-plugins.sh` builds `plugins/*` and `plugins/disabled/*` (examples/ and tools/ are not
packaged). Pass `VERSION=0.1.2` (or `$1`) to stamp every plugin's `package.json` to a release tag
before building, so the `.s2sp` manifests match the runtime zip.

---

## Docker live gate

The live gate runs the addon on a real CS2 dedicated server (`joedwards32/cs2`). It is **not
automated** — a human drives it and records the result.

> The first `up` triggers a ~30 GB CS2 download.

### One-time setup

Install Metamod:Source 2.0 into `docker/metamod/`. Download the CS2-compatible build from
<https://www.sourcemm.net/downloads.php?branch=dev> and copy its `csgo/addons/metamod/` contents:

```bash
tar xzf metamod_*.tar.gz
cp -r package/csgo/addons/metamod/* docker/metamod/
# docker/metamod/ should now hold metamod.vdf, bin/, …
```

### Run it

```bash
make docker-test                          # docker compose -f docker/docker-compose.yml up
docker logs -f s2script-cs2

python3 scripts/rcon.py "meta list"       # 127.0.0.1:27015, password s2script
python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1"
```

A hibernating LAN server barely fires `GameFrame`, so get a map ticking before you expect any
per-frame behaviour.

To pick up a rebuilt addon:

```bash
docker compose -f docker/docker-compose.yml restart cs2
```

Use `restart` — **not** `--force-recreate`, which resets `gameinfo.gi`.

`docker/pre.sh` re-runs `patch-gameinfo.sh` on every boot, so the Metamod SearchPath self-heals
even on a first boot. To re-apply it by hand (idempotent):

```bash
docker exec s2script-cs2 /patch-gameinfo.sh
```

### A second server per worktree

Any linked worktree can run its own server alongside the primary, sharing the one ~74 GB install:

```bash
cd ~/projects/s2script-<slice>
scripts/package-addon.sh              # build this worktree's dist/addons/s2script first
scripts/gate.sh up                    # its own container + a port in 27016-27030
python3 scripts/rcon.py --port <N> "meta list"
scripts/gate.sh down                  # stop, keep the clone
scripts/gate.sh destroy               # stop and delete the clone
```

`gate.sh up` reflink-clones `docker/cs2-data` into the worktree's gitignored `.gate/` — sub-second
and effectively zero real disk on btrfs. Each instance is a *full independent* install, which is
why two servers never corrupt each other. (`du` reports ~74 GB per clone; that is reflinked extents
counted per file. `df` is the truth.)

Point a gate at some other addon build:

```bash
scripts/gate.sh up --addons   <dir>   # <dir> holds metamod/ + s2script/ (e.g. an unpacked release zip)
scripts/gate.sh up --s2script <dir>   # <dir> IS the s2script folder
```

Knobs `gate.sh` does not write to `gate.env` — set them in the environment, which takes precedence
over `--env-file`:

```
GATE_MAXPLAYERS=<n>      # default 12
GATE_STARTMAP=<map>      # default de_inferno
GATE_DAMAGE_SELFTEST=1   # default 0 — NOTE: the primary defaults to 1
```

**Update day.** The primary updates `docker/cs2-data` on boot. Instance clones do not follow it,
and letting an instance update itself costs that clone real disk. So: update the primary, then
`gate.sh destroy` + `gate.sh up` each instance — re-cloning is sub-second, updating in place is not.

---

## Vendored SDKs

Two upstream SDKs are pinned git submodules under `third_party/`:

| Submodule | Remote | Branch |
|---|---|---|
| `third_party/hl2sdk` | https://github.com/alliedmodders/hl2sdk | `cs2` |
| `third_party/metamod-source` | https://github.com/alliedmodders/metamod-source | `master` |

(metamod-source has no `dev` branch; `master` is where development happens.) The pinned SHAs are
whatever the submodule pointers say — `git submodule status` is the source of truth.

hl2sdk is a **patch-capable** dependency, not a trusted one: it lags Valve, so own your
schema/offset layer rather than believing the SDK's game-class fields.

### Bumping a submodule

```bash
git -C third_party/hl2sdk fetch
git -C third_party/hl2sdk checkout <newsha>
git add third_party/hl2sdk
git commit -m "chore: bump hl2sdk to <newsha>"
```

### Patching hl2sdk

- Make the change directly in `third_party/hl2sdk`.
- Export it: `git -C third_party/hl2sdk diff HEAD > patches/hl2sdk/NNNN-description.patch`
  (use `diff HEAD` — plain `diff` silently drops staged hunks).
- Patches in `patches/hl2sdk/` are re-applied in order on a fresh checkout.
- Every patch is reviewed and tracked in the update-day fire drill.

**Interface version strings live in `gamedata/core.gamedata.jsonc`**, never hardcoded in C++ or
Rust. When a game update changes one, fix the gamedata file — and confirm the new value with
`meta interfaces` on the live server, which is ground truth over the SDK headers. See
[`re-strategy.md`](re-strategy.md) for the full doctrine.

---

## Releasing

Two independent trains:

```bash
./scripts/package-release.sh <version>    # runtime zip (binaries + base plugins), after a sniper build
npm run changeset                         # @s2script/* npm packages (types + CLI)
npm run version-packages
npm run release
```

- **Runtime zip + base plugins** ride `git tag v*`; plugins are stamped to that tag at build time.
- **`@s2script/*` npm packages** publish independently via Changesets when `packages/` change.
  Pick only the packages that actually changed (see [`.changeset/README.md`](../.changeset/README.md)).
  Publishing uses **npm trusted publishing (OIDC)** from
  [`.github/workflows/changesets.yml`](../.github/workflows/changesets.yml) — there is no `NPM_TOKEN`.

Plugin-only work needs no changeset; package-only work needs no tag.

---

## Constraints & gotchas

**The host build is dev-only.** Covered above, and worth repeating because it is the single most
common way to lose an afternoon: host-built binaries fail to load on the server with a `GLIBC_2.3x
not found` from Metamod and `[META] Loaded 0 plugins`. Use the sniper build for anything deployed.

**The V8 `149.4.0` pin.** The stock V8 prebuilt for v150+ uses local-exec TLS
(`R_X86_64_TPOFF32`), which the linker rejects when building a `-shared` object (`cannot be used
with -shared`). The `149.4.0` prebuilt was built with `v8_monolithic_for_shared_library=true` and
links cleanly into our cdylib. To move past it: wait for a prebuilt that restores that flag, or
build from source with
`V8_FROM_SOURCE=1 GN_ARGS=v8_monolithic_for_shared_library=true cargo build`.

**Resident cdylib + platform-once.** `libs2script_core.so` is linked with `-Wl,-z,nodelete` so it
stays mapped for the process lifetime even when Metamod unloads the C++ shim.
`s2script_core_init` initializes the V8 `Platform` exactly once (guarded); `s2script_core_shutdown`
disposes only the Isolate + Context, never the platform. That is what makes a `meta load` after a
`meta unload` create a fresh isolate on the still-live platform instead of crashing.

**`configs/` and `data/` must be writable.** The host auto-generates plugin config JSON and creates
SQLite files there. Under a fully read-only addon tree the write fails degrade-safe (defaults only,
no crash), but the config file is never generated. In the Docker gate the addon is bind-mounted
`:ro` so the host owns the plugins dir, which means `configs/` needs a nested read-write mount
layered over the `:ro` parent (Docker resolves nested mounts by longest target path).

**Every CS2 update wipes `gameinfo.gi`.** The Metamod SearchPath entry is dropped and the addon
loads zero plugins until it is re-patched. In Docker this self-heals via `docker/pre.sh`; on a real
server it is a manual step — see [`INSTALL.md`](INSTALL.md).
