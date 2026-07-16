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

**Gate suite (run before every PR):**
```bash
make check-boundary                     # core must NOT import games/* (== scripts/check-core-boundary.sh)
./scripts/check-plugins-typecheck.sh    # every plugin + example typechecks vs the shipped .d.ts (the 5E.1 gate)
./scripts/check-schema-generated.sh     # codegen freshness — regenerate + `git diff --exit-code`
./scripts/check-nav-generated.sh
./scripts/check-events-generated.sh
./scripts/check-csitem-generated.sh
./scripts/test-boundary-nameleak.sh
```

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

## Ship work as a stack, not a branch (Graphite)

**Default to `gt`, not `git push` + one PR.** A slice is a *stack* of small PRs, each independently reviewable. Only ~24% of PRs over 1000 lines get any review comment at all — a 40-file PR is not reviewed, it is rubber-stamped, and agentic work reaches that size fast.

**Plan the stack before writing code.** After the plan exists and before the first edit, map the slice's building blocks in dependency order and make one task per intended PR. This is the step that produces good boundaries; deciding them afterwards means re-cutting commits.

**What earns its own PR:**
- **Atomic** — passes the gate suite and is safe to merge *on its own*. This is the binding constraint: a signature change that breaks every caller must land WITH its callers, in one PR. Don't split what CI can't verify separately.
- **Narrow scope** — one module, or one mechanical change across many.
- **Small.** No change is too small. A tiny PR buys clarity for the big one above it. Always argue for more PRs, never fewer.

```bash
gt ls                                  # see the stack
gt create <stack>/<change> -m "msg"    # new PR on top (after `git add`)
gt modify -m "msg"                     # amend the current one
gt up / gt down / gt top / gt bottom   # navigate
gt restack                             # rebase the stack on trunk
gt submit --no-interactive             # push the whole stack
```

Branch naming: `terse-stack-name/terse-change` (e.g. `contract-grammar/publishes-grammar`, `contract-grammar/host-injected-version`).

Run the gate suite **per PR**, not once at the top — an atomic PR that only passes with its children isn't atomic.

PR bodies need **Stack Context** (what the whole stack is for) and **Why** (what prompted this piece, how it fits). Write the body with the Write tool to a file and `gh pr edit N --body-file` — never a heredoc; shell escaping mangles tables and code blocks.

In a worktree the branch usually starts untracked (`gt branch info` errors): `gt track -p main` first, then create the stack.

## Repository layout

```
core/        Rust engine core (cdylib, embeds V8). Engine-generic — NEVER imports games/*. All engine facts flow through S2EngineOps + set_native'd natives.
shim/        C++ Metamod plugin (s2script.so). Owns every CS2/Source2 touchpoint: sigscan, SourceHooks, inline detours, protobuf reflection, vtable RTTI.
games/cs2/   CS2 game-package prelude (pawn.js + generated schema/nav accessors). CS2 field/class names live here, never in core/ or shim.
packages/    npm-published types + CLI: @s2script/<capability> (.d.ts) and @s2script/cli.
plugins/     The base-plugin suite (SourceMod parity) — ships in the runtime zip.
examples/    Demo plugins (not shipped).
disabled/    Opt-in plugins; the loader skips top-level `disabled/`. Operators move a .s2sp up one level to enable.
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

**Base plugins that ship:** basecommands · basechat · playercommands · antiflood · adminhelp · basecomm · basebans · reservedslots · basetriggers · funcommands (+ clientprefs). Opt-in under `disabled/`: nominations · rockthevote · funvotes · nextmap.

**Deferred/next queue** (as of the last logged slice): dropActiveWeapon sig-resolve → typed EKV setters (Vector/Color/EHandle) → translations → zone polish (viz / in-game editor / tags). Do **not** build ahead of an agreed slice; each deferred item is logged with its reason in `docs/PROGRESS.md`.

> The running slice-by-slice detail (what each slice built, why, and its live-gate result) is in **`docs/PROGRESS.md`** and git history. This summary is meant to stay short and durable; the branch may be ahead of it — check `git log` and the `/memory` store for the very latest.

**Naming convention (locked in Slice 4):** PascalCase events + types (`OnGameFrame`, `Pawn`), camelCase functions + properties (`delay`, `nextTick`, `pawn.health`). **Inter-plugin marshalling (locked in Slice 4.5):** methods = natives, events (`on`/`off`/producer `emit`) = forwards, exposed as one proxy object; call args + event payloads cross the context boundary by **structured copy** (never a live pointer/reference); a hard dep is a proxy that throws `InterfaceUnavailable` on producer-absence, an optional dep resolves to `Interface | null`; every import is ledgered and unload walks reverse-dependency order. Payloads cross as JSON — a `BigInt` throws and silently drops the whole payload, so carry 64-bit as a decimal string; `EntityRef` survives serial-gated.

**Entity safety (locked in Slice 5A):** entity refs are `EntityRef` = `{index, serial}` — a raw pointer never crosses to JS; every access resolves the pointer **and** compares the slot's live serial to the captured serial in a single lookup (no TOCTOU), degrading to `null`/`false` on mismatch; `Pawn` is `EntityRef`-backed and `Pawn.forSlot` decodes the controller's `m_hPlayerPawn` `CEntityHandle` via `__s2_handle_decode`. The game-package prelude (`pawn.js`) runs in the raw context scope, **not** inside the plugin's CJS `(function(require,module,exports){…})` wrapper, so it resolves `@s2script/entity` via the `__s2require` native, never a bare `require` (a live-gate finding).
