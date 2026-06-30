# Slice 0 — Boot Handshake — Design Spec

- **Project:** s2script (a TypeScript plugin framework for Source 2 games; SourceMod's spiritual successor)
- **Date:** 2026-06-30
- **Status:** Approved design, ready for implementation planning
- **Scope:** Slice 0 only. The full architecture lives in the project's durable record (`docs/ARCHITECTURE.md`, a Slice 0 deliverable). This spec covers the first risk-ordered vertical slice and nothing past it.

---

## 1. Purpose & the single risk retired

Prove exactly one load-bearing assumption: **a V8 isolate can be hosted inside a live CS2 dedicated server via Metamod:Source, and torn down cleanly without restarting the server.** Retiring this answers "can I host a JS engine inside Source 2 via Metamod at all" before any framework breadth is built on top of it.

Everything else in the architecture (multiplexer, schema pipeline, lifecycle/ledger, inter-plugin layer, registry, base plugins) is deliberately deferred. This slice is ~a thin thread through the lowest layer only.

## 2. Decided forks (and why)

Two genuinely-open decisions were resolved before scaffolding:

1. **Rust core from day one** (the architecture doc's primary stack), not C++-first.
   - Rationale: `rusty_v8` ships **prebuilt static V8** (no multi-hour V8-from-source build); no throwaway V8-host code that a later slice would rewrite; the engine-generic core/game boundary is correct from the first commit. Boot risk is still de-risked by a sequencing checkpoint (§12, step 2): a minimal empty shim must load on a real server *before* V8 is added.
2. **Dockerized CS2 verification** via the `joedwards32/cs2` image (already present locally), driven by the operator. Claude cannot run a CS2 game server; live load/unload verification is an exact checklist the user executes.

## 3. Components

Three top-level units, with the engine-generic/per-game boundary baked in from the start:

- **`shim/` — C++ Metamod:Source plugin (`ISmmPlugin`), engine-generic.**
  Owns the MM:S plugin lifecycle (`Load`/`Unload`/`Pause`/`Unpause`/plugin info); reads gamedata; acquires Source 2 engine interfaces; provides the logger function; calls the Rust core over the C ABI. The only C++ in the project. Links `libs2script_core`, hl2sdk (`cs2`), and metamod-source (`dev`).

- **`core/` — Rust `cdylib` (`libs2script_core.so`), engine-generic.**
  Embeds V8 via the `v8` crate (rusty_v8); owns the V8 platform/isolate/context and the injected `console.log`; exposes the 3-function C ABI. **Has zero dependency on any `games/*` crate, enforced by CI.**

- **`games/cs2/` — empty placeholder Rust crate.**
  Carries no Slice 0 code. Exists only to establish the per-game directory and give the boundary CI check a real target. The Source 2 interfaces acquired in Slice 0 are engine-generic facilities, not CS2-specific, so nothing game-specific belongs here yet.

## 4. Boot & teardown sequence (data flow)

```
CS2 server start
  → gameinfo.gi SearchPath injection loads Metamod:Source
  → Metamod loads the shim (s2script.so) as an ISmmPlugin
  → shim.Load():
       read gamedata/core.gamedata.jsonc            (interface version strings as data)
       acquire Source2Server / SchemaSystem / ICvar / NetworkServerService
           → log success OR a named warning PER interface (missing = non-fatal)
       s2script_core_init(logger_fn)                 FFI → Rust:
           → init V8 platform (exactly once, guarded) + create Isolate + Context
           → install global console.log → logger_fn
       s2script_core_eval("console.log('hello from V8 in CS2')")   FFI → prints to server console
  → `meta list`   shows "s2script vX"
  → `meta unload` → shim.Unload() → s2script_core_shutdown()   (dispose Isolate + Context only)
  → `meta load`   → boots again, no server restart
```

**Independence rule:** interface acquisition and V8 embedding are decoupled. If gamedata is missing or an interface fails to resolve, V8 still boots and `console.log` still fires. This keeps the boot-risk answer clean of the high-churn interface layer.

## 5. The sharp unknown — V8 lifecycle across MM:S unload/reload

This is the one real design judgment call in Slice 0, and acceptance criterion #5 ("subsequent `meta load` works without restart") tests it directly.

V8's `Platform` is a **process-global, initialize-once** object. If Metamod `dlclose`s our library on `meta unload` and `dlopen`s it fresh on `meta load`, a disposed-then-reinitialized platform is a known crash class.

**Posture:**
- `core` is a **resident cdylib**: `libs2script_core.so` is built with `-Wl,-z,nodelete` (or `dlopen`'d with `RTLD_NODELETE`) so it stays mapped for the process lifetime even when the C++ shim is unloaded.
- `s2script_core_init` initializes the V8 platform **exactly once** (guarded), then creates a fresh Isolate + Context.
- `s2script_core_shutdown` disposes **only the Isolate + Context — never the platform.**
- A reload reuses the still-live platform on the resident library.

Validating this against the Docker server is Slice 0's real job. If Metamod's unload semantics diverge from this model, **that finding is itself a primary deliverable** of the slice and informs Slice 1+.

## 6. C ABI surface (3 functions + 1 callback)

```c
typedef void (*s2_log_fn)(int level, const char* utf8_msg);

int  s2script_core_init(s2_log_fn logger);    // 0 = ok; platform init is idempotent/guarded
int  s2script_core_eval(const char* utf8_js); // 0 = ok; JS exceptions caught, logged, return non-zero
void s2script_core_shutdown(void);            // dispose isolate + context; safe to call twice
```

- **No panic may unwind across the FFI boundary.** Every Rust entry point is wrapped in `catch_unwind` and returns an error code.
- The C header `s2script_core.h` is committed (cbindgen-generated or hand-written; the surface is 3 functions).
- `level` on the logger is forward-looking (info/warn/error); Slice 0 may route everything through a single level.

## 7. gamedata & degradation posture

- `gamedata/core.gamedata.jsonc` holds the engine **interface version strings** as data (`Source2Server001`, `SchemaSystem_001`, plus the cvar and network-server-service strings, confirmed against the live binary at build time). **No interface string is hardcoded in C++ or Rust** — this is the first instance of "layout/strings are data, semantics are code."
- Degrade per-step, never crash the server:
  - missing/failed interface → named warning, continue;
  - malformed or missing gamedata → named error, skip interface acquisition, still boot V8;
  - V8 init failure → loud named error; the plugin still appears in `meta list` with the failure logged (so the failure is diagnosable, not silent).

## 8. Repo layout

```
Cargo.toml                  # workspace: core, games/cs2
Makefile                    # top-level: build core + shim, package addon, docker-test
CLAUDE.md                   # architecture §5 guardrails (Slice 0 deliverable)
README.md                   # full reproduce-from-scratch runbook
docs/
  ARCHITECTURE.md           # architecture §1–3 (Slice 0 deliverable)
  superpowers/specs/2026-06-30-slice-0-boot-handshake-design.md  # this spec
shim/
  CMakeLists.txt
  src/                      # ISmmPlugin implementation (.cpp/.h)
  include/s2script_core.h   # C ABI header consumed by the shim
core/
  Cargo.toml                # cdylib; dep: v8; MUST NOT depend on games/*
  src/{lib,ffi,v8host}.rs
games/cs2/
  Cargo.toml  src/lib.rs    # empty placeholder
gamedata/core.gamedata.jsonc
third_party/                # submodules
  hl2sdk/                   # branch cs2, pinned, patch-capable
  metamod-source/           # dev (2.0) branch
scripts/check-core-boundary.sh
.github/workflows/ci.yml    # Linux build + boundary check
```

## 9. Build & the one-way boundary check

- Top-level `Makefile`: `cargo build` (core → `libs2script_core.so`) → CMake (shim links libcore + hl2sdk + MM:S → `s2script.so`) → package the addon directory.
- `scripts/check-core-boundary.sh` uses `cargo metadata` to assert there is **no dependency path from `core` to any `games/*` crate**; wired into CI. This is the mechanical enforcement of "core is engine-generic; games are packages; dependencies point one way."
- `make` chosen over `just` to avoid the missing-`just` install dependency; a `justfile` is an optional later convenience.
- Linux x86-64 is the only build target this slice. Windows is a documented TODO.

## 10. Verification — Dockerized CS2 (operator-driven)

A `make docker-test` target plus a README runbook against `joedwards32/cs2`:

- Mount the built addon dir + Metamod into the container; inject the `gameinfo.gi` SearchPath line; register the shim via `s2script.vdf`; start with `sv_lan 1`.
- No GSLT and no joining players are required — every Slice 0 check is console-side (boot logs, `meta list`, `meta load`, `meta unload`).
- Runbook asserts, in order: per-interface acquisition logs appear → `hello from V8 in CS2` appears → `meta list` shows the plugin with a version string → `meta unload` does not crash the server → `meta load` boots it again without restart.

## 11. Testing strategy

- **Automated (`cargo test`, no CS2 required):** init core with a capturing test logger → eval a script that calls `console.log` → assert the captured output → shutdown → re-init (exercises the platform-reuse path from §5). This carries the real automated regression weight.
- **CI:** Linux build of both artifacts + the `core ↛ games/*` boundary check.
- **Manual:** the §10 Docker runbook (operator-driven).

## 12. Acceptance criteria (Slice 0 definition of done)

1. A Metamod:Source plugin implementing `ISmmPlugin` (against hl2sdk `cs2`) builds for Linux x86-64.
2. It loads on a live CS2 dedicated server, shows in `meta list` with a version string, and unloads via `meta unload` without crashing the server.
3. On load it acquires and logs success/failure for the core Source 2 interfaces (`Source2Server`, `SchemaSystem` / `SchemaSystem_001`, `ICvar`/cvar system, network/engine server service); a missing interface logs a named non-fatal error.
4. It embeds a V8 isolate, creates one context, and executes a hardcoded JS string calling an injected `console.log(msg)` that routes to the server console; output appears in the console.
5. Clean teardown: unload disposes the V8 isolate/context and leaves no dangling state; a subsequent `meta load` works without a server restart (validated against the §5 posture).
6. All build steps are reproducible from a documented `README` (hl2sdk + MM:S checkout/branch/patch workflow, build commands, where to drop the `.so`, the `gameinfo.gi` edit, the Docker runbook).

## 13. Out of scope (TODOs, explicitly not built this slice)

Multiplexer / any detour or hook; schema **walking** (the `SchemaSystem` interface is *acquired* but never walked); entity wrappers/handles; lifecycle/ledger; `.s2sp` format; inter-plugin layer; registry; base plugins; TS transpile/swc (Slice 0 evals a hardcoded JS string); config/convar systems; threadpool/async; Windows build. Note later needs as TODOs and stop at Slice 0.

## 14. Deliverables

- Scaffolded Cargo workspace + CMake shim build with the engine-generic/per-game boundary in place.
- hl2sdk (`cs2`) and metamod-source (`dev`) vendored as pinned, patch-capable submodules, with the patch workflow documented.
- A minimal empty-shim-loads-on-server checkpoint completed **before** V8 is added.
- The Rust V8 core + 3-function C ABI + committed C header.
- `gamedata/core.gamedata.jsonc` with interface version strings.
- `scripts/check-core-boundary.sh` + CI wiring (Linux build + boundary check).
- `docs/ARCHITECTURE.md` (architecture §1–3) and `CLAUDE.md` (architecture §5) committed.
- README reproduce-from-scratch runbook including the Docker verification harness.

## 15. Open items to validate during implementation

- Exact interface **version strings** and the precise factory used for each (notably `SchemaSystem`, often obtained via the `schemasystem` module factory rather than the engine factory) — confirmed against the live CS2 binary and CounterStrikeSharp's `mm_plugin.cpp` reference; recorded in gamedata, not code.
- Metamod's actual `dlclose`/`dlopen` behavior on `meta unload`/`meta load` vs. the §5 resident-cdylib model.
- The `joedwards32/cs2` image's game directory layout for mounting the addon dir and patching `gameinfo.gi`.

## 16. Reference material (read, don't reinvent)

- CounterStrikeSharp (`roflmuffin/CounterStrikeSharp`) — `src/mm_plugin.cpp`, canonical `ISmmPlugin` + interface acquisition.
- SwiftlyS2 (`swiftly-solution/swiftlys2`) — open C++ core: `src/engine`, `src/scripting`, `src/sdk`.
- Metamod:Source (`alliedmodders/metamod-source`, `dev`) — loader; `gameinfo.gi` injection.
- hl2sdk (`alliedmodders/hl2sdk`, `cs2`) — engine headers.
- `rusty_v8` (the `v8` crate) — isolate/context/native-callback embedding.
