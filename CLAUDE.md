# s2script — standing conventions & guardrails

- **The core owns every engine touchpoint.** Plugins never get raw detours; they get named, typed, multiplexed events + the single `HookResult` contract. Only exception: the explicit `unsafe` module.
- **Core is engine-generic; games are packages. Dependencies point one way: game → core, never core → game.** The core knows Source 2, never a specific game. Game classes, gamedata, descriptor bindings, team/weapon APIs live in `@s2script/cs2` (and future `@s2script/<game>`). A CI check fails the build if core/std imports a game package. Litmus test for any code: *would it still be true on a different Source 2 game?* If no → it's a game package, not core.
- **Never expose a raw pointer or raw cross-plugin reference across time.** Entities, shorter-than-plugin resources, and inter-plugin interfaces are handle/proxy-backed and host-invalidated; safe accessors return `T | null`; raw-live views are block-scoped and cannot cross `await`. Entity refs on the inter-plugin wire use the same `EntityRef`/`T | null` type as the entity system.
- **Cross-plugin comms are typed, versioned interfaces.** Methods = natives, events = forwards, one object, semver-governed. Hard deps return a proxy that throws on producer-unload; optional deps return `Interface | null`. All imports ledgered; unload resolves in reverse-dependency order.
- **`package.json` is the authoring format; reuse npm standards.** Standard fields for what npm models; the `s2script` block for engine facts (`apiVersion`, `publishes`, `pluginDependencies`/`optionalPluginDependencies`, `requiresGamedata`, `permissions`, `config`). `dependencies` = npm build-deps only; inter-plugin deps under `s2script`. Never overload npm's `exports`. The runtime consumes a derived minimal manifest baked into the `.s2sp`, never the full `package.json`.
- **npm scope taxonomy, one reserved official scope.** `@s2script/*` = first-party (engine-generic per-capability module packages `@s2script/entity`/`frame`/`timers`/`console`/`interfaces`; per-game `@s2script/cs2`/`@s2script/<game>` which ship that game's schema types; base plugins). `@<community>/*` = verified third-party; unscoped allowed. Never name a package `@s2script/core` ("core" is the native layer). Reserve `@s2script` everywhere from day one.
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

## Commands

**Build & test (host — for local dev/CI only; host glibc is too new to load on the server):**
```bash
make all          # core + shim + package
make core         # cargo build --release — Rust cdylib, embeds V8 149.4.0 (first run downloads ~130MB prebuilt)
make shim         # cmake build of the Metamod C++ shim (links core)
make package      # scripts/package-addon.sh → assembles dist/addons/s2script
cargo test -p s2script-core   # core unit + in-isolate suite (forced single-threaded via .cargo/config.toml — do not pass --test-threads)
```

**Gate suite (run before every PR) — these ARE the CI jobs:**
```bash
make ci           # both suites
make ci-native    # scripts/ci-native.sh — boundary + nameleak + sigscan + licenses, cargo build/test, shim
make ci-js        # scripts/ci-js.sh — codegen freshness, plugin typecheck, activity/antiflood/gate tests
```
`.github/workflows/ci-native.yml` and `ci-js.yml` each run one of those two scripts and nothing
else, so **local green means CI green** and a new gate is added to the script, never to the YAML.
`npm ci` (the `package-lock.json` drift guard) is CI-only — run `CI=1 make ci-js` to include it.
`make check-boundary` still runs the core→games boundary check on its own.

**Server ("sniper") build — the ONLY deployable binaries.** The server is Steam Runtime 3 (Debian bullseye, glibc 2.31); host builds need too-new GLIBC and won't load. Build inside a `rust:bullseye` container:
```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
# → s2script.so (GLIBC_2.14) + libs2script_core.so (GLIBC_2.30), repackaged into dist/
```

**Plugins:**
```bash
npx s2script build            # from a plugin dir → dist/<id>.s2sp
./scripts/build-base-plugins.sh
```

**Live gate (Docker CS2 dev server):**
```bash
make docker-test                                          # docker compose -f docker/docker-compose.yml up
docker exec s2script-cs2 /patch-gameinfo.sh              # re-inject Metamod SearchPath (EVERY CS2 update wipes gameinfo.gi)
docker compose -f docker/docker-compose.yml restart cs2  # re-bind addon dir + reload (NOT --force-recreate — that resets gameinfo.gi)
python3 scripts/rcon.py "<console command>"              # drive the running server (127.0.0.1:27015, pw s2script)
```

**Release:**
```bash
./scripts/package-release.sh                                        # runtime zip (binaries + base plugins)
npm run changeset && npm run version-packages && npm run release    # @s2script/* npm types + CLI (changesets)
```

## Ship one PR per slice

**A slice is one branch and one PR.** Plain `git` + `gh pr create`, squash-merged. The PR is as
big as the slice is — don't split a slice into a chain of dependent PRs, and don't batch two
slices into one. Graphite and stacked PRs are retired; there is no `gt`.

Branch naming: `<area>/<terse-change>` — e.g. `ci/consolidation`, `docs/readme-front-door`.

A PR must be **atomic**: it passes `make ci` and is safe to merge on its own. A signature change
that breaks every caller lands WITH its callers.

A pre-merge gate is not optional even for a one-line change: a push to `main` auto-fires
`changesets.yml`, which publishes to npm. There must be a gate between a bad commit and the registry.

PR bodies need **Why** — what prompted this, and how it fits. Write the body with the Write tool to
a file and `gh pr edit N --body-file`; never a heredoc, because shell escaping mangles tables and
code blocks.

## Repository layout

```
core/        Rust engine core (cdylib, embeds V8). Engine-generic — NEVER imports games/*. All engine facts flow through S2EngineOps + set_native'd natives.
shim/        C++ Metamod plugin (s2script.so). Owns every CS2/Source2 touchpoint: sigscan, SourceHooks, inline detours, protobuf reflection, vtable RTTI.
games/cs2/   CS2 game-package prelude (pawn.js + generated schema/nav accessors). CS2 field/class names live here, never in core/ or shim.
packages/    npm-published: @s2script/sdk (builtin capability .d.ts as @s2script/sdk/<cap> subpaths + the `s2s` CLI) and @s2script/cs2 (game types).
plugins/     The base-plugin suite (SourceMod parity) — ships in the runtime zip.
examples/    Demo plugins (not shipped).
plugins/disabled/  Opt-in plugins; the loader's non-recursive scan skips the `plugins/disabled/` subdir. Operators move a .s2sp up one level (into `plugins/`) to enable.
gamedata/    Regenerable engine facts: byte-signatures, offsets, schema/event/item catalogs (data, not code).
docs/        ARCHITECTURE.md · INSTALL.md · re-strategy.md · PROGRESS.md · superpowers/{specs,plans}/.
scripts/     Build, gate (check-*.sh), sniper build, rcon.py, package/release.
docker/      CS2 dev server (container s2script-cs2) + mysql/postgres sidecars.
third_party/ Vendored hl2sdk + Metamod:Source submodules (pinned, patch-capable).
```

## Documentation

- **`docs/ARCHITECTURE.md`** — the durable design (multiplexer + `HookResult`, ledger + lifecycle, schema codegen pipeline, package format, cross-plugin interfaces, the portability boundary).
- **`docs/PROGRESS.md`** — the full slice-by-slice history (the relocated running `Current state` log). Append finished-slice entries here, not to this file.
- **`docs/superpowers/{specs,plans}/`** — one design spec + one plan per slice (the authoritative per-slice detail).
- **`docs/re-strategy.md`** — the RE / gamedata doctrine: self-resolve every engine fact against *our* binary (schema-dump / byte-sig / string-xref / RTTI) or validate it at load; never a bare borrowed constant.
- **`docs/INSTALL.md`** — operator install + after-a-CS2-update steps. **`README.md`** — build-from-scratch + full Docker runbook.
- The cross-session working memory (per-slice findings, gotchas, treadmill notes) lives in the `/memory` store indexed by `MEMORY.md`, not here.

## Current state

Slices 0 → the async-network category are complete and each proven on a live CS2 server. **What's built (capability inventory):**

- **Boot & lifecycle** — V8-in-CS2 via Metamod; context-per-plugin; ledger teardown; hot-reload with `onUnload`→`onLoad` state handoff; `tsc` typecheck gate at build/reload; config materialization + `config.onChange` live-reload.
- **Engine surface** — the `OnGameFrame` multiplexer + `HookResult` collapse; tick-integrated async (`delay`/`nextTick`/`nextFrame`/threadpool `threadSleep`); game events (272-event typed catalog, `on`/`onPre`/`fire`/`fireToClient`); damage pre-hooks (`Damage.onPre`); entity I/O (`acceptInput` / `Entity.onOutput`); entity + client lifecycle listeners; `OnMapStart`.
- **Entities** — `EntityRef` (serial-gated, `T|null`); codegen'd schema accessors; `Player`/`Pawn` model + pointer-chain nav; `Vector`/`QAngle`/`@s2script/math`; create/spawn/teleport/remove + EKV-configured spawn; items (give/strip/enumerate weapons); ray tracing (`pawn.aimTrace`); coordinate zones (`@s2script/zones`).
- **Players & admin** — `Client` handle + lifecycle events; commands (`register`/`registerServer`/`registerAdmin`); colored chat + `Chat.onMessage`; host-global admin cache with flags, groups, and immunity; SM target resolution; kick/ban/slap/slay/gag/etc.
- **Async-network (off-thread on one shared tokio runtime)** — `fetch` (HTTP), WebSocket (`@s2script/ws`), DB (`@s2script/db`: SQLite + MySQL/Postgres via sqlx), raw TCP/UDP sockets (`@s2script/net`), cookies (`@s2script/cookies`).
- **Menus & votes** — `@s2script/menu` (chat + WASD center backends); adminmenu / TopMenu; basevotes / funvotes / rockthevote / nominations / nextmap.
- **Crash reporting (capture client)** — an Accelerator/Breakpad equivalent living in `shim`+`core` (runtime infra, **not** a plugin), armed at boot, engine-generic. A signal-safe breadcrumb (culprit plugin + dispatch + engine-op) + the treadmill fingerprint (gamedata `stale` + live CS2 build); three capture paths (native fault via vendored **Breakpad** minidump, fatal JS error, Rust panic — the panic `catch_unwind` used to swallow silently is now reported) → one frozen `schema_version:1` incident envelope → disk spool + **upload-on-next-boot** (multipart-with-minidump); opt-in `crashreporter.json`. Live-gate proven end-to-end. Backend + symbol pipeline are separate future cycles (spec #2/#3).

**Base plugins that ship:** basecommands · basechat · playercommands · antiflood · adminhelp · basecomm · basebans · reservedslots · basetriggers · funcommands (+ clientprefs). Opt-in under `plugins/disabled/`: nominations · rockthevote · funvotes · nextmap.

**Deferred/next queue** (as of the last logged slice): crash-reporter **backend** (spec #2: authenticated ingest → minidump symbolication against a per-build symbol store → grouping/dedup → per-operator + fleet dashboard → treadmill correlation) + the CI **symbol pipeline** (spec #3: `dump_syms` per release); dropActiveWeapon sig-resolve → typed EKV setters (Vector/Color/EHandle) → translations → zone polish (viz / in-game editor / tags). Do **not** build ahead of an agreed slice; each deferred item is logged with its reason in `docs/PROGRESS.md`.

> The running slice-by-slice detail (what each slice built, why, and its live-gate result) is in **`docs/PROGRESS.md`** and git history. This summary is meant to stay short and durable; the branch may be ahead of it — check `git log` and the `/memory` store for the very latest.

**Naming convention (locked in Slice 4):** PascalCase events + types (`OnGameFrame`, `Pawn`), camelCase functions + properties (`delay`, `nextTick`, `pawn.health`). **Inter-plugin marshalling (locked in Slice 4.5):** methods = natives, events (`on`/`off`/producer `emit`) = forwards, exposed as one proxy object; call args + event payloads cross the context boundary by **structured copy** (never a live pointer/reference); a hard dep is a proxy that throws `InterfaceUnavailable` on producer-absence, an optional dep resolves to `Interface | null`; every import is ledgered and unload walks reverse-dependency order. Payloads cross as JSON — a `BigInt` throws and silently drops the whole payload, so carry 64-bit as a decimal string; `EntityRef` survives serial-gated.

**Entity safety (re-based on the HOST'S BOOKS in E1):** liveness is decided by `entity_live`'s books — index → (host-minted id, engine serial), fed unconditionally by the entity listener and cleared at map start — never by reading the entity's own (possibly freed) memory; `EntityRef` = `{index, id}` where `id` is the host id (the engine serial never crosses to JS), resolved books-first then slot-validated in the system-owned identity chunk via `ent_resolve` (raw pointer stays in Rust, block-scoped per native), degrading to `null`/`false` on any mismatch; a dangling handle can never mint a live ref (`__s2_handle_adopt` books-gates the decode). `entity_live` and `plugin::Registry` are two separate instances of the shared `liveness.rs` `LiveTable` primitive — a plugin reload never invalidates entities and a map change never invalidates plugins. `Pawn` is `EntityRef`-backed and `Pawn.forSlot` decodes the controller's `m_hPlayerPawn` `CEntityHandle` via the same books-gated adopt path. The game-package prelude (`pawn.js`) runs in the raw context scope, **not** inside the plugin's CJS `(function(require,module,exports){…})` wrapper, so it resolves `@s2script/entity` via the `__s2require` native, never a bare `require` (a live-gate finding).
