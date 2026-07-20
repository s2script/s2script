# s2script — Architecture

> **Project:** `s2script` (domain: `s2script.com`). Plugin package extension: **`.s2sp`**. Authoring format: **`package.json`** (with an `s2script` namespace block). First-party scope: **`@s2script`** (engine-generic std lib: `@s2script/std`; per-game packages: `@s2script/cs2`, …).
>
> This document is the durable architecture record (Sections 1–3 of the design). The standing guardrails live in `CLAUDE.md`. The active build is tracked under `docs/superpowers/specs/` and `docs/superpowers/plans/`.

---

## 1. What we're building

A TypeScript plugin framework for **Source 2 engine games** (Counter-Strike 2 first), loaded via Metamod:Source, that is to Source 2 what SourceMod was to Source 1: the **single, unified runtime** that every server plugin loads into. Plugin authors write TypeScript against one standard library; the framework owns every engine touchpoint and multiplexes all plugins onto it. The **core is engine-generic** (knows Source 2, not any specific game); game-specific knowledge (CS2's classes, gamedata, APIs) lives in scoped per-game packages (`@s2script/cs2`). Around it sits a package registry and typed-interface distribution service (`s2script.com`) that is to Source 2 what AlliedModders + npm together are: one-command plugin installs for communities, and typed cross-plugin APIs for developers. A first-party plugin suite (`@s2script/base*`) mirrors the SourceMod base plugins so an established server can migrate and feel at home day one.

The existing CS2 frameworks (CounterStrikeSharp, ModSharp, SwiftlyS2) each solve most of the engine-access problem, but they **compete** — each ships its own detour engine, its own command registrar, its own chat hook, and they fight over the same engine functions (see Metamod:Source issue #215). The thing that made SourceMod feel good was never the Pawn language; it was that SourceMod was the *sole arbiter* — one `RegConsoleCmd`, one event system, one `Action` composition contract that thousands of uncoordinated plugins obeyed. **That unification is the product.** TypeScript + V8 is the modern delivery vehicle for it.

### The core principles (everything derives from these)

1. **The core owns every engine touchpoint and exposes one composition contract.** Exactly one detour per engine function, owned by the core, fanned out to N plugins via a single `HookResult` contract. Plugins compose instead of compete. The inversion of the current CS2 free-for-all.
2. **Unsafe-across-time is made unrepresentable, not discouraged.** A raw pointer held across a tick is a use-after-free waiting to happen — and so is a reference to another plugin held across its unload. The API makes holding either *impossible*: you hold validated handles/proxies the host can invalidate, and the type system compels you to confront staleness at compile time.
3. **Layout churn is regenerable data; semantics are durable code.** Offsets, signatures, and struct field positions move every game update — they live in auto-dumped, validated data files. Behavioral facts live in human-reviewed code. A routine update is a CI regeneration with no human in the loop.
4. **Contracts are typed and versioned — engine, host, and plugin-to-plugin.** The schema-derived `.d.ts`, the host `apiVersion`, and every published inter-plugin interface are versioned artifacts under semver. Breaking a contract is a major bump that fails fast and legibly, never a silent runtime drift (SourceMod's include-file footgun).
5. **Nothing is a foreign concept to a JS dev.** Reuse the ecosystem's standards (`package.json`, npm scopes, semver, `.d.ts`) wherever they already model what we need; only invent where the engine domain has no equivalent, and isolate those inventions under one namespace.
6. **Core is engine-generic; games are packages.** The core knows *Source 2*, never a specific game. Everything true only of one title (its schema classes, gamedata, descriptors bound to its functions, its team/weapon APIs) lives in a per-game package (`@s2script/cs2`). Dependencies point **one way only**: game packages depend on core; **core never imports a game package.** This is how SourceMod supported many games, and it's what lets a future Source 2 title (e.g. Deadlock) be a new package rather than a rewrite.

> **The honest framing for prioritization:** the architecture is ~30% of whether this succeeds. The other ~70% is the maintenance treadmill — keeping gamedata green within ~48h of every game patch, forever — plus the unified std lib, the registry, and the base-plugin parity suite that make plugins compose, spread, and migrate. Those underserved things are the moat, not the cleverness.

---

## 2. Architecture (the durable design)

### 2.0 Foundational dependency: hl2sdk (first-class, and itself part of the treadmill)

Every Source 2 framework builds against **AlliedModders' `hl2sdk`** (branch per game, e.g. `cs2`) — the community C++ headers for the Source 2 engine. Valve ships **no** official Source 2 SDK, so hl2sdk is *community-maintained and incomplete*, making it a first-class dependency **and** a first-class maintenance risk. Load-bearing in three places: (1) the C++ shim links against it for `ISmmPlugin`, the engine interface types (`ISource2Server`, `ISource2GameClients`, `INetworkServerService`, `ICvar`, `ISchemaSystem`, …), and core types (`CBaseEntity`, `CEntityHandle`/`CHandle<T>`, `CEntityInstance`, `CUtlVector<T>`); (2) the schema dumper walks structures whose C++ shape hl2sdk describes; (3) gamedata semantics are anchored to the SDK's view of those interfaces.

**The risk to write down:** when a game updates, hl2sdk *lags* (sometimes days). You will need to carry local patches ahead of upstream. Treat it as a pinned, vendored, patch-capable submodule, tracked in the update-day fire drill. **Posture:** lean on hl2sdk for stable interface/ABI types, but own your schema/offset layer entirely (don't trust the SDK to have a class's fields right — get those from your own live dump). Note hl2sdk's engine-generic vs per-game header split mirrors §2.0.5: the engine interface headers are core's concern; the game-specific bits inform the game package's gamedata.

### 2.0.5 Engine-generic core vs per-game packages (the portability boundary)

The most important structural rule, baked in from the start. Three layers:

**Layer 1 — Engine-generic core (knows "Source 2," nothing about any game).** True of *any* Source 2 title: the V8 host, isolate/context model, tick-integrated async; the multiplexer machinery + `HookResult` contract (the *mechanism*); `ISchemaSystem` access (a Source 2 facility every game has); the entity-system model — `CEntityInstance`, `CGameEntitySystem`, `CHandle<T>`, serial validation (ECS is engine architecture, game-independent); lifecycle, ledger, `.s2sp` format, inter-plugin interface model, config/convar layering, registry; the MM:S `ISmmPlugin` load path; `resultApply`'s four mechanisms (engine-ABI facts). A generic notion of "connected client" (the engine has client slots regardless of game) lives here.

**Layer 2 — Per-game packages (`@s2script/cs2`, later `@s2script/<game>`).** Specific to one title: the game's **schema classes** (`CCSPlayerController`, `CCSPlayerPawn`, the controller/pawn split — CS2's, not universal); the game's **gamedata** (CS2 signatures/offsets for CS2 binaries); **descriptor instances** bound to the game's functions (`OnTakeDamage` *as it binds to CS2's specific function* — the machinery is core, the binding is here); **game events** (`player_death` with CS2's field shape); **game APIs** (CT/T team enums, weapons, buy menu, bomb logic). The generated schema types (§2.4) are per-game and ship in the game package.

**Layer 3 — Engine-generic std lib (`@s2script/std`) — split carefully.** Generic facilities that work on any Source 2 game: timers, the command/chat/menu *frameworks*, db, translations, the handle/`EntityRef` *primitives*, config, the targeting *engine*, engine-generic Source 2 base types. **What must NOT leak in:** game concepts like `@ct`/`@t` targeting filters (CS team concepts) or helpers assuming the controller/pawn split — those belong in `@s2script/cs2`.

**The litmus test for core-vs-game:** *would it still be true on Deadlock (a different Source 2 game)?* `CHandle` validation → yes, core. `ISchemaSystem` walking → yes, core. The multiplexer → yes, core. The four `resultApply` mechanisms → yes, core. `CCSPlayerPawn.health` → no, cs2. `@ct` targeting → no, cs2. `player_death` shape → no, cs2. *Which* function `OnTakeDamage` detours → no, cs2. Run every descriptor and every API through this question.

**The boundary mechanism — inversion of control.** Core defines interfaces a game package *implements* (the IoC SourceMod did implicitly): core provides `IGameModule`, the entity-system accessor, a player-model abstraction, an (initially empty) descriptor registry, schema-system access, the gamedata loader. `@s2script/cs2` *implements* them — registers CS2 descriptors (`OnTakeDamage`→CS2 sig), binds CS2 schema types, ships CS2 gamedata, defines CS2 team/weapon enums, provides the `CCSPlayerController`/`CCSPlayerPawn` player model. **Core never names a CS2 class; it programs against the abstraction.** A plugin targeting CS2 depends on `@s2script/cs2` (the `#include <cstrike>` analog) and gets CS2 types + APIs; a pure timer/db plugin depends only on `@s2script/std` and runs on *any* Source 2 game.

**Two honest cautions (avoid premature generalization):**
- **You have one game today, so you can't fully validate the abstraction.** The trap is baking CS2-shaped assumptions *into the abstraction itself* and feeling portable without being portable. Mitigation: **draw the package boundary firmly now** (core and `@s2script/cs2` are separate crates/packages; core never imports cs2), but **keep the `IGameModule` interface thin** and let it be reshaped when a second game actually arrives. The package split is cheap and high-value; an elaborate game-abstraction API speculated against games that don't exist is expensive and probably wrong. Split packages, minimal interface, refactor the interface when game #2 is real.
- **The controller/pawn split is CS2-specific, but *some* player model is universal.** Core has a generic "connected client" (engine client slots); the pawn/controller relationship is cs2. Slice 3's `pawn.health` lives in `@s2script/cs2` from the very first accessor.

**Enforcement (mechanical, not aspirational):** the build graph enforces the one-way dependency. Core (Rust crate + `@s2script/std`) has **zero** dependency on any game package; a CI check fails the build if `core` imports `cs2`. That single rule is what prevents CS2 assumptions leaking into core.

### 2.1 Runtime model

- **Execution:** one embedded **V8 isolate** on the game's main thread, shared with the engine. Native calls into the engine are synchronous and cheap (no marshaling boundary). Chosen over QuickJS for mature Promise/microtask integration.
- **Isolation:** **context-per-plugin inside one shared isolate** (not isolate-per-plugin). Plugins share the entity-wrapper cache and binding objects (process-global truths about game state) and cross-plugin calls are cheap; the cost is a shared heap, mitigated by per-context heap accounting to name/disable a ballooning plugin. Per-plugin unload scope comes from disposing the context.
- **Async on a single thread:** the engine calls you synchronously each tick (~15.6ms @ 64-tick). You own the microtask checkpoint: every `OnGameFrame`, after frame logic, drain the microtask queue (`PerformMicrotaskCheckpoint`). `await` resolves at controlled frame boundaries, never preempting mid-tick. Primitives: `NextTick()`, `NextFrame()`, `Delay(ms)`.
- **Genuinely blocking work** (SQL, HTTP, file I/O) goes to a **threadpool** off the main thread; the result marshals back as a resolved Promise on the next tick drain. The one place you cross threads. `const rows = await db.query(...)` runs off-thread, resumes on-main-thread. No callback hell, no manual thread-hops.

### 2.2 The multiplexer + `HookResult` contract

The heart of the system. **One descriptor and one detour per engine touchpoint** (the *machinery* is core; CS2's specific descriptor instances live in `@s2script/cs2`), installed lazily on first subscription, removed when the last subscriber leaves.

**The composition contract — one result type, defined precedence:**

```ts
enum HookResult {
  Continue, // I didn't act; my return is irrelevant
  Changed,  // I mutated params in-place; keep dispatching
  Handled,  // suppress the game's default action, but other plugins still run
  Stop,     // suppress default AND stop the chain immediately
}
```

Collapse rule (run N handlers → one decision): track the **max** result by precedence (`Continue < Changed < Handled < Stop`). `Stop` short-circuits. `Handled` does **not** short-circuit (a later observer may still want the event). `Changed` propagates mutated params to the next handler. This short-circuit-vs-observe distinction is exactly why SourceMod's `Action` model composed across uncoordinated plugins.

**Handler ordering** is by an explicit priority ladder, never registration order: `High` (early veto — anti-cheat, god mode), `Normal` (default), `Low` (wants near-final params), `Monitor` (runs **after** the collapse is decided; gets the result but its return is ignored — the logging/stats tier, first-class so observers never accidentally influence outcomes). Within a tier: registration order, stable.

**Pre/Post phases** are orthogonal to priority. `Pre` runs before the original engine fn (can change/suppress); `Post` after (observe real outcome/return). Some data only exists post-original (final damage after armor, spawned entity pointer).

**Commands and chat are specialized multiplexers over the same machinery** (frameworks in core/std; CS2-specific bindings like `@ct` targeting in cs2):
- **Commands:** one detour on command dispatch; core parses argv once, resolves permissions once, routes by name to a *registry* (O(1) lookup). Chat-triggered (`!kick`) and console (`css_kick`) entries point at the same registry node.
- **Chat:** one detour on say; returns the same `HookResult`. Commands-in-chat peeled off to the command registry first.

**Inter-plugin events use the same machinery** (§2.9): a plugin-published event is a multiplexer descriptor whose *producer* is a plugin rather than the engine.

**The native/detour layer is core-private.** Plugins get *capabilities* (named, typed events), not raw detours. A loudly-marked `unsafe` module is the escape hatch for advanced plugins needing a raw detour the core doesn't model yet — popular ones get promoted to first-class descriptors.

**Re-entrancy & error isolation (design in from the start):** handler lists iterated over a snapshot/copy (subscribe/unsubscribe mid-dispatch can't corrupt iteration); each handler wrapped in try/catch at the core boundary (a thrower is logged, treated as `Continue`, auto-disabled after repeated offenses); per-handler timing telemetry since you can't preempt V8 mid-call.

### 2.3 `resultApply` — suppression mechanics (per-descriptor)

"Suppress the default" is not one operation. The four mechanisms are engine-ABI facts (core); which one a given descriptor uses is declared on the (cs2) descriptor. This is where per-hook reverse-engineering lives.

1. **Supercede** — block the original (`MRES_SUPERCEDE` / don't call the trampoline). You provide the return. Only safe for all-or-nothing decision functions; superceding skips *all* side effects, so you own them.
2. **Param mutation / neutralize** — let the original run with neutralized inputs (e.g. `damage = 0`). Usually safer for gameplay fns (engine state stays consistent). Requires a semantically-known no-op input.
3. **Return-value override** — let it run, rewrite the return in Post. Interacts with supercede (if Pre superceded there's no real return — Pre/Post share a per-call dispatch context).
4. **Out-param / by-ref mutation** — write into a pointer/struct param after the call; needs exact field offsets (schema/gamedata territory, high churn).

```
ResultApply { supercedable: bool; neutralize: (params)=>void | null;
              overridable: bool; defaultReturn: value | null; outParams: FieldRef[] }
```

Collapse → apply: if result ≥ `Handled` → prefer `neutralize`, else `supercede` if safe, else **refuse to expose `Handled` for this hook** (observe-only). If `Changed` → write mutated params/outParams, run original. Run Post; override return if `overridable`.

**Separation of churn:** behavioral facts (`neutralize`, `supercedable`) are durable code; layout facts (offsets, signatures, return-type width) are regenerated data. A field-offset change must never force a code change. Per-descriptor validation asserts; on failure, **disable that descriptor**, not the framework, with a named reason.

### 2.4 Schema codegen pipeline

Five stages: **dump → normalize → diff/validate → emit → load.** The pipeline *engine* is core (generic over any Source 2 game's schema); its *output* is per-game and ships in the game package.

1. **Dump** from live binaries via `ISchemaSystem` (`CreateInterface("SchemaSystem_001")`): walk type scopes, each `CSchemaClassInfo` → fields (name, type, offset), inheritance, enums, networked flags. Two contexts off one walker: **in-process** (ground truth, boot self-heal) and **offline CLI** (CI, produces shipped bindings). Model on `a2x/cs2-dumper` / SwiftlyS2 `src/sdk`. **Own this layer entirely — don't trust hl2sdk's game-class fields.**
2. **Normalize** into a stable, diffable **JSON IR** — *don't emit TS directly from the raw walk.* Resolve type strings into your own lattice (primitives, `CHandle<T>`, `CUtlVector<T>`, embedded structs, enums); flatten inheritance; tag each field networked + state-change group.
3. **Diff/validate** against the last committed IR: offset-changed/type-same → auto-accept (the bulk of churn, zero human attention); field removed/renamed/type-changed → hard-fail CI with named breaks; new class/field → additive; size-changed-without-field-changes → warn. Feeds the descriptor validation gate so stale `outParams`/field refs auto-disable their descriptors.
4. **Emit** two artifacts: (a) a compact **runtime offset table** (data — layout-only updates are a data swap, no recompile); (b) the **TypeScript `.d.ts` + accessor layer**, **per-game, published in the game package** (CS2's classes ship in `@s2script/cs2`), with the network state-change **folded into networked setters automatically** and `CHandle<T>` derefs returning `T | null` with serial validation. Engine-generic Source 2 base types live in `@s2script/std`. Bake offsets as literals (you regenerate per update anyway; literals inline well in V8).
5. **Load/self-check:** boot-time in-process dump vs shipped offset table. Match → normal. Layout-only drift on unused fields → log+continue. Drift on a used field → disable that descriptor. Wholesale mismatch → safe diagnostic mode, refuse plugins.

**The one durable human-owned piece:** the friendly-name mapping (`m_iHealth` → `health`) and the type-lattice resolver — small, judgment-based, changes only on Valve renames (caught by the diff).

### 2.5 Plugin lifecycle & the ledger

**Load sequence:** (1) typecheck gate (against the shipped `.d.ts` *and* declared dependency interfaces — catches schema-drift and inter-plugin contract breaks before runtime); (2) transpile TS→JS (swc/esbuild); (3) create context, inject std lib *bound to this plugin's identity* + resolved dependency proxies (§2.9); (4) execute top-level + `onLoad` (subscriptions tagged with plugin id); (5) mark active.

**The subscription ledger** makes unload clean — every persistent resource is auto-recorded:

```
PluginLedger { hookSubscriptions, commandRegistrations, timers, pendingAsync,
               nativeAllocations, contextRef, importedInterfaces, exportedInterface }
```

**Absolute rule:** if a std lib call grants a persistent effect, it must be ledgered. The ledger — not the plugin's cleanup code — is the teardown authority.

**Unload is deferred & phased:** (1) mark unloading → multiplexer skips its handlers, no memory touched; (2) **drain the stack** — if executing up the call stack, defer teardown to the next frame-boundary checkpoint; (3) **cancel pending async** — every in-flight `await` has a liveness guard checked before continuation (threadpool work completing post-unload drops its continuation, preventing use-after-free); (4) **resolve reverse dependencies** (§2.9); (5) walk the ledger in reverse (invalidate exported-interface proxies, unsubscribe hooks → may trigger lazy detour removal; unregister commands; kill timers; release handles); (6) dispose context.

**Hot-reload** = unload + load, with opt-in data-only state handoff (`onUnload(): SerializedState` → `onLoad(prev?)`; structured-clone-able data only, never live handles). **File-watch reload** runs the typecheck gate first — broken saves leave the running version untouched.

**Load-order determinism:** manifest-declared dependencies, topologically sorted, never filesystem order. Missing/failed dependency → refused with a clear reason. **Crash containment:** throw in `onLoad` → rolled back via partial ledger; throw in `onUnload` → force teardown via ledger anyway.

### 2.6 Handle/wrapper system (the use-after-free killer)

The #1 real-world crash: a plugin holds an entity wrapper, the entity dies (or a map
transition frees it out from under a held ref), the plugin reads it → garbage/corruption.
The pre-E1 design tried to answer "is this entity alive" by reading the *entity's own*
memory (a serial stored on the instance); a changelevel frees that memory before the
books are told, so a stale `EntityRef` dereferenced freed storage → deterministic SEGV.

**Doctrine (locked in E1): liveness is decided by the HOST'S BOOKS — populated by
engine notifications, cleared by transitions — never by reading the referent's own
memory.** The referent (an entity, a plugin) can be freed, reused, or gone; the books
are the one thing the host fully controls, so they are the one thing trusted to answer
"is this still the thing I captured".

- **`liveness.rs` — the shared primitive.** `LiveTable<K, M>`: `key → (host-minted
  monotonic id, meta)`. `insert` mints a fresh id for `key`, which *is* the invalidation
  of whatever id that key held before (a same-key re-create can never collide with a
  stale holder). The id allocator never resets — not on remove, not on `clear()` — that
  monotonicity is the anti-aliasing guarantee. Two separate instances share this module
  and stay separate tables: `plugin::Registry` (plugin generations) and `entity_live`
  (entity liveness). A plugin reload must not invalidate entities; a map change must not
  invalidate plugins — they are deliberately independent tables, not one shared map.
- **`entity_live.rs` — the entity books.** `LIVE: index → (host-id, engine_serial)`,
  thread_local (game-thread only, like every v8host-adjacent table). Fed by the shim's
  `IEntityListener` through the ffi entry, **unconditionally, before and independent of
  the JS mux dispatch** — a create/delete witnessed while JS is on-stack (e.g. a
  plugin's own synchronous `createEntity`) still updates the books even though the mux
  early-returns with no subscribers, because liveness bookkeeping must never depend on
  whether anyone happened to be listening. `onCreated`/`onSpawned` book *before* the JS
  dispatch; `onDeleted` books *after* — a handler may still read the dying,
  slot-validated entity during its own delete notification, and the ref goes dead the
  moment the FFI entry returns. The whole table is cleared at map start (the implicit
  epoch — no counter is stamped; a cleared table with a fresh monotonic allocator *is*
  the epoch boundary).
- **`EntityRef = {index, id}`.** `id` is the host-minted `LiveTable` id (u64, JS-safe as
  f64 up to 2^53 mints) — never the engine's own serial, which never crosses to JS.
- **3-stage resolution, cheapest and safest first** (`entity_resolve_ptr`): (1) THE
  BOOKS — `engine_serial_for(index, id)`; a miss returns null with zero engine memory
  touched. (2) Defense-in-depth — the shim's `ent_resolve` op re-validates the stored
  engine serial against the system-owned identity chunk (never instance memory) before
  returning the live pointer. (3) Only the calling native derefs the instance, and only
  block-scoped within that one native call — the raw pointer is used and discarded
  entirely inside Rust; it never crosses the JS boundary. Errors at any stage fall
  toward `null`, never toward a deref.
- **The map-start clear + one-shot repair sweep.** The books are the *only* liveness
  authority, so a create witnessed before the listener attaches (first-boot map,
  preallocated controllers) or before `StartupServer` completes would otherwise never
  appear. The map-start clear arms a one-shot repair sweep that fires at the first
  simulating frame: it reconciles the books against a chunk-walk snapshot of live
  identity slots (the shim's `ent_snapshot` op — system-owned memory, never an instance
  read), upserting anything the listener feed missed and evicting anything stale. This
  is a safety net over the listener feed, not a replacement for it — it runs once per
  map, not per frame.
- **The wire form.** An `EntityRef` crossing the inter-plugin structured-copy boundary
  (§2.9) or a reload state-handoff (§2.5) is tagged `{__s2ref: [index, id]}`; the
  receiving context's reviver rehydrates it into a live `EntityRef` bound to *its own*
  natives, never a plain data blob. The old engine-serial-keyed wire tag is retired —
  the host id is the only thing worth carrying across a context boundary, because it's
  the only thing the target context's books can re-validate against.
- **Minting is restricted, not free-form.** Two natives mint a books-backed ref from
  engine-side data, and both are designed so a dangling handle can never mint a live
  one: `__s2_ent_id_for_index(index)` looks up the books' current id for a slot (no id
  ⇒ not live, mint nothing); `__s2_handle_adopt(handle)` decodes a raw engine
  `CEntityHandle` and calls `entity_live::adopt`, which only returns an id when the
  handle's engine serial matches what the books currently hold for that index — a stale
  handle field read off a dead entity adopts to `null`. The public `EntityRef`
  constructor is **not** part of the typed `.d.ts` surface (a plugin cannot hand-build a
  ref); only the game-package prelude, which owns both minting natives, constructs one.
- **Resolve-on-every-touch.** The wrapper stores `{index, id}`; every access re-runs the
  3-stage resolution — validates then reads/writes. No cached "is it alive" bit outlives
  a single native call.
- **Two access tiers:** **safe accessors return `T | null`** (`player.pawn` is
  `CCSPlayerPawn | null`; `pawn.health` on a possibly-dead ref is a *compile error* —
  `IsValidEntity` enforced at compile time); **block-scoped `lock()`** for hot paths
  (resolves+validates once, guaranteed-live view for the *synchronous block only*;
  cannot be stored or cross an `await`/tick; make escaping awkward and lint-catchable).
- **Identity-derived reads (e.g. `pawn.isValid`) stay off the instance too.** CS2
  pre-allocates all 64 controller entities, so "the entity exists" doesn't mean "fully
  spawned" — SourceMod/CSSharp-sense validity also checks the engine's staging flag.
  That flag is read from the system-owned identity *slot* (`ent_identity_flags`, gated
  by the same books-first `(index, id)` check), not by walking a pointer chain into the
  instance — the E0-era `[16]→48` chain was itself a crash site on a stale ref.
- **The await hazard:** an `EntityRef` surviving `await` is *fine* (re-resolves through
  the books on next touch, safe no-op on mismatch); a `lock()` surviving `await` is
  *not* — hence block-scoping. The line is drawn exactly at the async boundary.
- **Wrapper cache** keyed by `(index, id)`: same live entity → same wrapper (reference
  identity, usable as map keys). Evicted on `OnEntityDeleted` with **core bookkeeping
  ordered before plugin dispatch** so even a death handler gets a safe dead ref.
- **Generalizes to all resources:** timers, DB connections, file handles, subscriptions,
  **and inter-plugin proxies (§2.9)** — SourceMod's `Handle` generalized, and now backed
  by the same `liveness.rs` primitive as plugin generations. The `EntityRef` *primitive*
  is core/std; the *typed `CCSPlayerPawn` wrapper* is `@s2script/cs2`.

### 2.7 Package format, authoring & file ownership

**Authoring format = `package.json`** (Principle 5). Standard npm fields for what npm models; s2script-specific facts under one `s2script` block.

```json
{
  "name": "@edge/admin-core",
  "version": "2.1.0",
  "main": "plugin.js", "types": "plugin.d.ts",
  "dependencies": { "lodash": "^4.17.0" },
  "s2script": {
    "apiVersion": "1.x",
    "publishes": "AdminAPI",
    "pluginDependencies": {
      "@s2script/std": "^1.0.0",
      "@s2script/cs2": "^1.0.0"
    },
    "optionalPluginDependencies": { "@edge/admin-menu": "^3.0.0" },
    "requiresGamedata": ["CBaseEntity::TakeDamage"],
    "permissions": ["chat", "commands", "db"],
    "config": { "name": "admin", "defaults": { "max_bans": 50 } }
  }
}
```

**Hard rules:** standard fields for what npm models (tooling/semver/muscle-memory just work); **`dependencies` = npm build-deps only** (bundled by swc); **inter-plugin deps under `s2script.pluginDependencies`/`optionalPluginDependencies`** (host-resolved as proxies — different mechanism, different field); **`publishes`, not npm's `exports`** (don't overload bundler semantics). A CS2-targeting plugin lists `@s2script/cs2` among its plugin-deps; a pure-generic plugin lists only `@s2script/std` and runs on any Source 2 game.

**Build artifact = `.s2sp` (zip), produced by `s2script build`** from the `package.json`-rooted source: reads `package.json`, bundles `dependencies` + source into one `plugin.js`, runs the typecheck, and **bakes a derived minimal runtime manifest** into the archive (id, version, apiVersion, plugin-deps, requiresGamedata, permissions, publishes, config). The runtime **never parses the dev's full `package.json`** — devDependencies/scripts/cruft never reach the server.

```
my-plugin.s2sp (zip)
├── manifest.json   # DERIVED minimal runtime manifest (not the authored package.json)
├── plugin.js       # bundled, transpiled entry
├── plugin.d.ts     # typecheck-gate input AND published interface types
├── translations/  configs/  assets/   # optional
```

**Ownership split:** the `.s2sp` is author-owned and immutable (replaced wholesale on upgrade); everything outside `/plugins` is operator-owned and persistent:

```
addons/s2script/plugins/        # drop .s2sp here (immutable)
cfg/s2script/<name>.cfg         # settings, author-named, first-run generated, operator-owned
addons/s2script/configs/<name>.jsonc   # structured config, author-named (defaults to plugin id)
addons/s2script/gamedata/*.gamedata.jsonc   # operator-droppable, version-tied, shared
```

**Drop-in flow:** watch `/plugins`; read derived manifest from the zip in-memory → validate (`apiVersion`, no id collision, plugin-deps satisfiable) → typecheck gate → materialize configs on first run (never clobber operator edits) → load. Remove → ledger teardown. **Run-from-archive**; only operator-editable config hits disk.

**Config follows the SourceMod convention:** named by plugin (author-overridable via `s2script.config.name`, the `AutoExecConfig("name")` analog), first-run materialized from declared defaults, operator-owned forever, **missing keys fall back to declared defaults at read time** (the file is never rewritten behind their back).

**Gamedata is never bundled per-plugin** — it's version-tied, game-specific, shared infrastructure (ships with `@s2script/cs2`, droppable into `/gamedata` to override). Files merge into one **validated namespace**; **operator-dropped files override core** (the hotfix path — a signature breaks, drop a corrected `.gamedata.jsonc`, every dependent plugin fixed immediately). Bad files rejected with a named error, never silently merged. Plugins **declare** `requiresGamedata` and fail-fast-and-legibly at load.

### 2.8 Config & convars (two-layer model)

- **Settings (~90%):** framework-managed. `config.define("max_rounds", { default: 15, min: 1, max: 30 })`. Materialized to `<name>.cfg`, parsed/applied by the framework. **Zero dependency on Source 2 convar internals.**
- **Real convars (opt-in):** `convar.register("rtv_enabled", { default: true })` for RCON/console/tooling visibility. Goes through actual Source 2 convar registration (subject to engine RE).

Both feed the same `<name>.cfg`; only `convar.register` touches the engine. Default path is `config.define`.

### 2.9 Cross-plugin communication — typed, versioned interfaces (forwards + natives, unified)

SourceMod split inter-plugin comms into natives (others call me) and forwards (I broadcast), both stringly-typed and runtime-discovered. s2script collapses both into **one thing: a plugin publishes a typed, versioned interface object.** Methods = "natives"; events on it = "forwards." Same machinery, type-safe, semver-governed.

**Producer:**
```ts
export interface AdminAPI {
  getImmunity(player: PlayerRef): number;
  banPlayer(player: PlayerRef, minutes: number, reason: string): Promise<void>;
  onPunishment: Event<{ target: PlayerRef; admin: PlayerRef; kind: PunishKind }>;
}
export const api: AdminAPI = { /* ... */ };
```
`package.json`: `"s2script": { "publishes": "AdminAPI" }`. On publish, the registry stores `plugin.d.ts` as the **published contract**.

**Consumer:**
```ts
import type { AdminAPI } from "@edge/admin-core";
const admin = host.require<AdminAPI>("@edge/admin-core"); // resolved proxy
admin.onPunishment.on(e => { /* forward */ });
await admin.banPlayer(player, 60, "cheating");            // native
```

- **Hard dep:** `s2script.pluginDependencies`. Host topo-sorts so the producer loads first; consumer can't load without a compatible version (fail-fast). `host.require` never returns null.
- **Optional dep:** `s2script.optionalPluginDependencies`. `host.optional<AdminAPI>(...)` returns **`AdminAPI | null`** — the type forces a null-check, and re-checks after `await` (the proxy can go null if the producer unloads).

**Use-after-free hazard → proxies, not raw objects:** if A unloads while B holds its interface, B calls a dead plugin — the entity-staleness bug across the plugin boundary. So **the host hands B a host-owned proxy** that is handle-backed and ledgered (`importedInterfaces`) and **invalidated on A's unload** — a dead hard-dep proxy throws `DependencyUnloadedError`; an optional one flips to `null`.

**Entity refs on the wire:** interface args and event payloads carrying entities use the **same handle-backed `EntityRef`/`T | null` type as §2.6** — never a raw pointer. A `PlayerRef` B receives from A obeys identical staleness rules; producer and consumer agree on what a `PlayerRef` is because it's the one shared ref type (defined in core/std, the typed game wrappers in cs2).

**Unload resolution (reverse-dependency order):** unloading A with live hard-dependents **cascades or is refused** with a named reason. Default = refuse-unless-cascade-confirmed. Optional dependents aren't a barrier (proxies flip to null). The ledger's `exportedInterface` tracks consumers so resolution is exact.

**Versioning (Principle 4):** the published interface is the versioned artifact; a breaking change is a major bump; consumers' `^2.1.0` refuses the incompatible producer at install/load. `apiVersion` governs host compat; plugin semver governs consumer compat. Both checked at the typecheck gate and again at load.

### 2.10 The registry & developer platform (`s2script.com`)

A plugin is already a versioned `package.json` package with deps and a published interface — the registry falls out. Three services: **(1) package registry + resolver** (`s2script install @edge/admin-core` resolves the dependency closure into `/plugins` — `npm install` for servers; `s2script update`/lockfile for reproducibility); **(2) type/interface distribution** (`s2script add` pulls a plugin's published `.d.ts` into the dev environment — build against another dev's interface type-safely without their source; the load-time gate verifies the same contract); **(3) discovery/community** (browse, search, versioned downloads, publisher pages).

**Scope taxonomy (npm's model, one reserved official scope):**
- **`@s2script/*` — first-party, official.** Engine-generic std lib `@s2script/std`; **per-game packages `@s2script/cs2`, `@s2script/<game>`** (game classes, gamedata, descriptors, APIs); the base-plugin suite (§2.11). Analog: `@angular/*`.
- **`@<community>/*` — verified third-party scopes** (`@edge/*` = EdgeGamers). Exactly npm org scopes.
- **Unscoped — allowed, like npm** (`rtv`). Keeps the barrier low.

**Naming locked:** std lib = **`@s2script/std`**; per-game = **`@s2script/<game>`** (CS2 = `@s2script/cs2`, which also ships the CS2 schema types). **Never name a package `@s2script/core`** — "core" is the Rust/native layer in this spec. Reserve `@s2script` on public npm **and** the s2script registry from day one; verified ownership means `@s2script/*` can only be published by the project (a dev can trust `@s2script/cs2` is genuinely official, not a typosquat).

**Trust model (open, identity + optional signing):** open ecosystem (maximally easy for any community), built on **namespaced scopes with verified ownership** + **optional package signing**. Operators can require signed packages or allowlist scopes without the registry gatekeeping the ecosystem. You can tighten an open ecosystem with opt-in policy; you can't loosen a gatekept one. Accept the real risk (plugins run with full server access) and mitigate with identity + signing + allowlists + a fast unpublish/yank + advisory mechanism from day one.

**Build order:** the registry is Slice 5.5+, **but** lock the `package.json` contract (semver, scopes, `s2script` block, `publishes`, per-game + plugin-dep conventions) *now* so the registry is a distribution layer over an existing model, not a retrofit.

### 2.11 First-party plugin suite — SourceMod base-plugin parity (CS2-specific)

The north star for "the std lib is done": a first-party suite mirroring the SourceMod bundled plugins so an established server **migrates and feels at home day one.** Built as **registry-distributed `@s2script/*` plugins that consume the std lib + the game package** — *not* baked into the runtime. This dogfoods §2.9/§2.10 (the official admin plugin is consumed via the same typed-interface + registry path third parties use), keeps the core lean, lets operators swap a base plugin for a community alternative, and makes the project its own first power-user of the registry.

**These are CS2 plugins** — they depend on `@s2script/cs2` (teams, weapons, the player model) as well as `@s2script/std`. The *parity goal* is CS2's; the *std lib they sit on* is engine-generic. A future Source 2 game would get its own equivalent suite atop its own game package — the std lib and the pattern carry over, the specifics don't.

**Target set** (mirror SourceMod): `@s2script/basechat`, `/basecommands`, `/basebans`, `/basecomm`, `/basevotes`, `/funcommands`, `/playercommands`, `/admin` (the `AdminId`/`GroupId`/flags/immunity system + flatfile + sql backends), `/adminmenu`, `/nextmap`, `/mapchooser`, `/reservedslots`.

**Three-axis parity:** **(1) user-facing behavior** — `!ban/!kick/!gag/!mute/!map/!votekick`, the `sm_admin`-style menu, admin flag letters, `@all`/`@ct`/`@me` targeting, immunity (highest day-one value; mostly portable logic on the std lib, with CS2 targeting filters from cs2); **(2) developer-facing APIs** — published interfaces (`@s2script/admin` mirroring `CanUserTarget`/immunity/flags; `@s2script/comm` exposing gag/mute + `onClientMute`-style events; `@s2script/mapchooser` exposing `getNextMap`/`setNextMap`) so a dev who knows the SourceMod native finds the typed equivalent behaving the same — this is what makes the *ecosystem* portable; **(3) config-format ingestion** — read SourceMod's `admins.cfg`, `admin_groups.cfg`, `admin_overrides.cfg`, the SQL admin schema, `maplist.txt`, so an operator drops existing files in instead of re-entering 200 admins (cheap relative to adoption value; an explicit feature).

**Cost read:** overwhelmingly the *portable* subsystems on capabilities Slice 5 already requires — adds API surface and logic, not much *new* engine RE. The two genuinely engine-coupled pieces: **voice mute/gag** (basecomm needs the Source 2 voice path) and **ban/kick enforcement** (the connection/`kickid` path + a ban backend).

**Strategic role — the std lib's acceptance test.** The std lib (Slice 5) isn't "done" until basechat/basecommands/basebans/basecomm/basevotes implement *cleanly* on top of it. Awkwardness implementing basebans is the std lib telling you it's wrong — fix it in the std lib. The base plugins double as the std lib's spec-by-example. → **Slice 6.**

---

## 3. Build path (risk-ordered vertical slices)

Sequence by **risk, not architectural layer.** Each early slice could prove a load-bearing assumption false; find out cheaply. Build a thin thread through every layer before breadth.

- **Slice 0 — Boot handshake.** MM:S CS2 plugin (`ISmmPlugin`, against hl2sdk `cs2`) loads on a live server, acquires Source 2 interfaces (all engine-generic), embeds V8, runs `console.log("hello")`, unloads cleanly. *Retires: "can I host a JS engine inside Source 2 via Metamod at all."* Steal the load path from CounterStrikeSharp's `mm_plugin.cpp`. (Everything here is Layer-1 engine-generic — `Source2Server`/`SchemaSystem` are Source 2 interfaces, not CS2-specific.)
- **Slice 1 — One multiplexed hook, full contract.** `OnGameFrame` with the *real* descriptor machinery (lazy detour, handler list, `HookResult` collapse, priority ladder, re-entrancy-safe iteration). Prove two handlers compose. **Don't shortcut into "just call a callback"** — this is the thesis; build it properly so every later hook is `+1 descriptor`. (Machinery in core; the `OnGameFrame` *binding* is the first cs2 descriptor — keep them in separate crates from this slice.)
- **Slice 2 — Tick-integrated async.** Microtask checkpoint in `OnGameFrame`; `NextTick()`, `Delay(ms)`, and one threaded op (threadpool sleep resuming on main thread). Prove `await Delay(1000)` doesn't block the tick.
- **Slice 3 — One schema-backed typed accessor, state-change folded — *in the cs2 package*.** In-process dump, resolve one field (`CCSPlayerPawn::m_iHealth`), expose `pawn.health` get/set with the network state-change in the setter. **See the client HUD update.** **The accessor lives in `@s2script/cs2`, not core** — this proves the engine-generic/per-game boundary holds at the very first game-specific accessor. Hardcode the rest; don't build the full codegen pipeline yet.
- **Slice 4 — The slice closes: one `.s2sp` that hot-reloads.** A `.s2sp` (authored from `package.json`, built by `s2script build`, depending on `@s2script/cs2`) exercising Slices 1–3. Drop in → loads via context+ledger. Edit+rebuild+drop → hot-reloads without restart. Delete → clean teardown. **The milestone — the whole architecture proven end to end on a thin thread.**
- **Slice 4.5 — Two plugins talk (prove the inter-plugin contract).** A producer publishes a tiny typed interface (one method + one event carrying a `PlayerRef`); a consumer declares it under `s2script.pluginDependencies`, calls/subscribes. Prove topo-load order, host-injected proxy, the shared `PlayerRef` type across the boundary, and — critically — **unload the producer and confirm the consumer's hard-dep proxy throws `DependencyUnloadedError`** rather than crashing. **Locks the `package.json` contract.**
- **Slice 5 — Std lib breadth on a proven spine.** Full schema codegen pipeline (automate Slice 3, publish CS2 types via `@s2script/cs2`); the proper handle/`EntityRef` system *before* exposing entities widely; `resultApply` breadth + descriptor validation gate; the engine-generic std lib `@s2script/std` (commands, chat, menus, db, timers, translations, targeting engine, handle primitives) **kept clean of CS2 concepts**, with CS2 specifics (team targeting, player model) in `@s2script/cs2`; gamedata hotfix/override + validation tooling. **CI check: core/std must not import any game package.**
- **Slice 5.5 — Registry/platform (§2.10).** Resolver (`s2script install`), type distribution (`s2script add`), `@s2script` reservation + verified ownership + optional signing, `s2script.com`.
- **Slice 6 — First-party base-plugin suite (§2.11) = the std lib's acceptance test.** `@s2script/basechat`, `/basecommands`, `/admin`, `/basebans`, `/basecomm`, `/basevotes`, … as registry-distributed std-lib + `@s2script/cs2` consumers, on all three parity axes (behavior, dev API, SourceMod-config ingestion). Awkwardness → **fix the std lib.** Engine-RE items: voice mute/gag, ban/kick enforcement.

**Cross-cutting, start early:** (a) **gamedata discipline from Slice 1**; (b) **hl2sdk pinned/vendored/patch-capable from Slice 0**, in the update-day fire drill; (c) **core/cs2 package boundary + one-way-dependency CI check from Slice 1**; (d) **`package.json` contract locked by Slice 4.5**; (e) **an update-day fire drill** once Slice 3 has a real offset dependency.

**Effort/risk distribution:** Slices 0–4.5 are ~15–20% of the code but retire ~80% of the risk. **Discipline:** resist Slice 5 breadth before Slice 4.5 closes. A 20-hook multiplexer with a re-entrancy flaw is 20 rewrites; one hook all the way through, then breadth.
