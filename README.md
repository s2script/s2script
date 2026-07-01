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
git clone https://github.com/GabeHirakawa/s2script.git
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

> ⚠️ The host `make` build above is **dev-only**. Binaries built on a modern host (newer glibc) will
> NOT load on the CS2 server — see [Building for the server](#building-for-the-server-steam-runtime--glibc-231).

---

## Building for the server (Steam Runtime / glibc 2.31)

The CS2 dedicated server runs under **Steam Runtime 3 "sniper" (Debian 11, glibc 2.31)**. Binaries
built on a newer host link against `GLIBC_2.34`+ and Metamod refuses to load them
(`version 'GLIBC_2.32' not found`). Build inside a matching-glibc container instead:

```bash
docker run --rm -v "$PWD:/repo" -w /repo \
  -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```

`scripts/build-sniper.sh` installs g++/cmake, rebuilds `core` + `shim`, repackages `dist/`, and prints
the resulting GLIBC requirement (must be ≤ 2.31). The cargo cache volume avoids re-downloading the V8
prebuilt on repeat runs. This is the canonical build for anything deployed to a real server.

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
corresponding line reads `[s2script] WARN: interface MISSING: <name> (<version>)` — this is non-fatal;
V8 still boots. Fix the version string in the gamedata file (confirm with `meta interfaces`)
and reload.

`meta list` should show:
```
Listing 1 plugin:
  [01] s2script (0.0.0-slice1)  by s2script
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
`[s2script] WARN: interface MISSING: Source2Server (Source2Server_BAD)` for that interface but still print
`hello from V8 in CS2` — confirming that a broken interface string never crashes or silences V8.
Restore the correct string when done.

---

## Acceptance checklist

**✅ All six criteria verified on a live CS2 dedicated server (`joedwards32/cs2`).** The plugin was
built against the Steam Runtime (see [Building for the server](#building-for-the-server-steam-runtime--glibc-231)),
loaded via Metamod with CounterStrikeSharp removed, and driven over RCON.

| # | Criterion | Live result |
|---|---|---|
| 1 | Builds for Linux x86-64 | ✅ `make check-boundary` → `core boundary OK`; **sniper build** (`scripts/build-sniper.sh`) produces server-loadable `s2script.so` (GLIBC_2.14) + `libs2script_core.so` (GLIBC_2.30) |
| 2 | Loads on live CS2; `meta list` shows it; `meta unload` no crash | ✅ `meta list` → `[02] s2script (0.0.0-slice1) by s2script`; `meta unload` → `[s2script] Unload(): shutting down V8 core`, server stays up |
| 3 | Per-interface acquisition logged; missing = named warn | ✅ `interface OK: Source2Server (Source2Server001)`, `EngineCvar (VEngineCvar007)`, `NetworkServerService (NetworkServerService_001)`; `SchemaSystem` deferred NOTE; missing-gamedata → `WARN`, V8 still boots (degrade proven) |
| 4 | V8 embedded; `console.log` → server console | ✅ `[s2script] hello from V8 in CS2` printed to the server console on load |
| 5 | Clean teardown; `meta load` reprints hello without restart | ✅ `meta unload` → `meta load` reprinted `hello from V8 in CS2` on a fresh isolate; **server never restarted, never crashed** — the §5 resident-cdylib + platform-once posture validated against Metamod's real `dlclose`/`dlopen` |
| 6 | Reproduces from this README | ✅ Build → package → docker runbook → RCON `meta` checks all reproduce |

---

## OnGameFrame multiplexer (Slice 1)

Slice 1 adds the generic hook multiplexer (`core/src/multiplexer.rs` — priority ladder
`High<Normal<Low<Monitor`, `HookResult` collapse `Continue<Changed<Handled<Stop`, Pre/Post,
snapshot re-entrancy, error isolation/auto-disable, lazy detour) bound to one engine touchpoint,
`ISource2Server::GameFrame`, via a SourceHook detour installed lazily on first subscription.

The full contract (Stop short-circuit, Monitor-after, re-entrancy snapshot, auto-disable, lazy
install/remove) is proven in `cargo test -p s2script-core -- --test-threads=1` (the V8-free
`multiplexer` suite + the `frame_tests` V8-integration suite, including a re-entrancy test where a
JS handler subscribes mid-dispatch without panicking).

**Live demonstration.** `Load()` evals a baked-in demo that subscribes two `onGameFrame` handlers at
different priorities (a Slice-1 stand-in for the future `import { onGameFrame } from "@s2script/events"`;
removed when real plugin loading lands in Slice 4). On a live CS2 server the console shows the detour
installing on first subscribe and dispatch firing every tick with priority-ordered composition:

```
[s2script] [demo] subscribed 2 OnGameFrame handlers; HIGH should log before low each frame
[s2script] [demo] HIGH tick=0    firstTick=true
[s2script] [demo] low
[s2script] [demo] HIGH tick=256  firstTick=true
[s2script] [demo] low
[s2script] [demo] HIGH tick=512  firstTick=true
[s2script] [demo] low
```

`HIGH` logs before `low` every frame (priority composition); the tick counter advances (the detour
fires each frame); the server never crashes. `firstTick=true` every frame is correct — a dedicated
server simulates one tick per `GameFrame`, so each frame is both the first and last tick of its batch.

**Slice 1 acceptance (live + cargo):**

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (multiplexer + V8 integration) + sniper build | ✅ 17 core tests pass; `make check-boundary` green; sniper `s2script.so` GLIBC_2.14 |
| detour | installs on first subscription, fires per tick | ✅ live (`request_hook("OnGameFrame",1)` → `SH_ADD_HOOK`; ticks advance) |
| compose | two handlers compose, priority order | ✅ live (`HIGH` before `low` each frame) |
| contract | Stop short-circuit / Monitor-after / re-entrancy / auto-disable / lazy remove | ✅ cargo (`multiplexer` + `frame_tests`, incl. re-entrancy) |
| gamedata | `dladdr` path fix — interfaces resolve without cwd workaround | ✅ live (cwd-path gamedata removed; `interface OK:` lines still appear) |

---

## Tick-integrated async (Slice 2)

Slice 2 owns the V8 microtask checkpoint so `await` resolves at controlled **frame boundaries** and
never preempts mid-tick. The isolate runs with `MicrotasksPolicy::Explicit`; once per frame, on the
Post `GameFrame`, `frame_async_drain` resolves due timers + completed threadpool jobs and then runs
the single `perform_microtask_checkpoint` — the one point where `await`/`.then` continuations execute.
It adds the provisional globals `Delay(ms)` / `NextTick()` / `NextFrame()` (cooperative timers that
never block a thread) and `threadSleep(ms)` (a demo op that runs genuinely blocking work on a fixed
4-worker pool and marshals the result back as a resolved Promise on a later drain). The combined
lazy-detour keeps `GameFrame` installed while `(onGameFrame subscribers > 0) OR (async pending > 0)`,
so an `await Delay(...)` with no frame subscriber still drives the drain. All engine-generic → `core`;
the C++ shim and the C ABI are unchanged (only the baked-in demo string). These globals are provisional
(the typed `@s2script/std` async API is Slice 5), like Slice 1's `onGameFrame`.

The full contract (kExplicit defers microtasks to the drain; `Delay` at/after its deadline;
`NextTick`/`NextFrame` at the expected drain; the cross-thread marshal; non-blocking `await`; the
combined lazy-detour; the re-entrancy discipline where a resolved continuation re-enters the timer
primitives mid-checkpoint; the process-global pool's job accounting across `shutdown`/re-init) is
proven in `cargo test -p s2script-core -- --test-threads=1` (the V8-free `async_rt` unit suite + the
`frame_tests` V8-integration suite).

**Live demonstration.** The baked-in demo arms after 128 live frames (past the boot window, where the
server barely ticks), then runs `await Delay(1000)` followed by `await threadSleep(50)`. A
monitor-priority handler counts frames so the post-`Delay` log proves the tick advanced throughout the
await:

```
GC Connection established for server version 2000848, instance idx 1
[s2script] [async] before Delay(1000) at frame 128
[s2script] [async] after Delay(1000); frames elapsed ~64 (tick was NOT blocked)
[s2script] [async] after threadSleep(50) - resumed on the main thread
```

The frame counter advanced ~64 (≈ tickrate) during the 1-second `await` — proving `await Delay(1000)`
does **not** block the tick — and the off-thread `threadSleep` continuation resumed on the main thread.
The Slice-1 `HIGH`-before-`low` composition still fires each tick and the server never crashes.

**Slice 2 acceptance (live + cargo):**

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (async_rt + V8 integration) + sniper build | ✅ 30 core tests pass; `make check-boundary` green; sniper GLIBC ≤ 2.30 |
| policy | explicit microtask policy; continuations only at the drain | ✅ cargo (`microtasks_do_not_run_until_frame_drain`) |
| timers | `Delay` at/after deadline; `NextTick`/`NextFrame` at expected drain | ✅ cargo (`delay_resolves_only_after_its_deadline`, `next_frame_resolves_one_frame_later`) |
| marshal | off-thread op resolves on a later frame drain | ✅ live (`threadSleep(50)` resumed on main) + cargo (`thread_sleep_runs_off_thread_and_resolves_on_a_drain`) |
| non-block | `await Delay(1000)` does not block the tick | ✅ live (frames elapsed ~64 during the 1 s await) |
| detour | `GameFrame` stays installed while async pending, removed when both counts reach zero | ✅ cargo (install: `delay_with_no_onframe_subscriber_still_requests_detour_install`; remove: `async_completion_removes_detour_when_pending_reaches_zero`) |

---

## Schema-backed typed accessor (Slice 3)

Slice 3 is the **first crossing of the engine-generic/per-game boundary**: it resolves one field —
`CCSPlayerPawn::m_iHealth` — from the live Source 2 SchemaSystem in-process and exposes `pawn.health`
get/set, with the network state-change folded into the setter. Core gains only **engine-generic**
Source 2 machinery with **zero CS2 names in Rust**; all the CS2 knowledge lives in `@s2script/cs2` (JS).

**Where each thing lives (the boundary):**
- **Core (`core/`, engine-generic):** the V8 natives `__s2_schema_offset` / `__s2_entity_by_index` /
  `__s2_deref_handle` / `__s2_ent_read_i32` / `__s2_ent_write_i32` / `__s2_ent_state_changed`. The actual
  C++ engine calls (SchemaSystem virtuals, the entity chunk walk, `NetworkStateChanged`) live in the C++
  shim and are passed to core as C-ABI function pointers (an `S2EngineOps` table) — the same
  shim→core callback pattern as `logger`/`request_hook`. Core never names a CS2 class or field.
- **`@s2script/cs2` (`games/cs2/js/pawn.js`, JS):** the names (`CCSPlayerPawn`, `m_iHealth`,
  `CCSPlayerController`, `m_hPlayerPawn`), the `slot → controller → pawn` walk, and `class Pawn { get/set
  health }` (the setter writes **and** calls `__s2_ent_state_changed`). Loaded at boot via
  `s2script_core_load_cs2` (real plugin loading is Slice 4).

A CI gate (`scripts/check-core-boundary.sh` + `scripts/test-boundary-nameleak.sh`) fails the build if any
CS2 identifier appears in `core/`. **Layout is data:** the `m_iHealth` offset is resolved live from
SchemaSystem every run (never hardcoded); a Valve offset shift needs no code change. The one exception is
the `IGameResourceService → CGameEntitySystem*` byte offset, which lives in `gamedata/` (`offsets.GameEntitySystem`,
`0x50`/`80` on Linux) and is re-confirmed on the update treadmill.

**Live demonstration (auto readback gate).** The baked demo arms once the server is live-ticking, then
scans slots for the first player pawn and proves `pawn.health` get/set + the folded-in state-change by
reading the value straight back:

```
[s2script] interface OK: SchemaSystem (SchemaSystem_001)
[s2script] interface OK: GameResourceService (GameResourceServiceServerV001, entity-system offset=80 cached; resolved per-call)
[s2script] [cs2] slot=0 HEALTH_OFFSET=1456 health get=100
[s2script] [cs2] slot=0 health set=1234 readback=1234
```

The offset resolved live (`m_iHealth` at 1456, found by walking the schema base-class chain — it is
inherited, not a direct field of `CCSPlayerPawn`); `health` read the live pawn's value (100), the setter
wrote 1234 and called `NetworkStateChanged`, and the readback confirms the write — all on a real CS2
server, no crash, with Slices 1–2 still regressing. The engine-generic core stayed free of CS2 names
throughout.

**Update-treadmill note.** This slice's live gate landed during a real CS2 update (build 2000854), which
is the treadmill in miniature: the update reset `gameinfo.gi` (re-patch + restart), and two engine methods
the standard SDK paths use — `CEntitySystem::GetEntityIdentity` and `ConCommand::Create` — turned out to be
exported by **no** CS2 module. The handle deref was switched to a signature-free entity chunk walk (no
dependency on the unexported symbol), and every offset/schema failure **degraded per-descriptor** (a named
`WARN`, never a crash) while the layout facts were re-confirmed — exactly the posture the framework is built
for.

**Deferred to Slice 5.** A console-command-triggered *manual* HUD demo (e.g. `s2_sethp <hp>`) is deferred:
registering a Source 2 `ConCommand` needs `ConCommand::Create`, which no CS2 module exports, so
command registration belongs to the Slice-5 command framework. The auto gate above already proves the
accessor **and** the state-change server-side (the readback confirms the field changed and `NetworkStateChanged`
fired); the client-observed HUD lands with Slice 5.

**Slice 3 acceptance (live + cargo):**

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (schema cache + memory helpers + bridge) + sniper build | ✅ 37 core tests; both boundary gates green; sniper GLIBC ≤ 2.30 |
| boundary | zero CS2 identifiers in `core/`; accessor + names in `games/cs2/js/pawn.js` | ✅ cargo (`check-core-boundary.sh` + `test-boundary-nameleak.sh`) |
| live offset | `m_iHealth` resolved live from SchemaSystem (not hardcoded); missing field degrades named | ✅ live (`HEALTH_OFFSET=1456`, via base-class walk) + cargo (`OffsetCache` tests) |
| get/set | `pawn.health` reads a live pawn, writes a marker, state-change fires, readback confirms | ✅ live (`get=100`, `set=1234 readback=1234`) + cargo (`entity::read_i32`/`write_i32`) |
| degrade | wrong offset / null entity system / unresolved field degrades, never crashes | ✅ live (graceful WARNs throughout the update-day debugging; no crash) |
| manual HUD | console-command HUD (`s2_sethp`) | ⏸ deferred to Slice 5 (`ConCommand::Create` unexported — command framework) |

---

## Plugin lifecycle — one `.s2sp` that hot-reloads (Slice 4)

Slice 4 is the **milestone**: the whole architecture proven end-to-end on a thin thread. You author a
TypeScript plugin, `npx s2script build` it into a `.s2sp`, drop it into `addons/s2script/plugins/`, and it
loads into **its own V8 context** exercising Slices 1–3 — then hot-reloads on re-drop without a server
restart, and tears down cleanly on delete. This replaces the baked-in demos with real plugin loading.

**The runtime.** One shared V8 isolate hosts a registry of plugin instances, each with **its own
`v8::Context`**; the calling plugin is identified by the current context's `set_slot::<PluginId>` (not a
thread-local — correct across the microtask checkpoint). Every persistent effect (hook subscriptions,
timers, pending async) is auto-recorded in a per-plugin **ledger**, which is the teardown authority:
unload walks it in reverse at a frame boundary, then disposes the context. The **async-liveness guard**
tags each timer/job resolver `(plugin_id, generation)` and drops any continuation whose plugin was
unloaded or reloaded — a threadpool result completing after its plugin is gone never runs into a disposed
context (the use-after-free killer).

**Authoring & injection.** Plugins are TypeScript; `@s2script/cli` (`npx s2script build`) esbuild-bundles
them to a CJS `plugin.js` with `@s2script/*` marked **external** and derives a minimal `manifest.json`.
The runtime evals the bundle under a `(function(require, module, exports){…})` wrapper whose `require`
resolves `@s2script/std` (`OnGameFrame`, `delay`, `nextTick`, `nextFrame`, `threadSleep`, `console`) and
`@s2script/cs2` (`Pawn`) to the per-context injected API, and captures `module.exports` (`onLoad`/`onUnload`).
Core stays **engine-generic**: `@s2script/std` is built in-core over the natives, but the CS2 `@s2script/cs2`
JS is registered as **external data** (the shim reads the packaged `pawn.js` → `register_injected_package`),
never baked into the core binary — a boundary gate rejects `include_str!` of `games/`.

**Naming convention** (locked here): PascalCase events + types (`OnGameFrame`, `Pawn`), camelCase functions +
properties (`delay`, `nextTick`, `pawn.health`).

**Live demonstration.** `npx s2script build examples/demo-plugin` → drop the `.s2sp` into
`addons/s2script/plugins/`; edit + rebuild + re-drop to hot-reload; delete to tear down. On a live CS2 server:

```
[s2script] @s2script/cs2 registered (… from …/js/pawn.js)
[s2script] plugins dir: …/addons/s2script/plugins
[s2script] [demo] onLoad                       # dropped .s2sp loaded into its own context
[s2script] [demo] tick 1 hp=100                # OnGameFrame + Pawn.forSlot(0).health, per-context
[s2script] [demo] after delay(1000)            # await delay resolved
[s2script] [demo] onUnload                     # re-drop → old torn down (ledger) …
[s2script] [demo] onLoad                       #   … new loaded, fresh state, NO server restart
[s2script] [demo] onUnload                     # delete → clean teardown, then silence (no more ticks)
```

No restart, no crash; the reloaded instance starts fresh (old context disposed) and the deleted instance's
subscription stops firing. The whole spine — context-per-plugin, the ledger, async-liveness, the loader/watch,
the CLI-built `.s2sp`, and the externally-registered cs2 — proven end to end.

**Slice 4 acceptance (live + cargo):**

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (registry/ledger/liveness + loader + context/dispatch/drain) + the CLI test + sniper | ✅ 49 core tests; `@s2script/cli` test; both boundary gates green; sniper GLIBC ≤ 2.30 |
| build tool | `npx s2script build` turns a TS plugin into a loadable `.s2sp` (cjs + external + derived manifest) | ✅ cargo (`read_s2sp`) + the CLI round-trip test |
| load | a dropped `.s2sp` loads into its own context and runs Slices 1–3 (`OnGameFrame`, `await delay`, `Pawn.health`) | ✅ live (`[demo] onLoad` → ticks `hp=100` → `after delay`) |
| hot-reload | edit + rebuild + re-drop reloads with no server restart (old torn down, new active) | ✅ live (`onUnload` → `onLoad`, fresh state, same process) |
| teardown | delete tears down cleanly via the ledger (subscription gone, context disposed, no crash) | ✅ live (`onUnload`, no more ticks, server stable) |
| async-liveness | a continuation whose plugin unloaded/reloaded is dropped (no use-after-free) | ✅ cargo (`drain_drops_continuation_when_owner_no_longer_live`, `delay_continuation_for_unloaded_plugin_is_dropped`) |
| boundary | core stays engine-generic — cs2 JS is external data, not baked in (`include_str!(games/)` gated) | ✅ cargo (`check-core-boundary.sh` + name-leak/include_str gate) |

Deferred to later slices: the `tsc` typecheck gate; inter-plugin deps/proxies (Slice 4.5); the handle/`EntityRef`
system + reload state-handoff (Slice 5); config materialization + permissions enforcement.

---

## Two plugins talk — inter-plugin interfaces (Slice 4.5)

Slice 4.5 proves the **inter-plugin typed-interface contract** end to end. A producer plugin
`publishInterface(name, version, impl)`s a versioned interface (`@demo/greeter@1.0.0`) whose methods
become natives and whose `PublishHandle.emit` fans forwarded events out to subscribers; a consumer
`require("@demo/greeter")`s it as a **hard dependency** and gets a producer-backed **proxy** — it calls
`greet(0)` across the context boundary and subscribes to the `greeted` event. Method args and event
payloads cross by **structured copy** (never a live pointer or cross-context reference). When the
producer unloads, the proxy throws `InterfaceUnavailable` and the consumer **degrades** (its `try/catch`
logs and it keeps ticking — no crash); when the producer reloads, calls **recover with no consumer
reload**. Imports are auto-ledgered, so teardown resolves in reverse-dependency order.

The two demos live in [`examples/greeter-plugin`](examples/greeter-plugin) (producer) and
[`examples/greeter-consumer`](examples/greeter-consumer) (consumer). Because interface `.d.ts` codegen is
deferred, the consumer hand-writes an ambient `src/greeter.d.ts` declaring the producer's published shape
so `import greeter = require("@demo/greeter")` type-checks.

### Runbook

```bash
# Build the CLI + both plugins → .s2sp
node packages/cli/build.mjs
( cd packages/cli && npm link )                 # so `npx s2script` resolves to this repo's CLI
npx s2script build examples/greeter-plugin
npx s2script build examples/greeter-consumer

# Sniper build the runtime (GLIBC <= 2.31) and bring the server up
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
mkdir -p dist/addons/s2script/plugins
docker compose -f docker/docker-compose.yml up -d
# A CS2 update resets gameinfo.gi (metamod SearchPath dropped) — re-patch + restart if so:
docker exec s2script-cs2 /patch-gameinfo.sh
docker compose -f docker/docker-compose.yml restart cs2
# Get the map ticking (a hibernating LAN server barely fires GameFrame):
python3 scripts/rcon.py "bot_quota 1" "sv_hibernate_when_empty 0"

# --- The 5-step gate (host writes into the :ro-mounted plugins dir; the shim READS it) ---
# 1. Drop producer then consumer → publish + load + call + forwarded event
cp examples/greeter-plugin/dist/_demo_greeter.s2sp             dist/addons/s2script/plugins/
cp examples/greeter-consumer/dist/_demo_greeter-consumer.s2sp dist/addons/s2script/plugins/
# 2. Unload the producer → consumer degrades (InterfaceUnavailable), server keeps ticking
rm dist/addons/s2script/plugins/_demo_greeter.s2sp
# 3. Re-drop the producer → consumer's greet recovers, NO consumer reload
cp examples/greeter-plugin/dist/_demo_greeter.s2sp dist/addons/s2script/plugins/
# 4. Unload the consumer → clean teardown, producer runs on, no crash
rm dist/addons/s2script/plugins/_demo_greeter-consumer.s2sp
```

### Captured live log

Captured on a live `joedwards32/cs2` server (build 2000855; the run landed on the update treadmill —
the CS2 update reset `gameinfo.gi`, re-patched + restarted per the runbook):

```
# --- boot: s2script loads over Metamod and watches the plugins dir ---
[s2script] interface OK: Source2Server (Source2Server001)
[s2script] interface OK: SchemaSystem (SchemaSystem_001)
[s2script] @s2script/cs2 registered (2054 bytes from …/addons/s2script/js/pawn.js)
[s2script] plugins dir: …/game/csgo/addons/s2script/plugins
[META] Loaded 1 plugin.

# 1. drop greeter-plugin then greeter-consumer → publish, load, call, forwarded event
[s2script] [greeter] onLoad — publishing @demo/greeter@1.0.0
[s2script] [consumer] onLoad
[s2script] [consumer] greet -> hello, player 0
[s2script] [consumer] event greeted: slot=0 tick=1025
[s2script] [consumer] greet -> hello, player 0
[s2script] [consumer] event greeted: slot=0 tick=1281

# 2. delete greeter-plugin.s2sp (unload producer) → consumer degrades; server keeps ticking, no crash
[s2script] [greeter] onUnload
[s2script] [consumer] greet failed (degraded): Error: InterfaceUnavailable: @demo/greeter
[s2script] [consumer] greet failed (degraded): Error: InterfaceUnavailable: @demo/greeter

# 3. re-drop greeter-plugin (reload producer) → greet recovers with NO consumer reload
[s2script] [greeter] onLoad — publishing @demo/greeter@1.0.0
[s2script] [consumer] greet -> hello, player 0
[s2script] [consumer] greet -> hello, player 0

# 4. delete greeter-consumer.s2sp (unload consumer) → clean teardown, then silence; container stays Up
[s2script] [consumer] onUnload
```

The consumer's `greet -> hello, player 0` (a cross-context native call) and `event greeted: slot=0
tick=…` (a forwarded event) fire live; deleting the producer flips every subsequent `greet` to the
degraded `InterfaceUnavailable` path **without a crash** (`docker ps` shows `Up` throughout); re-dropping
the producer recovers `greet` with **no `[consumer] onLoad`** (the consumer never reloaded); deleting the
consumer prints `[consumer] onUnload` and then silence, producer still resident. The whole inter-plugin
spine — publish/require proxy, cross-context call + forwarded event, hard-dep degrade/recover, and
ledgered reverse-dependency teardown — proven end to end.

### Slice 4.5 acceptance (live + cargo)

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (publish/require/call/emit/teardown + loader dep maps) + CLI test + sniper | ✅ 74 core tests; `@s2script/cli` test; both boundary gates green; sniper GLIBC ≤ 2.31 |
| publish | producer `publishInterface(name, version, impl)` exposes methods-as-natives + a `PublishHandle` | ✅ live (`[greeter] onLoad — publishing @demo/greeter@1.0.0`) |
| call | consumer hard-dep proxy calls `greet(0)` cross-context (structured-copy args/return) | ✅ live (`[consumer] greet -> hello, player 0`) |
| event | producer `handle.emit("greeted", …)` forwards to the consumer's `on("greeted", …)` | ✅ live (`[consumer] event greeted: slot=0 tick=1025`) |
| degrade | producer-unload → hard-dep proxy throws `InterfaceUnavailable`; consumer `try/catch` degrades, no crash | ✅ live (`greet failed (degraded): Error: InterfaceUnavailable: @demo/greeter`; container `Up`) |
| recover | producer-reload → consumer's `greet` recovers with **no consumer reload** | ✅ live (`greet -> hello, player 0` returns; no `[consumer] onLoad`) |
| teardown | consumer-unload tears down cleanly via the ledger; producer runs on; no leak/crash | ✅ live (`[consumer] onUnload`, then silence; `docker ps` `Up`) |
| boundary | core stays engine-generic — no game names in the inter-plugin path | ✅ cargo (`check-core-boundary.sh` + name-leak gate) |

Deferred to later slices: interface `.d.ts` codegen (consumers hand-write the ambient `.d.ts` today);
the `tsc` typecheck gate; the handle/`EntityRef` system + schema codegen + `@s2script/std` breadth (Slice 5);
config materialization + permissions enforcement + reload state-handoff.

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
were confirmed against the live CS2 binary (`Source2Server001`, `VEngineCvar007`,
`NetworkServerService_001` all resolved `interface OK`) but will drift as Valve ships
updates. On any live-gate failure showing `[s2script] WARN: interface MISSING`, run `meta interfaces` on the
server to see the actual strings, update `gamedata/core.gamedata.jsonc`, and rebuild — never
hardcode a version string in C++ or Rust.

**Build target = Steam Runtime, not the host (live-gate finding).** Binaries built on a modern host
(e.g. Arch, glibc 2.43) require `GLIBC_2.34`/`2.38` and **fail to load** on the CS2 server, which runs
under **Steam Runtime 3 "sniper" (Debian glibc 2.31)** — Metamod reports
`version 'GLIBC_2.32' not found ... [META] Loaded 0 plugins`. Build inside a glibc-2.31 container
(`scripts/build-sniper.sh`, uses `rust:bullseye`) → `s2script.so` needs only `GLIBC_2.14` and
`libs2script_core.so` `GLIBC_2.30`, both ≤ 2.31. The `v8 = 149.4.0` prebuilt links fine at this
glibc. **The host `make` build is dev-only; distributable plugins must use the sniper build.**

**gamedata path must not resolve from cwd (live-gate finding, must-fix).** The shim currently reads
`addons/s2script/gamedata/core.gamedata.jsonc` relative to the process cwd, but the CS2 server's cwd is
`game/bin/linuxsteamrt64/` (the engine binary dir), **not** `game/csgo/`. So the file is missed and
interface acquisition is silently skipped (the degrade path: `WARN`, V8 still boots). Confirmed: placing
the gamedata at the cwd-relative path makes acquisition succeed. **Fix (Slice 1):** resolve the gamedata
path relative to the plugin's own `.so` location (`dladdr`/`/proc/self/maps`), independent of cwd.

**Gamedata path assumes `csgo/` as the server working directory.** The shim loads
`addons/s2script/gamedata/core.gamedata.jsonc` as a path relative to the game root (`csgo/`).
This is the standard CS2 dedicated server layout. If the cwd differs, the gamedata load will fail
with a named warning and interface acquisition is skipped — V8 still boots.
