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
the `tsc` typecheck gate; full schema codegen (5B) + the engine-generic `@s2script/std` breadth (5C);
config materialization + permissions enforcement + reload state-handoff.

---

## Safe entities — the EntityRef guardrail (Slice 5A)

Slice 5A closes the Slice-3 **use-after-free**: back then `Pawn` held a raw entity pointer, so a `Pawn`
stashed across time would dereference freed memory the moment its entity was destroyed. Now every `Pawn`
holds an **`EntityRef` = `{index, serial}`** (no raw pointer ever crosses to JS), and **every field access
re-validates the captured serial against the engine's live `CEntityIdentity` before touching memory**. A
stale ref degrades safely to `null`/`false` — never garbage, never a crash.

**Where each thing lives (the boundary holds):**
- **Core (`core/`, engine-generic):** the serial-gated natives `__s2_ent_ref_valid` / `__s2_ent_ref_read_i32` /
  `__s2_ent_ref_write_i32` / `__s2_ent_ref_state_changed`, plus `__s2_ent_current_serial` and the
  `__s2_handle_decode` `CEntityHandle` bit-split (`entity.rs`). Each read/write resolves the entity pointer
  **and** compares the slot's current serial to the ref's captured serial in a single lookup (no TOCTOU); a
  mismatch returns `null`/`false`. `EntityRef` itself is the engine-generic `@s2script/std` class wrapping
  those natives. **Zero CS2 names in Rust** (both boundary gates green).
- **`@s2script/cs2` (`games/cs2/js/pawn.js`, JS):** `Pawn` now stores an `EntityRef` (+ the resolved health
  offset); `Pawn.forSlot(slot)` walks `slot → controller (index slot+1) → m_hPlayerPawn handle → pawn
  EntityRef`, decoding the controller's handle via `__s2_handle_decode` and capturing the pawn's live serial.
  `get health` is `ref.readInt32(off)` (→ `number | null`); `set health` writes then
  `ref.notifyStateChanged(off)` only if the write succeeded.

### Runbook (build → load → kill → respawn)

```bash
cd /home/gkh/projects/s2script
# 1. Build the demo .s2sp (stashes a Pawn, logs stashed vs fresh health every ~256 frames)
node packages/cli/build.mjs                 # (run from packages/cli/ if it can't resolve src/cli.ts)
npx s2script build examples/demo-plugin
# 2. Sniper-build the runtime (GLIBC <= 2.31) — MUST post-date the 5A commits (fresh EntityRef core)
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
# 3. Bring the server up; a CS2 update resets gameinfo.gi — re-patch + restart if the addon won't load
docker compose -f docker/docker-compose.yml up -d
docker exec s2script-cs2 /patch-gameinfo.sh && docker compose -f docker/docker-compose.yml restart cs2
python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1"   # get the map ticking + a bot in slot 0

# --- The 3-step host-invalidation gate (host writes into the :ro-mounted plugins dir) ---
cp examples/demo-plugin/dist/_demo_hello.s2sp dist/addons/s2script/plugins/   # 1. load → live pawn reads 100
python3 scripts/rcon.py "bot_quota 0" "bot_kick"                              # 2. REAL destruction → stashed → null
python3 scripts/rcon.py "bot_quota 1"                                         # 3. respawn → fresh reads 100 (new serial)
```

**Force a REAL destruction, not `mp_restartgame`.** The Task-1 spike proved `mp_restartgame` does *not*
destroy the pawn (serials persist), so it never exercises the null path. `bot_kick` destroys the controller
**and** the pawn; a re-added bot at the same index gets an **incremented serial**, so the stashed
`{index, serial}` fails the equality check. (Lethal damage / a natural round death also works — there the
controller persists and `fresh` recovers on respawn.)

### Captured live log

Captured on a live `joedwards32/cs2` server (build 2000856). `bot_kick` was used, so after the kill the
controller at index `slot+1` is also gone and `fresh` reads `none` until a bot re-joins:

```
# --- boot: fresh EntityRef core loads; the (larger) EntityRef-backed pawn.js registers ---
[s2script] interface OK: SchemaSystem (SchemaSystem_001)
[s2script] @s2script/cs2 registered (2737 bytes from …/addons/s2script/js/pawn.js)
[s2script] plugins dir: …/game/csgo/addons/s2script/plugins
[META] Loaded 1 plugin.

# 1. drop the .s2sp + a live bot in slot 0 → the STASHED pawn and a FRESH forSlot both read 100
[s2script] [demo] onLoad
[s2script] [demo] tick 1 stashed.health=100 fresh.health=100
[s2script] [demo] tick 257 stashed.health=100 fresh.health=100
[s2script] [demo] tick 769 stashed.health=100 fresh.health=100

# 2. bot_quota 0; bot_kick → the stashed pawn's entity is DESTROYED; the next tick reads null (serial
#    mismatch — NOT garbage, NOT a crash). fresh=none (the controller is gone too). server keeps ticking.
[s2script] [demo] tick 1025 stashed.health=null fresh.health=none
[s2script] [demo] tick 1281 stashed.health=none fresh.health=none     # demo re-stashes; no bot yet

# 3. bot_quota 1 → a bot re-joins slot 0 with an INCREMENTED serial; a fresh forSlot reads 100 again and
#    the demo re-stashes the new pawn (so stashed reads 100 too).
[s2script] [demo] tick 2561 stashed.health=100 fresh.health=100
[s2script] [demo] tick 2817 stashed.health=100 fresh.health=100
```

The stashed `Pawn` read `100` while its entity lived, flipped to `null` the tick its entity was destroyed
(the serial no longer matched the engine's `CEntityIdentity`), and the server **kept ticking** across the
destruction (`docker ps` showed `Up` throughout; no panic/segfault in the logs). A fresh `forSlot` recovered
with the new serial once a bot re-joined. The whole guardrail — `EntityRef` capture, per-access serial
validation, safe `null` degrade, host-invalidation across a real entity death — proven end to end.

**Live-gate finding (the gate earned its keep).** The first drop logged
`WARN: @s2script/cs2 prelude eval error: ReferenceError: require is not defined` and never ticked. The
Task-4 `pawn.js` prelude resolved `EntityRef` via bare `require("@s2script/std")`, but the game-package
prelude is evaluated in the raw context scope (**not** inside the plugin's
`(function(require,module,exports){…})` wrapper), so `require` is out of scope and `__s2pkg_cs2` never got
set — the demo's frame handler then threw every tick. Fixed by resolving through the `__s2require` native
(the same primitive the `@s2script/std` prelude itself uses). No unit test covered the cs2 prelude's
require-scope, so this was exactly the integration gap the live gate exists to catch. Core was untouched.

### Slice 5A acceptance (live + cargo)

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (entity decode/serial-resolve + in-isolate degrade) + both boundary gates + sniper | ✅ 81 core tests; `check-core-boundary` + name-leak gates green; sniper GLIBC ≤ 2.31 (s2script.so 2.14 / core 2.30) |
| load | the EntityRef-backed demo `.s2sp` loads; a live pawn reads via the stashed ref | ✅ live (`tick 1 stashed.health=100 fresh.health=100`) |
| invalidate | a REAL entity destruction (`bot_kick`) flips the stashed `Pawn.health` to `null` (serial mismatch), no crash | ✅ live (`tick 1025 stashed.health=null`; container `Up`, no panic/segfault) |
| keep-alive | the server keeps ticking through the destruction | ✅ live (frame counter advanced 769→1025→…→2561; `docker ps` `Up`) |
| recover | a fresh `forSlot` gets the new serial after respawn; the demo re-stashes | ✅ live (`tick 2561 stashed.health=100 fresh.health=100`) |
| boundary | raw pointer never crosses to JS; `EntityRef` engine-generic in `@s2script/std`; CS2 names only in `pawn.js` | ✅ cargo (both gates) + the serial-gated natives in `entity.rs` |

Deferred to later slices: full schema codegen + the engine-generic `@s2script/std` breadth (Slice 5B); the
`tsc` typecheck gate; interface `.d.ts` codegen; config materialization + permissions + reload state-handoff.

---

## EntityRef across the wire (5A fast-follow)

Slice 4.5 wired two plugins together; Slice 5A made entity access serial-safe. This fast-follow closes
the seam **between** them — the last 5A deferral, *"entity refs on the inter-plugin wire"*. An `EntityRef`
now round-trips across the typed inter-plugin boundary **as a live ref**, not a dead copy: the producer's
`pawnRef(slot)` returns a pawn's `EntityRef`, the marshaller tags it crossing the structured-copy wire
(the `__s2_entref_replacer`), and the consumer's context rehydrates it into an `EntityRef` bound to **its
own** natives (the `__s2_entref_reviver`). The consumer then validates that ref against the **shared**
entity system — and it **flips to invalid across the plugin boundary** when the pawn is destroyed. That is
cross-plugin host-invalidation: the same `{index, serial}` guardrail that protects a stashed `Pawn` within
one plugin now protects a ref handed *between* plugins.

The two demos live in [`examples/entref-producer`](examples/entref-producer) (publishes `@demo/ent@1.0.0`
with `pawnRef(slot) → EntityRef | null` and a producer-side `pawnHealth(slot) → number | null`) and
[`examples/entref-consumer`](examples/entref-consumer) (hard-deps `@demo/ent`). The consumer is
**offset-free** — it never resolves a schema offset; it holds the received `EntityRef` and calls
`ref.isValid()` (TRUE while the pawn lives, FALSE once it is destroyed — the host-invalidation proof) and
reads the number through the producer's `pawnHealth(0)` while alive. Because interface `.d.ts` codegen is
deferred, the consumer hand-writes an ambient [`src/entref-iface.d.ts`](examples/entref-consumer/src/entref-iface.d.ts)
declaring `pawnRef`/`pawnHealth` — `pawnRef` returns the same `@s2script/std` `EntityRef` type the entity
system uses.

### Runbook (build → drop → read → kill)

```bash
cd /home/gkh/projects/s2script
# 1. Build the CLI + both demo .s2sp
node packages/cli/build.mjs
( cd packages/cli && npm link )                 # so `npx s2script` resolves to this repo's CLI
npx s2script build examples/entref-producer
npx s2script build examples/entref-consumer

# 2. Sniper-build the runtime (GLIBC <= 2.31) — MUST post-date the Task-1 commit (fresh replacer/reviver core)
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
mkdir -p dist/addons/s2script/plugins           # package-addon.sh rebuilds dist/ — recreate the plugins dir

# 3. Bring the server up; a CS2 update resets gameinfo.gi — re-patch + restart if the addon won't load
docker compose -f docker/docker-compose.yml up -d
docker exec s2script-cs2 /patch-gameinfo.sh && docker compose -f docker/docker-compose.yml restart cs2
python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1"   # get the map ticking + a bot in slot 0

# --- The cross-plugin host-invalidation gate (host writes into the :ro-mounted plugins dir) ---
cp examples/entref-producer/dist/_demo_entref-producer.s2sp dist/addons/s2script/plugins/   # 1a. producer publishes @demo/ent
cp examples/entref-consumer/dist/_demo_entref-consumer.s2sp dist/addons/s2script/plugins/   # 1b. consumer reads the wired ref → valid=true health=100
python3 scripts/rcon.py "bot_quota 0" "bot_kick"                                            # 2.  REAL destruction → received ref → valid=false health=null
```

**Force a REAL destruction, not `mp_restartgame`.** The 5A spike proved `mp_restartgame` does *not* destroy
the pawn (serials persist), so it never flips the serial. `bot_kick` destroys the controller **and** the
pawn; the received `{index, serial}` then fails the equality check against the engine's live
`CEntityIdentity` — in the *consumer's* context, over a ref that originated in the *producer's* context.

### Captured live log

Captured on a live `joedwards32/cs2` server (version `1.41.6.6/14166`), on a freshly sniper-built core that
post-dates the Task-1 replacer/reviver commit. `bot_kick` (with `bot_quota 0`) destroyed the pawn:

```
# --- boot: fresh core (Task-1 wire marshalling) loads; the EntityRef-backed pawn.js registers ---
[s2script] interface OK: SchemaSystem (SchemaSystem_001)
[s2script] @s2script/cs2 registered (2737 bytes from …/addons/s2script/js/pawn.js)
[s2script] plugins dir: …/game/csgo/addons/s2script/plugins
[META] Loaded 1 plugin.

# 1. drop producer then consumer + a live bot in slot 0 → a producer-passed pawn EntityRef arrives LIVE
[s2script] [producer] onLoad — publishing @demo/ent@1.0.0
[s2script] [consumer] onLoad
[s2script] [consumer] tick 1 received-ref valid=true health=100
[s2script] [consumer] tick 257 received-ref valid=true health=100
[s2script] [consumer] tick 513 received-ref valid=true health=100
[s2script] [consumer] tick 769 received-ref valid=true health=100
[s2script] [consumer] tick 1025 received-ref valid=true health=100

# 2. bot_quota 0; bot_kick → the pawn's entity is DESTROYED; the received ref invalidates ACROSS the
#    plugin boundary — valid=false, health=null (serial mismatch — NOT garbage, NOT a crash). Server ticks on.
[s2script] [consumer] tick 1281 received-ref valid=false health=null
[s2script] [consumer] tick 1537 received-ref valid=false health=null
[s2script] [consumer] tick 1793 received-ref valid=false health=null
[s2script] [consumer] tick 2049 received-ref valid=false health=null
[s2script] [consumer] tick 2305 received-ref valid=false health=null
```

The consumer holds an `EntityRef` it **received across the wire** from the producer: while the pawn lived
it validated `true` against the shared entity system and read `health=100` (via the producer's offset-free
`pawnHealth`); the tick its entity was destroyed the ref flipped to `valid=false`/`health=null` — the
serial no longer matched the engine's `CEntityIdentity`. The server **kept ticking** across the destruction
(`docker ps` showed `Up` throughout; no panic/segfault, no `[s2script]` WARN in the logs — the only
`with error:` line is the unrelated benign `steamclient.so` LAN-mode notice). Cross-plugin
host-invalidation — a live ref handed between plugins, invalidated by a real entity death — proven live.

### 5A-fast-follow acceptance (live + cargo)

| # | Criterion | Result |
|---|---|---|
| build | `cargo test` (82 prior + the in-isolate wire round-trip tests) + both boundary gates + sniper | ✅ 85 core tests; `check-core-boundary` + name-leak gates green; sniper GLIBC ≤ 2.31 (s2script.so 2.14 / core 2.30) |
| wire | `s2script build` produces the producer + consumer `.s2sp`s; the producer publishes `@demo/ent`, the consumer hard-deps it | ✅ both `.s2sp` built; manifests carry `publishes` / `pluginDependencies` |
| live-ref | a producer-passed pawn `EntityRef` arrives LIVE in the consumer and validates against the shared entity system | ✅ live (`[consumer] tick 1 received-ref valid=true health=100`) |
| invalidate | a REAL entity destruction (`bot_kick`) flips the received ref to `isValid()===false` / `health=null` ACROSS the plugin boundary, no crash | ✅ live (`tick 1281 received-ref valid=false health=null`; container `Up`, no panic/segfault) |
| offset-free | the consumer proves invalidation with `ref.isValid()` alone — it resolves no schema offset (the producer's `pawnHealth` is the only offset read) | ✅ consumer `.s2sp` has no schema native; `valid=false` is the proof |
| boundary | core stays engine-generic — the EntityRef replacer/reviver + wire path carry no game names | ✅ cargo (both gates) |

Deferred to later slices: typed `@s2script/cs2` accessors *over* a wired ref (a consumer reconstructing a
`Pawn` from a received `EntityRef` without a producer-side read) come in 5B; the `tsc` typecheck gate;
interface `.d.ts` codegen.

---

## Schema catalog dump (Slice 5B.1 — treadmill)

Slice 5B.1 produces the **schema catalog** — [`games/cs2/gamedata/schema-catalog.json`](games/cs2/gamedata/schema-catalog.json),
a regenerable snapshot of the live CS2 SchemaSystem (every class → `{parent, fields}`; each field →
`{name, offset, type:{kind, name?/inner?}}`). It is the **source of truth** the 5B.3 typed-accessor
codegen consumes, and it is **regenerated after every CS2 update** (offsets/fields move each patch —
this is the maintenance treadmill).

**How it's produced (offsets/signatures are data, not code).** The pure catalog builder lives in
`core/src/schema_catalog.rs` (engine-generic, V8-free, no CS2 names). The live SDK walk lives in the
**shim** (`schema_enumerate` in `shim/src/s2script_mm.cpp`), which streams classes/fields to core over
C-ABI callbacks. The dev/treadmill native `__s2_schema_dump(path) -> boolean` drives that walk, builds a
`Catalog`, and writes JSON — returning `true` only when the schema is warm (classes enumerated) **and**
the file was written. `__s2_schema_dump` is a dev native (not part of the typed `@s2script/std` surface);
the dump plugin declares it ambiently. The catalog stores **per-class own-fields**; inheritance is the
`parent` chain (e.g. `m_iHealth` lives on `CBaseEntity`, so a consumer resolves it by walking
`CCSPlayerPawn → … → CBaseEntity`).

The dump plugin is [`examples/schema-dump`](examples/schema-dump): it subscribes to `OnGameFrame`, waits
~128 ticks for a map to go live + the schema to populate, then retries `__s2_schema_dump("/tmp/schema-catalog.json")`
until it returns `true`.

### Runbook (regenerate after a CS2 update)

```bash
# 1. Build the CLI + the dump plugin → .s2sp
node packages/cli/build.mjs
( cd packages/cli && npm link )                 # so `npx s2script` resolves to this repo's CLI
npx s2script build examples/schema-dump

# 2. Sniper-build the runtime (GLIBC <= 2.31; carries the shim's schema_enumerate) and bring the server up
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
mkdir -p dist/addons/s2script/plugins
docker compose -f docker/docker-compose.yml up -d
# A CS2 update resets gameinfo.gi (metamod SearchPath dropped, addon loads 0 plugins) — re-patch + restart if so:
docker exec s2script-cs2 /patch-gameinfo.sh
docker compose -f docker/docker-compose.yml restart cs2

# 3. Get a map fully live so the SchemaSystem is populated (a hibernating LAN server barely ticks)
python3 scripts/rcon.py "sv_hibernate_when_empty 0" "bot_quota 1" "status"

# 4. Drop the dump plugin (host writes into the :ro-mounted plugins dir; the shim READS it)
cp examples/schema-dump/dist/_demo_schema-dump.s2sp dist/addons/s2script/plugins/
docker logs -f s2script-cs2 | grep schema-dump      # wait for: dump OK -> /tmp/schema-catalog.json

# 5. Copy the catalog out of the container and commit it (the path is relative to the server CWD;
#    the plugin passes an absolute /tmp path, so it lands at /tmp/schema-catalog.json in the container)
docker cp s2script-cs2:/tmp/schema-catalog.json games/cs2/gamedata/schema-catalog.json
git add games/cs2/gamedata/schema-catalog.json && git commit -m "chore(gamedata): regen schema-catalog.json"
```

Captured live log (`joedwards32/cs2`, de_inferno, 2 bots active):

```
[s2script] [schema-dump] onLoad — will dump once the schema is live
[s2script] [schema-dump] dump OK -> /tmp/schema-catalog.json
```

(No `not ready, retrying` lines here because the map was already fully live when the plugin loaded — the
schema was warm on the first attempt at tick 128. On a cold boot the plugin logs retries until the schema
populates.)

### Spot-check the committed catalog

```bash
python3 - <<'PY'
import json
d = json.load(open('games/cs2/gamedata/schema-catalog.json'))
print('classes', len(d))                                   # 2429 (many — all schema scopes)
print('parent', d['CCSPlayerPawn']['parent'])              # CCSPlayerPawnBase
# m_iHealth is INHERITED — resolve it by walking the parent chain (as codegen will):
def resolve(cls, field):
    while cls in d:
        for f in d[cls]['fields']:
            if f['name'] == field: return cls, f
        cls = d[cls].get('parent')
    return None, None
oc, f = resolve('CCSPlayerPawn', 'm_iHealth')
print('m_iHealth', oc, f['type'], f['offset'])             # CBaseEntity {kind:atomic,name:int32} 1456
print('has_handle', any(f['type']['kind']=='handle' for c in d.values() for f in c['fields']))  # True
print('has_class',  any(f['type']['kind']=='class'  for c in d.values() for f in c['fields']))  # True
PY
```

Verified on the committed artifact: **2429 classes**; `CCSPlayerPawn` → parent `CCSPlayerPawnBase`;
`m_iHealth` = `{kind:atomic, name:int32}` at offset **1456** (owned by `CBaseEntity`, cross-checks the
Slice-3 `__s2_schema_offset` resolve of 1456 on de_inferno); field-kind distribution atomic 9591 /
class 1918 / enum 529 / unknown 309 / handle 247 / ptr 113 (≥1 `handle` with an `inner`, ≥1 `class` with
a `name`). The file is deterministic (classes sorted; fields in schema order), so a re-dump on the same
build yields byte-identical JSON.

---

## Typed field access (Slice 5B.2)

Slice 5B.2 extends the `EntityRef` API with **kind-dispatched typed read/write methods** so plugins
can access any field from the committed schema catalog — not just `i32` — via the same serial-gated
`T | null` contract as `Pawn.health`. All reads are serial-gated: if the entity slot has been reused
or destroyed, every accessor returns `null` (or `false`), never garbage.

### `EntityRef` typed-method surface

| Method | Return | Description |
|---|---|---|
| `readInt32(off)` | `number \| null` | Read a signed 32-bit integer (existing) |
| `writeInt32(off, v)` | `boolean` | Write a signed 32-bit integer (existing) |
| `readFloat32(off)` | `number \| null` | Read an IEEE-754 float32 |
| `writeFloat32(off, v)` | `boolean` | Write a float32 |
| `readBool(off)` | `boolean \| null` | Read a single byte as bool |
| `writeBool(off, v)` | `boolean` | Write a bool |
| `readInt8(off)` | `number \| null` | Read a sign-extended int8 |
| `readInt16(off)` | `number \| null` | Read a sign-extended int16 |
| `readUInt8(off)` | `number \| null` | Read an unsigned uint8 |
| `readUInt16(off)` | `number \| null` | Read an unsigned uint16 |
| `readUInt32(off)` | `number \| null` | Read an unsigned uint32 |
| `readHandle(off)` | `EntityRef \| null` | Read a `CEntityHandle` field, decode it, and return a **live, serial-gated** `EntityRef` — or `null` if the source ref is stale or the handle slot is invalid |

All methods live on the engine-generic `EntityRef` class in `@s2script/std`. Offsets come from
`__s2_schema_offset("ClassName", "fieldName")`, which walks the inheritance chain (so an inherited
field resolves correctly regardless of which base class owns it). The runtime dispatches on the
`kind` parameter passed by the caller — `"f32"`, `"bool"`, `"i8"`, etc. — to a single
`__s2_ent_ref_read` / `__s2_ent_ref_write` native.

### Usage snippet (from `examples/demo-plugin`)

```ts
import { OnGameFrame, EntityRef } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

declare const __s2_schema_offset: (cls: string, field: string) => number;

let FRICTION_OFF   = -1; // m_flFriction (float32, CBaseEntity)
let RAGDOLL_OFF    = -1; // m_bClientSideRagdoll (bool, CBaseEntity)
let CONTROLLER_OFF = -1; // m_hController (handle → CBasePlayerController, CBasePlayerPawn)
let PLAYERPAWN_OFF = -1; // m_hPlayerPawn (handle → CCSPlayerPawn, CCSPlayerController)

OnGameFrame.subscribe(() => {
  const p = Pawn.forSlot(0);   // Pawn | null (EntityRef-backed)
  if (!p) return;

  // Resolve offsets once (schema warm after map load; OffsetCache makes repeated calls free)
  if (FRICTION_OFF   < 0) FRICTION_OFF   = __s2_schema_offset("CCSPlayerPawn", "m_flFriction");
  if (RAGDOLL_OFF    < 0) RAGDOLL_OFF    = __s2_schema_offset("CCSPlayerPawn", "m_bClientSideRagdoll");
  if (CONTROLLER_OFF < 0) CONTROLLER_OFF = __s2_schema_offset("CCSPlayerPawn", "m_hController");
  if (PLAYERPAWN_OFF < 0) PLAYERPAWN_OFF = __s2_schema_offset("CCSPlayerController", "m_hPlayerPawn");

  const friction: number | null  = FRICTION_OFF >= 0 ? p.ref.readFloat32(FRICTION_OFF) : null;
  const ragdoll: boolean | null  = RAGDOLL_OFF  >= 0 ? p.ref.readBool(RAGDOLL_OFF)     : null;
  // readHandle decodes the CEntityHandle into a live, serial-gated EntityRef (or null).
  // Chain a read THROUGH it — the controller's m_hPlayerPawn handle back to the pawn —
  // proving the handle-derived ref is a usable, live EntityRef, not just data.
  const ctrl: EntityRef | null   = CONTROLLER_OFF >= 0 ? p.ref.readHandle(CONTROLLER_OFF) : null;
  const back = ctrl && PLAYERPAWN_OFF >= 0 ? ctrl.readHandle(PLAYERPAWN_OFF) : null;

  console.log("friction=" + friction + " ragdoll=" + ragdoll
    + " controller=" + (ctrl ? ("idx=" + ctrl.index + " valid=" + ctrl.isValid()) : "null")
    + " pawnBack=" + (back ? ("idx=" + back.index + " valid=" + back.isValid()) : "null"));
});
```

`m_flFriction` and `m_bClientSideRagdoll` live on `CBaseEntity`, `m_hController` on
`CBasePlayerPawn` — all inherited by `CCSPlayerPawn`, and `__s2_schema_offset` walks the base-class
chain, so passing `"CCSPlayerPawn"` resolves them. `readHandle` demonstrates the full round-trip:
decode the `CEntityHandle` → serial-validate → a live `EntityRef` (or `null`), which is itself
serial-gated — so a field read *through* the handle-derived ref (here another handle, back to the
pawn) is safe.

**Live-gate log** (Docker CS2, `de_inferno`, one bot in slot 0; the runtime rebuilt via
`scripts/build-sniper.sh`, the demo hot-reloaded from its `.s2sp`):

```
[demo] tick 1     stashed.health=100  fresh.health=100  friction=1 ragdoll=false controller=idx=1 valid=true pawnBack=idx=732 valid=true
[demo] tick 257   stashed.health=100  fresh.health=100  friction=1 ragdoll=false controller=idx=1 valid=true pawnBack=idx=732 valid=true
...
bot_quota 0 ; bot_kick        # real entity destruction (NOT mp_restartgame — serials persist across that)
[demo] tick 2305  stashed.health=100  fresh.health=100  friction=1 ragdoll=false controller=idx=1 valid=true pawnBack=idx=732 valid=true
[demo] tick 2561  stashed.health=null fresh.health=none  friction=null ragdoll=null controller=null
[demo] tick 2817  stashed.health=none fresh.health=none  friction=null ragdoll=null controller=null
```

Alive: `readFloat32` reads `1` (m_flFriction), `readBool` reads `false`, `readHandle` yields a live
`EntityRef` for the controller (idx 1), and a chained `readHandle` through it reads back to the pawn
(idx 732) — both valid. After `bot_kick` the pawn's entity is destroyed: every typed read (and the
stashed pawn's `health`) flips to `null` on the serial mismatch, `Pawn.forSlot(0)` returns `null`,
and the server keeps ticking with no crash — the serial-gated `T | null` contract holds for every
scalar type, not just `i32`.

---

## Schema codegen (Slice 5B.3)

Slice 5B.3 turns the raw plumbing above into **generated typed accessors**. Authors write idiomatic
properties; the `__s2_schema_offset` + `EntityRef.read*` calls become internal to the generated code:

```ts
import { Pawn } from "@s2script/cs2";

const p = Pawn.forSlot(0);
if (p) {
  const hp = p.health;             // number | null   (generated from m_iHealth)
  p.health = 100;                  // setter writes + auto-notifyStateChanged
  const f  = p.friction;           // number | null   (m_flFriction)
  const rag = p.clientSideRagdoll; // boolean | null  (m_bClientSideRagdoll)
  const ctrl = p.controller;       // EntityRef | null (m_hController, a handle field)
}
```

No offsets, no `readFloat32`, no `__s2_schema_offset` in author code — and offsets are still resolved
**live** at runtime (never baked), so a per-patch offset move is absorbed by regenerating, not editing code.

**How it's generated (`s2script gen-schema`).** A pure offline transform over the committed
[`schema-catalog.json`](games/cs2/gamedata/schema-catalog.json) + a curated class list
([`games/cs2/codegen-classes.json`](games/cs2/codegen-classes.json) = `CCSPlayerPawn`,
`CCSPlayerController`, `CCSWeaponBase`, whose ancestor chains are pulled in automatically). It emits two
**committed** artifacts: the runtime accessors [`games/cs2/js/schema.generated.js`](games/cs2/js/schema.generated.js)
(injected ahead of `pawn.js`) and the author types [`packages/cs2/schema.generated.d.ts`](packages/cs2/schema.generated.d.ts)
(`export interface CCSPlayerPawn extends CCSPlayerPawnBase { … }`). Property names are idiomatic (strip the
`m_` + Hungarian tag, camelCase); the generated code hardcodes the raw name + declaring class as the resolve
key (`off("CBaseEntity","m_iHealth")`), and idiomatic collisions fall back to the raw name. Only scalar +
handle fields are generated this slice — strings, vectors, embedded structs, `enum` (the catalog lacks enum
byte-width), and 64-bit ints are skipped with a logged per-class reason.

**Treadmill runbook (per CS2 update).** Offsets move every patch, so after a game update: re-dump the
catalog (Slice 5B.1) → `node packages/cli/dist/cli.js gen-schema` → commit the regenerated
`schema.generated.{js,d.ts}`. [`scripts/check-schema-generated.sh`](scripts/check-schema-generated.sh)
regenerates and `git diff --exit-code`s to fail CI if the catalog changed but the codegen wasn't rerun. The
generator is deterministic (byte-identical output), so the gate is meaningful. The curated list grows as the
base-plugin suite (Slice 6) needs more classes.

**Live-gate log** (Docker CS2, `de_inferno`, one bot; **no sniper rebuild** — 5B.3 changed no Rust/core/shim,
only the CLI tooling + injected JS):

```
[demo] tick 513  health=100 friction=1 ragdoll=false team=2 controller=idx=1 valid=true stashed.health=100
...
bot_quota 0 ; bot_kick        # destroy the pawn entity (NOT mp_restartgame)
[demo] tick 4865 health=100  friction=1    ragdoll=false ... controller=idx=1 valid=true stashed.health=100
[demo] tick 5121 health=none friction=none ragdoll=none team=none controller=null      stashed.health=null
[demo] tick 5377 health=none friction=none ragdoll=none team=none controller=null      stashed.health=none
```

Every value (`health`, `friction`, `clientSideRagdoll`, `teamNum`, and the `controller` handle) is read
through a **generated** accessor. Alive they read correct values; after `bot_kick` the pawn's entity is
destroyed and every generated getter — including the stashed pawn's `health` — returns `null` on the serial
mismatch, with the server still ticking, no crash. The serial-gated `T | null` guarantee is preserved through
the generated layer.

---

## Module packages (Slice 5C.1)

The engine-generic standard library is not one package — it's a set of per-capability packages under the
`@s2script/*` scope. Authors import from the specific module:

```ts
import { EntityRef } from "@s2script/entity";
import { OnGameFrame } from "@s2script/frame";
import { delay, nextTick, nextFrame, threadSleep } from "@s2script/timers";
import { console } from "@s2script/console";
import { publishInterface } from "@s2script/interfaces";
```

| Package | Provides |
|---|---|
| `@s2script/entity` | `EntityRef` (serial-gated entity access + typed read/write/`readHandle`) |
| `@s2script/frame` | `OnGameFrame`, `SubscribeOptions` |
| `@s2script/timers` | `delay`, `nextTick`, `nextFrame`, `threadSleep` |
| `@s2script/console` | `console` |
| `@s2script/interfaces` | `publishInterface`, `PublishHandle` |

There is no `@s2script/std` umbrella — one import path per capability. Resolution is one engine-generic rule:
core's `require` maps any `@s2script/<name>` to the injected `globalThis.__s2pkg_<name>` (a game package like
`@s2script/cs2` uses the very same rule), so new first-party modules never need a core change. The CLI
externalizes `@s2script/*` by wildcard, so those imports stay `require()` calls the engine resolves at load.

**Live-gate log** (Docker CS2, `de_inferno`, one bot; the demo imports `OnGameFrame` from `@s2script/frame`
and `Pawn` from `@s2script/cs2`):

```
[demo] tick 3073 health=100 friction=1 ragdoll=false team=2 controller=idx=1 valid=true stashed.health=100
bot_kick
[demo] tick 4097 health=none friction=none ragdoll=none team=none controller=null stashed.health=null
```

The demo loading and ticking at all proves the module resolution end-to-end (`require("@s2script/frame")`,
`require("@s2script/cs2")`, and `pawn.js`'s `require("@s2script/entity")` all resolve live through the one
generalized rule); the generated `Pawn` accessors still read correctly and still degrade to `null` on entity
death — the split changed how the API is *packaged*, not how it behaves.

---

## The player model (Slice 5C.2)

CS2 splits SourceMod's single "client" into two entities: the **controller** (`CCSPlayerController` — the
persistent player: team/score/ping; survives death) and the **pawn** (`CCSPlayerPawn` — the in-world body:
health/position; respawns). The player model makes that honest — `Player` **is** the controller, with typed
navigation to the pawn:

```ts
import { Player } from "@s2script/cs2";

for (const p of Player.all()) {          // the in-game players
  const team = p.teamNum;                // generated CCSPlayerController accessor
  const body = p.pawn;                   // controller -> pawn (Pawn | null)
  const hp = body ? body.health : null;  // the pawn's generated accessor
  const back = body ? body.controller : null;  // pawn -> controller reverse hop
}
const p0 = Player.fromSlot(0);           // Player | null (0-based CPlayerSlot)
```

`Player` wraps the controller `EntityRef` (entity index `slot+1`); `player.pawn` reads `m_hPlayerPawn` and
`pawn.controller` reads `m_hController` — both typed, both serial-gated (`T | null`). A stored `Player`
degrades to `null` on reuse/disconnect (no dangling — the safety CSSharp uses userid re-lookup for, we get
from serial-gating). It's entirely in the `@s2script/cs2` JS + types layer — **no core/shim change**.

**Occupancy** was a live-gate finding: CS2 pre-allocates all 64 controller entities, so `isValid()` alone
returns every slot, and `m_iConnected` reads `0` for occupied *and* empty slots (verified via a diagnostic).
The clean signal is that an occupied controller has a **live player pawn** — so `Player.all()`/`fromSlot`
keep the in-game (spawned) players. Connected-but-pawnless (dead/spectating) enumeration plus
`player.userId`/`Player.fromUserId` are the **engine-identity follow** (they need engine natives; the
engine `userId` isn't schema-readable). *(Update: `player.playerName` and `player.steamID` **are**
schema-readable — `m_iszPlayerName`/`m_steamID` — and landed as generated accessors in **Slice 5B.4** below.)*

**Live-gate log** (Docker CS2, `de_inferno`, `bot_quota 2`; **no sniper rebuild** — JS-only):

```
[demo] tick 3585 players=2
  slot=0 teamNum=2 health=100 backSlot=0
  slot=1 teamNum=3 health=100 backSlot=1
bot_kick
[demo] tick 4353 players=0
```

`Player.all()` returns exactly the two bots (filtered from 64 controllers); `player.teamNum` reads, `player.pawn`
→ `health`, and `pawn.controller` round-trips back to the same slot; all drop to `players=0` on disconnect,
server ticking, no crash.

---

## String + 64-bit fields (Slice 5B.4)

Slice 5B.4 extends the typed reads + codegen to the last two common scalar kinds: **`char[N]` inline strings**
and **64-bit numbers** (`uint64`/`int64`/`float64`). This gives the player model its identity —
`player.playerName` (`m_iszPlayerName`, a `char[128]`) and `player.steamID` (`m_steamID`, `uint64`) — as
generated accessors, with **no engine natives**:

```ts
import { Player } from "@s2script/cs2";

for (const p of Player.all()) {
  const name = p.playerName;   // m_iszPlayerName (char[128]) -> string
  const sid  = p.steamID;      // m_steamID (uint64)          -> DECIMAL STRING (e.g. "76561198…")
}
```

**64-bit ⇒ decimal *string* (the load-bearing decision).** The low-level `EntityRef.readUInt64`/`readInt64`
primitives return the exact `bigint` (with `readFloat64` → `number`, `readString(off, maxLen)` → `string`), but
the **generated** accessors for `uint64`/`int64` fields return a **decimal string**. This is deliberate:

- **SourceMod-parity** — SourcePawn has no 64-bit integer type, so `GetClientAuthId(client, AuthId_SteamID64, …)`
  hands back a string. Steam IDs are used as strings everywhere (bans, stats, keys, logging, comparison).
- **Wire-safe** — a string crosses the inter-plugin structured-copy wire unchanged, so this **dissolves the
  `BigInt`-on-the-wire concern** for field reads entirely (no `devalue`, no serialization edge). An author who
  wants numeric/bitmask math uses the `readUInt64` primitive directly, or `BigInt(str)`.
- **Exact** — no `2^53` precision loss (a steamid64 ≈ `7.6e16` overflows a JS `number`).

Under the hood the `str`/`u64`/`i64` getters are generated: a `str` field emits `readString(off, N)`; a
`u64`/`i64` field reads the `bigint` primitive, null-guards, and `.toString()`s it. Offsets stay live-resolved
(layout-is-data); reads only (string + 64-bit writes, and `CUtlString`/`CUtlSymbolLarge` pointer-backed strings,
are deferred). A **name-transform fix** shipped alongside: `idiomaticName` now strips only a *known* Hungarian-tag
set, so `m_steamID` → `steamID` (was the over-stripped `iD`) and `m_bombSite` → `bombSite` (was `site`), while
the existing gameplay names (`health`/`teamNum`/`friction`/`pawn`/`controller`) are unchanged.

**Live-gate log** (Docker CS2, `de_inferno`, `bot_quota 2`; sniper-rebuilt for the new natives). This log is from
an *instrumented* form of the demo that reads each player twice — the generated accessors (`GEN`) and the raw
primitives (`RAW`) — to prove they agree; the committed `examples/demo-plugin` emits just the generated form. See
[the spike-findings doc](docs/superpowers/specs/2026-07-02-slice-5b4-spike-findings.md) for the full comparison:

```
[demo] tick 257 players=2
  slot=0 | GEN name="Specialist" steamID=0 (typeof string) | RAW name="Specialist" sid=0 (nameOff=2036 sidOff=2528) health=100
  slot=1 | GEN name="Rex"        steamID=0 (typeof string) | RAW name="Rex"        sid=0 (nameOff=2036 sidOff=2528) health=100
bot_kick
[demo] tick 3841 players=0        (server ticking, no crash)
```

`player.playerName` reads the bot names (`char[128]` → string); `player.steamID` is a **string** `"0"` (bots read
`0`; a real player reads `"7656…"`), matching the raw `readString(2036, 128)` / `readUInt64(2528)`. Both drop to
`null`/empty on disconnect (`Player.all()` → `players=0`), server stable.

---

## Vector value type (Slice 5C.3)

Slice 5C.3 opens the `@s2script/std`-breadth taxonomy with the first engine-generic value-type module,
`@s2script/math`, and extends the codegen to **direct atomic `Vector`/`QAngle` fields**:

```ts
import { Player } from "@s2script/cs2";

for (const p of Player.all()) {
  const body = p.pawn; if (!body) continue;
  const aim = body.eyeAngles;      // m_angEyeAngles (QAngle) -> { x: pitch, y: yaw, z: roll } | null
  const vel = body.absVelocity;    // m_vecAbsVelocity (Vector) -> { x, y, z } | null
  const speed = vel ? vel.length() : 0;   // magnitude helper
}
```

- **`@s2script/math`** — the `Vector` / `QAngle` value types (copied `{x,y,z}` snapshots; `Vector.length()` is
  the magnitude). They live in the core prelude alongside `entity`/`frame`/… because a vector is engine-generic
  (true on any Source 2 game), not CS2-specific. A `Vector` is a plain object, so it crosses the inter-plugin
  structured-copy wire cleanly — no wire caveat.
- **`EntityRef.readFloats(off, count)`** — one serial-gated lookup reading `count` (1–4) contiguous `float32`s
  into a `number[]`. The **generated getter** (game layer) wraps it: `var a = this.ref.readFloats(off, 3);
  return a === null ? null : new Vector(a[0], a[1], a[2]);`. `@s2script/entity` stays independent of
  `@s2script/math` — the value-type construction lives in the generated code, which requires `@s2script/math`
  itself.
- **Codegen** maps the `atomic` `Vector`/`QAngle` type-names (fixed 3-float layout) to `vector`/`qangle` kinds;
  `pawn.eyeAngles`, `pawn.absVelocity` (and ~15 more direct Vector/QAngle fields) become generated accessors.

**Live-gate log** (Docker CS2, `de_inferno`, `bot_quota 2`; sniper-rebuilt for the new native):

```
[demo] tick 513 players=2
  slot=0 eyeAngles=QAngle(1.716, 78.596, 0) absVelocity=Vector(0, 0, 0) speed=0.0
  slot=1 eyeAngles=QAngle(1.716, -107.403, 0) absVelocity=Vector(0, 0, 0) speed=0.0
bot_kick
[demo] tick 7425 players=0        (server ticking, no crash)
```

`pawn.eyeAngles` reads the bots' live, frame-varying view angles (pitch/yaw change each tick, roll = 0);
`pawn.absVelocity` reads a `Vector` (`0,0,0` for the stationary bots — a correct read via the same `readFloats`
path). Both drop to `null` on disconnect (`players=0`), server stable.

**Deferred:** `origin` (behind the `CGameSceneNode` pointer — shipped in **Slice 5C.4** below); Vector **writes**
(velocity/angle networking, and `origin`-write = an engine `Teleport()`, not a field poke);
`Vector2D`/`Vector4D`/`Color`/`Quaternion` value types + codegen; Vector arithmetic.

---

## Pointer-chain fields — origin (Slice 5C.4)

`origin` (a player's world position) doesn't live on the entity — it's behind a two-pointer chain
(`entity → CBodyComponent → CGameSceneNode`). Slice 5C.4 adds the capability to follow such a chain **entirely
in-core** and read a copied value at the end, and applies it (hand-written) to ship `pawn.origin` +
`pawn.angles`:

```ts
import { Player } from "@s2script/cs2";

for (const p of Player.all()) {
  const body = p.pawn; if (!body) continue;
  const pos = body.origin;    // Vector {x,y,z} | null — world position (m_vecAbsOrigin, via the chain)
  const rot = body.angles;    // QAngle {x,y,z} | null — body rotation (m_angAbsRotation); ≠ eyeAngles (view/aim)
}
```

- **`EntityRef.readFloatsChain(ptrOffs, finalOff, count)`** — the engine-generic primitive: serial-gate the root
  entity, follow each pointer offset in `ptrOffs` (`p = *(p + off)`, null-checking every hop), then read `count`
  floats at `finalOff`. The raw `CBodyComponent*`/`CGameSceneNode*` **never cross to JS** — the whole chain is
  followed and the value copied out inside one synchronous native. A stale root or a null hop → `null`.
- **`pawn.origin` / `pawn.angles`** are **hand-written** in `pawn.js` (like the 5C.2 player nav): they resolve
  the chain offsets (`m_CBodyComponent` → `m_pSceneNode` → `m_vecAbsOrigin`/`m_angAbsRotation`) **live** via
  `__s2_schema_offset` and call `readFloatsChain`. CS2 field names stay in the game layer; core stays generic.

**Live-gate log** (Docker CS2, `de_inferno`, `bot_quota 2`; sniper-rebuilt for the new native) — two bots at
their (distinct) spawn points:

```
[demo] tick 257 players=2
  slot=0 origin=Vector(-1662.18, 288.76, -63.97) angles=QAngle(0, 77.5, 0)
  slot=1 origin=Vector(2353, 1977, 135.52)        angles=QAngle(0, 97.5, 0)
bot_kick
[demo] tick 2305 players=0        (server ticking, no crash)
```

Real de_inferno world coordinates (CT vs T spawns, far apart) — not zero, not garbage, not null; both drop out on
disconnect, server stable.

**Deferred:** teaching the *codegen* to auto-generate embedded/ptr accessors across the schema graph (this slice
is the capability + a hand-written application); the quantized `m_vecOrigin` wrapper; Vector/origin **writes**
(teleport = an engine `Teleport()` call); a generic scalar-behind-pointer read (this native reads floats only).

---

## Game events (Slice 5D.1)

A generic engine-generic game-event bus with a typed CS2 overlay for IntelliSense:

```ts
import { Events, Player } from "@s2script/cs2";   // typed overlay (event names + fields autocomplete)

Events.on("player_death", (ev) => {               // "player_death" autocompletes; ev is typed to it
  const victim   = Player.fromSlot(ev.getPlayerSlot("userid"));   // key "userid" autocompletes (player field)
  const attacker = Player.fromSlot(ev.getPlayerSlot("attacker"));
  const weapon   = ev.getString("weapon");                        // key "weapon" autocompletes (string field)
  const headshot = ev.getBool("headshot");
});
```

- **The mechanism** — `@s2script/events` (engine-generic): the shim registers an `IGameEventListener2` with the
  engine's `IGameEventManager2`, holds the live `IGameEvent*` (it **never crosses to JS**), and calls into a
  core event multiplexer (per-name subscriber lists, re-entrancy-safe snapshot, ledgered teardown — mirrors
  `OnGameFrame`). The handler gets a **live `GameEvent` accessor** valid only during the synchronous handler
  (`getInt`/`getFloat`/`getBool`/`getString`, `getUint64` → decimal string, `getPlayerSlot` → slot); a stashed
  `ev` used after an `await` reads defaults. Player resolution is `Player.fromSlot(ev.getPlayerSlot(key))` — so
  events need no engine-identity dependency.
- **The typed overlay** — event names are CS2 facts, so the IntelliSense lives in `@s2script/cs2`: a committed,
  reference-sourced [`event-catalog.json`](games/cs2/gamedata/event-catalog.json) (CS2 buries event defs in the
  VPK — no live dump) is codegen'd (types-only, freshness-gated) into
  [`packages/cs2/events.generated.d.ts`](packages/cs2/events.generated.d.ts): a `GameEvents` map + a typed
  `Events.on<K>` overload where each getter's `key` is constrained to that event's fields of the matching type.
  The runtime stays the generic bus; any uncatalogued event still works via the string API.

**Status — mechanism + types complete; live delivery pending a signature.** The core mechanism (in-isolate
tested), the `@s2script/events` module, and the typed catalog all ship and work. But **CS2 does not export
`IGameEventManager2` via `CreateInterface`** (neither the engine nor the server factory resolves
`GAMEEVENTSMANAGER002`) — the manager must be found via a **signature scan** (as CounterStrikeSharp/Swiftly do),
which is a per-update gamedata/treadmill item deferred to **5D.1b**. Until then the event ops degrade gracefully
(`Events.on` is a no-op receiver; nothing crashes). See
[the live-gate findings](docs/superpowers/specs/2026-07-02-slice-5d1-spike-findings.md).

**Deferred:** the `IGameEventManager2` signature-scan acquisition (5D.1b); blocking/pre-hooks/`HookResult` for
events; *firing* events (`Events.fire`); an auto-dumped event catalog (VPK/KV3 extraction or runtime-RE); an
eager stashable event snapshot.

---

## Pointer-chain wrappers (Slice 5C.5)

5C.4 hand-wrote `pawn.origin` by following a pointer chain (`entity → CBodyComponent → CGameSceneNode`). Slice
5C.5 **generalizes that into codegen** over a *curated* set of navigation targets — so fields behind pointers
become typed, pointer-backed wrappers:

```ts
import { Player } from "@s2script/cs2";

for (const p of Player.all()) {
  const b = p.pawn; if (!b) continue;
  const pos    = b.sceneNode?.absOrigin;         // Vector — world position (via the CGameSceneNode chain)
  const scale  = b.sceneNode?.scale;             // number
  const weapon = b.weaponServices?.activeWeapon; // EntityRef | null — the active weapon (a handle through the chain)
  const ducked = b.movementServices?.ducked;     // boolean
}
```

- **Curated, not traversed.** The schema graph has 1918 embedded + 113 pointer fields, is cyclic, and is mostly
  engine noise — so a committed [`nav-targets.json`](games/cs2/nav-targets.json) lists the *few* useful
  `{source → path → target}` chains (the scene node + the pawn's weapon/movement/aim-punch services), exactly
  like `codegen-classes.json` curates schema classes. `navgen` emits a pointer-backed wrapper per target (its
  fields reused from the schema codegen) + a nav accessor on the source, freshness-gated.
- **The runtime primitive** is one generic `EntityRef.readInt32Via`/`readFloat32Via`/`readBoolVia`/`readHandleVia`/…
  over a `__s2_ent_ref_read_chain` native (the `KIND_*` scalar dispatch + a pointer-chain deref, all in-core);
  vectors reuse 5C.4's `readFloatsChain`.
- **Safety:** a wrapper holds `(rootEntityRef, pathOffsets)` — never a cached pointer. **Every field access
  re-resolves the chain in-core, serial-gated at the root**, so a stale entity reads `null`, never garbage. The
  nav accessor even re-resolves the path *offsets* per access (self-healing across a schema-warm boundary).
- **`pawn.origin`/`pawn.angles`** (5C.4) are kept as thin aliases → `pawn.sceneNode.absOrigin`/`absRotation`.

**Live-gate log** (Docker CS2, `de_inferno`, `bot_quota 2`):

```
  slot=0 absOrigin=Vector(-1675.6, 351.7, -64.0) scale=1 activeWeapon=ref#1014 ducked=false origin(alias)=ok
  slot=1 absOrigin=Vector(2472.3, 2006.0, 134.5) scale=1 activeWeapon=ref#1013 ducked=false origin(alias)=ok
bot_kick
[demo] tick 4865 players=0        (server ticking, no crash)
```

All four wrappers read correct through their chains (two distinct spawns; valid weapon `EntityRef`s), degrade to
`null` on disconnect. **Deferred:** graph auto-traversal / non-curated targets; cyclic or deeper chains;
`CUtlVector`/array fields (`m_hMyWeapons`); pointer-chain *writes*.

---

## Live game events + engine identity (Slice 5D.2)

Slice 5D.2 reaches two **un-exported** CS2 engine facilities through committed, regenerable gamedata facts —
one **signature** and six **offsets** — lighting up live game-event delivery (the deferred 5D.1b) and engine
identity (`player.userId` / `Player.fromUserId` / `Player.allConnected`). The access *mechanisms* stay
engine-generic in core/shim; the *values* live in `gamedata/core.gamedata.jsonc`; the `Player.*` API lives in
the CS2 game layer.

**Events (Thread A).** `CreateInterface("GAMEEVENTSMANAGER002")` returns nothing in CS2 (Slice 5D.1 confirmed
it), so a pure byte-pattern scanner (`shim/src/sigscan.{h,cpp}`, host-`g++`-tested) resolves the
`IGameEventManager2*` global from `libserver.so` via a committed **ctor-body-xref signature** (a
`.signatures` gamedata section + `LoadSignatures`), populating `s_pGameEventManager`. Everything downstream from
5D.1 (`Events.on`, the `GameEvent` accessor, `event_mux`, the typed catalog) is unchanged.

**Identity (Thread B).** Five engine-generic client-list ops (appended to `S2EngineOps`) read
`INetworkServerService*` → game server → `CServerSideClient[]` at gamedata offsets; `player.userId` is the engine
user-id (not schema), `Player.fromUserId(id)` looks a player up by it, and `Player.allConnected()` enumerates
connected players **regardless of pawn** (complementing the pawn-gated `Player.all()`).

**Live gate (de_inferno, `bot_quota 2`):**

```
[s2script] interface OK: GameEventManager (sig-scan ctor-body-xref, 0x…e0860)
[demo] EVENT player_spawn slot=0 / slot=1
[demo] EVENT round_start timelimit=0
[demo] allConnected=2
  slot=0 userId=0 teamNum=2 pawn=yes fromUserId(uid).slot=0
  slot=1 userId=1 teamNum=3 pawn=yes fromUserId(uid).slot=1
bot_kick → [demo] allConnected=0        (server ticking, no crash)
```

`fromUserId` round-trips to the same slot (the engine client-list index == the player slot). **Live-gate bug
fixed:** `FindModuleText` first matched Metamod's thin `libserver.so` *proxy* (its path also contains the
substring) — now it scans all substring matches and keeps the **largest** executable segment (the real game
module). **Deferred:** blocking/pre-hooks + *firing* events; connected-client steam-id / HLTV typing;
auto-regenerated signatures/offsets.

> **Live-gate deploy note.** `scripts/package-addon.sh` `rm -rf`s the bind-mounted `dist/addons/s2script`, which
> detaches the container's mount — `docker compose -f docker/docker-compose.yml restart cs2` re-binds it AND
> preserves the `gameinfo.gi` Metamod patch. Avoid `--force-recreate` (it resets `gameinfo.gi`; if used, re-run
> `docker exec s2script-cs2 /patch-gameinfo.sh` then restart).

---

## Event actionability — block / modify / fire (Slice 5D.3)

Slice 5D.3 adds the **write** direction to game events (5D.1/5D.2 were read + delivery), bringing the
event system to parity with the `OnGameFrame` multiplexer. `Events.on` (notify/post) is unchanged.

```ts
import { Events, HookResult } from "@s2script/cs2";

// BLOCK + MODIFY: a pre-hook runs before the event broadcasts. Read/modify it, and return a HookResult:
//   Handled/Stop → suppress the client broadcast (the server still processes it; `on` post-subs still fire).
Events.onPre("player_hurt", (ev) => {
  ev.setInt("dmg_health", (ev.getInt("dmg_health") / 2) | 0);   // modify the live event
  return HookResult.Handled;                                     // suppress the broadcast (SM parity)
});

// FIRE: synthesize + fire an event (runtime types are inferred from the JS values).
Events.fire("player_death", { userid: 0, attacker: 1, weapon: "ak47" });
```

**Mechanism.** The shim `SH_ADD_HOOK`s the sig-scanned `IGameEventManager2::FireEvent` (from 5D.2); a Pre
hook runs the JS pre-subscribers, collapses their `HookResult`s through the same `run_chain` the frame
multiplexer uses, and on suppress re-calls the original with `bDontBroadcast=true` + `MRES_SUPERCEDE`.
The mechanism is engine-generic (core/shim); only the typed `Events.onPre<K>`/`fire<K>` overlay is CS2.

**Live gate (de_inferno):**
```
[demo] fired player_hurt (from onLoad) ok=true
[demo] PRE round_start timelimit 0->4242 (Handled)
[demo] POST round_start timelimit=4242        (modify + block-as-broadcast-suppress confirmed)
```

**Limitation (by design).** A JS-triggered `Events.fire` cannot re-dispatch to this framework's own
`on`/`onPre` JS subscribers — all JS runs while the V8 isolate is borrowed, so the fired event reaches
the engine (clients + C++ listeners / other plugins) but not our JS subs on that pass. A re-entrancy
guard skips the nested dispatch gracefully (no panic). Firing an event your own plugin also subscribes to
and expects to re-handle synchronously is the case this doesn't cover.

---

## The typecheck gate (Slice 5E.1)

`s2script build` **typechecks** each plugin (full `strict`) against the shipped engine `.d.ts` before it
bundles, and **fails the build (emits no `.s2sp`) on any type error**:

```
$ npx s2script build ./my-plugin
typecheck failed (1 error(s)):
  src/plugin.ts:12:9 — TS18047: 'p' is possibly 'null'.
$ echo $?
1
```

Because the dev file-watch reload *rebuilds*, a failing typecheck produces no new `.s2sp`, so the
running plugin is left untouched — the charter's "a failing reload leaves the running version
untouched," for free. (The other half of the charter, the load-time `apiVersion` refuse, already lives
in the host — a plugin whose `apiVersion` major differs from the host is skipped at load.)

- **Full `strict`** is the fixed baseline — the API is `T | null` everywhere by design, so null-checking
  is the point.
- **`@s2script/*`** resolves to the shipped `packages/*/index.d.ts`; the injected `console` global comes
  from a shipped `globals.d.ts` (the sandbox — never `lib: dom`, so `window`/`document` stay undefined).
- **Plugins are pure ESM.** `import x = require(...)` is rejected; author inter-plugin imports as ESM
  named imports (`import { greet } from "@demo/greeter"`) — esbuild bundles them to the same runtime
  access, so this is authoring hygiene, not a behaviour change.
- **Inter-plugin deps** are `any` for now (a typed producer→consumer `.d.ts` is deferred).
- Every `examples/*` plugin passes the gate (`scripts/check-examples-typecheck.sh`), which keeps the
  shipped `.d.ts` surface honest.

---

## Plugin config (Slice 5E.2)

Plugins get settings: **declare** typed config in `package.json`, the host **materializes** it at load
(declared defaults merged with an admin-editable JSON file, auto-generated on first run), and the plugin
**reads** it through a typed `@s2script/config` API — with opt-in **live-reload** when the author registers
`onChange`.

**Declare** — the `s2script.config` block (types: `string`/`int`/`float`/`bool`; each a
`{ type, default, description? }`). The CLI validates `default` matches `type` at build (a mismatch fails
the build) and bakes the block into the `.s2sp` manifest:

```json
"s2script": {
  "config": {
    "greeting": { "type": "string", "default": "hello from s2script", "description": "Logged on load" },
    "maxUses":  { "type": "int",    "default": 3, "description": "Demo counter" },
    "enabled":  { "type": "bool",   "default": true, "description": "Feature toggle" }
  }
}
```

**Materialize** — at load the host reads `addons/s2script/configs/<plugin-id>.json` (the sanitized id),
**auto-generating** it with the declared defaults + `//`-comments if absent, then merges defaults with the
file per-key. A wrong-typed or malformed value degrades to that key's default + a `WARN` — a broken config
file never crashes the plugin.

**Read** — `import { config } from "@s2script/config"`; `config.getString/getInt/getFloat/getBool(key)`
return the materialized value (an undeclared key yields the type zero-value, never throws).

**Live-reload (opt-in)** — a plugin that never calls `config.onChange` is read-only and its file is not
watched (zero overhead). The first `onChange(handler)` makes the loader's frame-drain poll watch that
plugin's file; on a content change the host re-materializes and fires the handler with the new config —
**no plugin reload**.

```typescript
import { config } from "@s2script/config";
export function onLoad(): void {
  console.log("[demo] onLoad — greeting=" + config.getString("greeting")
    + " maxUses=" + config.getInt("maxUses") + " enabled=" + config.getBool("enabled"));
  config.onChange((cfg) => {
    console.log("[demo] config changed — greeting=" + String(cfg.greeting) + " maxUses=" + String(cfg.maxUses));
  });
}
```

### Captured live log (de_inferno)

```
# --- first load: the host AUTO-GENERATES addons/s2script/configs/_demo_hello.json (defaults + //-comments)
[META] Loaded 1 plugin.
[s2script] [demo] onLoad — greeting=hello from s2script maxUses=3 enabled=true
# the generated file:
#   { // bool — Feature toggle
#     "enabled": true,
#     // string — Logged on load
#     "greeting": "hello from s2script",
#     // int — Demo counter
#     "maxUses": 3 }

# --- edit the file (greeting/maxUses/enabled) → onChange fires WITHOUT a reload (no 2nd onLoad):
[s2script] [demo] config changed — greeting=live-reloaded via onChange maxUses=7 enabled=false

# --- corrupt the file → per-key degrade to defaults, server ticking, no crash:
[s2script] [demo] config changed — greeting=hello from s2script maxUses=3 enabled=true
```

Config lives on the engine-generic side: the CLI bakes the block into the manifest; the core parses it,
materializes (a pure `materialize_config`), injects `globalThis.__s2pkg_config_values` per plugin context,
and runs the `@s2script/config` prelude + the `onChange` mux + re-materialize; the shim owns the disk
(`config_read`/`config_write` ops read/auto-write the override file).

---

## Reload state-handoff (Slice 5E.3)

A hot-reloaded plugin can carry runtime state from its old instance to its new one. On a same-id
file-watch **Reload**, the old instance's `onUnload()` may return a `State` object; the host holds it
across the teardown→load gap and passes it to the new instance's `onLoad(prev)`. A file edit no longer
wipes in-memory state.

```typescript
interface State { reloads: number; pawn: EntityRef | null; }
let reloads = 0; let pawn: EntityRef | null = null;
export function onLoad(prev?: State): void {
  if (prev) { reloads = prev.reloads; pawn = prev.pawn; }   // prev === undefined on a first load
  // ... use reloads / pawn ...
}
export function onUnload(): State { return { reloads: reloads + 1, pawn }; }
```

- **Mechanism (reuses the inter-plugin marshalling):** `onUnload()`'s return is serialized in the old
  context via `JSON.stringify` + the EntityRef replacer into a host-held `String` (survives the
  context's disposal), then revived in the new context via `JSON.parse` + the EntityRef reviver. So a
  `State` may contain any JSON value — primitives, strings, arrays, nested objects — and **live
  `EntityRef`s** (a carried `EntityRef` revives serial-gated: reads `null`/`isValid()===false` if its
  entity died during the gap, never a crash). Carry a 64-bit value as a decimal `string` (the framework
  convention — `JSON.stringify` cannot serialize a `BigInt`, so a `bigint` in `State` silently discards
  the whole handoff).
- **Trigger:** any same-id **Reload** hands off (the author owns state-shape migration across versions,
  like config). A first **Load** → `onLoad(undefined)`. A final removal (**Vanished**) discards the
  captured state — a re-add starts fresh. `shutdown` clears everything.
- **Degrade-never-crash:** `onUnload` throws / returns a non-serializable value → no handoff + a WARN;
  a throwing `onLoad(prev)` → the existing WARN. Consume-once: the blob is dropped on load regardless.
- **Boundary:** entirely engine-generic core (`v8host.rs` capture/revive + a `PENDING_HANDOFF` map;
  `loader.rs` Reload-consume vs Vanished-clear). No shim/native/op change — one sniper rebuild (core).

### Captured live log (de_inferno)

```
# first load — no prior state
[demo] onLoad — reloads=0 hadPrev=false pawnAlive=null
# touch the .s2sp (Reload): onUnload hands off, onLoad restores — the counter CLIMBS (state survived)
[demo] onUnload — handing off reloads=1
[demo] onLoad — reloads=1 hadPrev=true pawnAlive=true pawnRef=732/69664   # a tracked pawn EntityRef survives the gap, LIVE + serial-gated
[demo] onUnload — handing off reloads=2
[demo] onLoad — reloads=2 hadPrev=true pawnAlive=true pawnRef=732/69664
# delete the .s2sp (Vanished) then re-add it (Load): the pending state was cleared → fresh identity
[demo] onUnload — handing off reloads=3
[demo] onLoad — reloads=0 hadPrev=false pawnAlive=null
```

`RestartCount=0`, server ticking throughout — a broken/absent handoff degrades to `onLoad(undefined)`,
never a crash.

---

## Commands + chat — the command spine (Slice 6.1)

The first rung of the base-plugin suite (Slice 6). A plugin registers a **server command** and receives a
typed dispatch context; a new `@s2script/chat` module carries messages to player chat. Proven by
`@s2script/basecommands` exposing `sm_say`.

```typescript
import { Commands } from "@s2script/commands";
import { Chat } from "@s2script/chat";

Commands.register("sm_say", (ctx) => {
  // ctx.callerSlot (-1 = server console), ctx.args, ctx.argString, ctx.reply(msg)
  if (!ctx.argString.trim()) { ctx.reply("Usage: sm_say <message>"); return; }
  Chat.toAll("[SM] " + ctx.argString.trim());
});
```

- **`@s2script/commands`** (engine-generic): `Commands.register(name, handler)`; the handler's `ctx` is
  `{ callerSlot, args, argString, reply }`. `callerSlot` is a raw slot (`-1` = server console) so the
  module never depends on `@s2script/cs2`; `reply` routes to the caller (console → server print; a
  player → their chat). Dispatch is owner-tracked and runs the handler in the **registering plugin's
  context** (liveness-gated, re-entrancy-safe — mirrors the game-event dispatch); a plugin's commands
  are dropped on unload.
- **`@s2script/chat`** (engine-generic): `Chat.toSlot(slot, msg)` / `Chat.toAll(msg)` (loops live slots
  via `__s2_client_valid`, not `Player.all()`).
- **Command registration RE (the cracked blocker):** CS2 does **not** export `ConCommand::Create` (the
  seed was neutralized on that). The working path is `ICvar::RegisterConCommand(ConCommandCreation_t&)`
  — a vtable call on the already-resolved `VEngineCvar007`; the shim fills a `ConCommandCreation_t` whose
  callback is the shared trampoline and stores the returned `ConCommandRef` (name-lifetime anchor +
  idempotent, reload-safe).
- **`scripts/rcon`** — a dependency-free Source RCON client for command delivery (`scripts/rcon "sm_say
  hi"`); it also unblocks the `bot_quota`/`bot_kick` gates that earlier slices worked around.

### Captured live log (de_inferno)

```
[s2script] interface OK: EngineCvar (VEngineCvar007)
[s2script] ConCommand 'sm_say' registered (accessIdx=823)     # registration works (ICvar::RegisterConCommand)
[s2script] [basecommands] onLoad — sm_say registered
# scripts/rcon "sm_say hello world":
[s2script] [basecommands] sm_say by slot=-1 msg=hello world    # register → trampoline → owner-context dispatch → ctx → handler
```

**Deferred to 6.1b — the actual chat *send*.** `Chat.toSlot`/`toAll` reach the shim's `client_print`,
which resolves + holds `IGameEventSystem` + `INetworkMessages` but is currently a degrade-safe **stub**:
the concrete `CUserMessageSayText2` protobuf type is not in the vendored hl2sdk, so the `SayText2`
user-message send needs protobuf **reflection** (the generic runtime *is* vendored, but linking it risks
the same undefined-symbol `dlopen` breakage 5D.1 hit with tier1) or a hand-serialized wire encoding —
a focused follow-up. Until then `sm_say` registers + dispatches + logs; `Chat.toAll` loops the live slots
and no-ops the send. The `basecommands` plugin needs **no change** once 6.1b lands the send.

---

## Known findings / constraints

**Config auto-generate needs a container-writable configs dir (Slice 5E.2 live-gate finding).** The
`.s2sp` addon is bind-mounted `:ro` so the host owns the plugins dir while the container only reads it.
But config **auto-generate** (`config_write`) and admin edits require **container writes** to
`addons/s2script/configs/`. The fix is a nested read-write mount layered over the `:ro` parent (Docker
resolves nested mounts by longest target path):
`../dist/addons/s2script/configs:…/addons/s2script/configs` (no `:ro`). Under a fully read-only addon the
write silently fails (degrade-safe: defaults-only, no crash), but the file is never generated — so a
production deploy must keep the `configs/` subtree writable. A related bug this gate surfaced: the
`@s2script/config` module must expose its API under the **named `config` export** (`__s2pkg_config =
{ config: … }`) to match the `.d.ts` (`import { config }`) — the in-isolate test missed it by reading the
module object directly instead of through the named-export indirection the bundler emits.

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
