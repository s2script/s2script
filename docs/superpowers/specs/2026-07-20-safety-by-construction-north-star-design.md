# s2script re-architecture — the safety-by-construction north star

**Status:** design / north star (umbrella doc). Per-slice specs land beside this one (E1 first).
**Date:** 2026-07-20.
**Prompted by:** the s2s-ttt port surfacing the same "compiles clean, crashes the server" footgun class that plagues CSSharp, plus a crash-reporter-confirmed core entity-safety bug (changelevel UAF SEGV). Nothing is in production; breaking changes are on the table.

---

## 0. Thesis (one paragraph)

s2script has one disease with three faces: **code that typechecks green and still crashes the server, because the framework lets you write the footgun.** That is the CSSharp failure mode. SourceMod won its war precisely here — the *shape of a correct plugin* was both obvious and enforced, so structurally-broken plugins didn't compile and didn't ship. This re-architecture ports that property to s2script and sharpens it with a modern language: **the host's own books are the only authority — never ask the thing itself.** Entity liveness comes from the host's ledger, not by dereferencing a maybe-freed entity. What is *legal to write* comes from the host's typed contract, not from what happens to compile. And because we're on TypeScript, the enforcement is **live in the editor** — `tsserver` re-checks as you type and ESLint runs the same rules the build runs — so the guarantee is not "the build catches it" but *"your editor red-squiggles it before you ever build."* The competitive wedge: **fast, performant plugins, in an easy language, with ~90% of the C#-framework boilerplate gone, where the server-crashing mistake is a red squiggle, not a live incident.**

The guarantee to design toward, restated as the product promise:

> **green editor (LSP)  ⟹  `s2s build` passes  ⟹  the server loads it — and a plugin that loads cannot corrupt the host.**

---

## 1. The doctrine and its three instances

**Doctrine:** *Liveness and legality are decided by the host's books — populated by notifications, cleared by transitions, expressed as types — never by reading the resource's own memory or by what merely compiles.*

This already exists in the codebase, but only once: the **plugin** layer (`core/src/plugin.rs` — `Registry::generation` + `is_live`, async resolvers gated on it). Two more instances are missing, and their absence is the entire bug list:

| Instance | Books | Today | This re-arch |
|---|---|---|---|
| **Plugins** | `id → generation`, `is_live` | exists | refactored onto a shared `liveness.rs` |
| **Entities** | `index → (host-id, engine-serial)` | **absent** → changelevel UAF SEGV | new; listener-fed; the E1 slice |
| **Load phase** | plugin generation + phase state | **absent** → hook-anywhere, zombie-on-load-failure | new; the L1 slice |

The two generation axes stay **separate instances of the same mechanism**: a plugin reload must not invalidate entities; a map change must not invalidate plugins. Same code (`liveness.rs`), same sentence in CLAUDE.md, two tables.

---

## 2. Confirmed root cause (the urgent correctness face)

Crash-reporter-confirmed (2026-07-20): `EntityRef` is not host-invalidated across a `changelevel`. `entity_resolve_ptr` (`core/src/v8host.rs:3452-3469`) reads the "live" serial **through the very instance it is about to hand back** (`ent → identity@+0x10 → handle`), then compares it to the captured serial (`entity::resolve`, `core/src/entity.rs:174` = `cur == ref && both ≥ 0`). Across a changelevel the game frees that storage; the freed bytes can still read as the old serial, so the gate **green-lights its own use-after-free** and returns a dangling pointer. The next hop (`ent_ref_read_chain` / `readInt32Via`, `v8host.rs:1003` — the native behind `pawn.isValid`'s `EF_IN_STAGING_LIST` read) hardware-SEGVs. `catch_unwind` cannot catch a hardware fault.

The true root is **not** "changelevel forgot to invalidate." It is that the entity layer is **the only handle system in s2script whose liveness authority is the referent itself** — a use-after-free deciding whether a use-after-free is safe. Changelevel is merely the first (loudest) engine operation to expose it. The SAFETY comment at `v8host.rs:3450` admits the whole invariant is *"no entity destroyed between the serial read and the deref"* — a changelevel violates it wholesale, and so, in principle, does any other destroy-then-use window.

Note the safe idiom already exists in-repo: `s2_deref_handle` / `s2_entity_name` validate the serial against the identity **slot in the system-owned chunk**, never touching the instance. `entity_resolve_ptr` is the one resolver doing it backwards, because the shim op it consumes (`ent_by_index`) returns only `m_pInstance` and discards the slot.

---

## 3. Entity safety architecture (slice E1)

### 3.1 Design: host-minted entity identity — `EntityRef = { index, id }`

Mirror the plugin registry's generation pattern exactly.

- **Books.** Core holds `LIVE: index → (id, engine_serial)` on the game thread. On every `OnEntityCreated` notification, mint `id = NEXT_ENTITY_ID++` (u64 monotonic, never reset across maps; JS-safe as f64 up to ~2^53 creations). On `OnEntityDeleted`, remove the entry. On level transition, **clear the whole table** — this *is* the epoch, implicit, no counter to stamp.
- **The feed already exists end-to-end.** `IEntityListener::OnEntityCreated/OnEntityDeleted` (`shim/src/entity_listener.cpp:17-27`) → `s2script_core_dispatch_entity_event(kind, cls, packed_handle)` (`core/src/ffi.rs:134`) → core. The packed handle carries exactly `(index, serial)`.
  - **Critical:** the table write must live in the FFI entry, **unconditionally**, *before and independent of* the JS mux dispatch. `dispatch_entity_event` (`v8host.rs:4175`) early-returns when no subscribers exist and skips under the `try_borrow_mut` re-entrancy guard — a create/delete witnessed while JS is on-stack must still update the books.
- **`EntityRef = { index, id }`.** Engine serial becomes an internal detail held only in the books.
- **Resolution order (the whole point — cheapest & safest first):**
  1. `LIVE[index].id == ref.id`? No → `null`. **No engine memory touched.**
  2. Defense-in-depth: shim-side identity-**slot** validation of the stored `engine_serial` (the `s2_deref_handle` idiom — a new `ent_resolve(index, serial)` op that validates in the chunk and returns `m_pInstance`).
  3. Only now dereference the instance.
- **Minting from raw engine handles** (`readHandle`, `__s2_handle_decode`, enumeration ops): decode → look up `LIVE[index]` → serial match adopts the table's `id`; mismatch/absent → `null`. **A dangling handle field can never mint a live ref.**
- **Identity-derived data** (staging flags for `pawn.isValid`, designer/target names): read from the identity slot via chunk-walk ops, **never** via `instance + 0x10`. The `[16] → 48` chain dies; `pawn.isValid` becomes a table lookup + a slot-flag read — no instance memory at all.

### 3.2 Why host-id beats a bare serial+epoch

Identical guarantees, one field instead of two, an opaque wire format (`{ __s2ref: [index, id] }` — one-line change to the replacer/reviver at `v8host.rs:1140-1143`), immunity to every engine-side serial behavior (reset, wrap, reuse — all become non-events), and it makes the entity layer's liveness mechanism **literally the same pattern** as `Registry::is_live`. The failure modes *degrade*: a missed create → a live entity reported `null` (fail-closed); a missed delete → a window where the stage-2 slot check is the only guard (still memory-safe while the system is alive). **Errors fall toward `null`, never toward a deref** — the property none of the engine-memory designs have.

### 3.3 Alternatives considered (and why not)

- **A — global epoch stamped on each ref, bumped at StartupServer POST.** Fixes only the *observed* crash (old-map ref used after map start). Leaves the circular read intact, doesn't fix intra-map bookkeeping lag, and a ref minted from a stale handle in the new epoch walks straight back into the UAF. Pays most of the ref-shape migration for none of the general guarantee. **This is the bandaid we explicitly rejected.**
- **B — core-owned table keyed on engine `(index, serial)`, no host id.** Sound for the UAF class, but vulnerable to cross-map `(index, serial)` aliasing if the recreated entity system restarts serial counters (a stable index — player pawns! — can legitimately re-appear with an identical pair, so an old ref silently aliases a *different* entity — a correctness lie `T | null` also forbids). Host-id (D) makes this impossible.

### 3.4 Coverage matrix

| Failure window | A epoch | B bare table | **D (chosen)** |
|---|---|---|---|
| Old-map ref used after map start (the crash) | fixed\* | fixed | fixed |
| Ref used in the teardown gap (pre-StartupServer) | **not fixed** | iff deletes fire | iff deletes fire; else slot-check, memory-safe |
| Intra-map bookkeeping lag (unknown paths) | **not fixed** | fixed | fixed |
| New ref minted from stale handle / zombie slot | **not fixed** | partial | fixed |
| Cross-map `(index,serial)` aliasing | fixed | **not fixed** | fixed |
| Liveness read from freed instance memory | **remains** | removed | removed |

\* assumes StartupServer-POST precedes all new-map dispatch (V3/V4).

### 3.5 Live-verification battery (slice E0 — run before freezing E1 details)

E1's *design* is fail-closed under every answer, so it does not block on these; the answers tune one detail (repair-sweep timing) and decide whether E2 is needed.

- **V1** — does `OnEntityDeleted` fire per entity during a changelevel teardown? **V1a** — does it fire before instance free intra-map?
- **V2** — does `CGameEntitySystem` persist or get recreated across changelevel? do identity slots get invalidated on mass-free?
- **V3** — do `GameFrame` ticks (the hook takes `bool simulating`, `s2script_mm.cpp:83`) or any JS dispatches run in the teardown gap before `StartupServer`?
- **V4** — does `StartupServer` POST precede all new-map `OnEntityCreated`? (else add a repair sweep at the first simulating frame)
- **V5** — does Source2 MM:S expose `IMetamodListener::OnLevelShutdown` (or a pre-teardown equivalent)? (hardening for the gap, not a dependency)
- **V6** — do slot serials restart on system recreation? (D makes the answer irrelevant)

### 3.6 Acceptance

The changelevel repro (rapid round-cycling → match-end → changelevel → access a held ref) goes from **deterministic SEGV → deterministic `null`**, crash-reporter-verified (spool empty; accessor returns `null`).

---

## 4. Lifecycle contract (slice L1)

### 4.1 What's broken and why

- `onLoad`'s return is discarded (`v8host.rs:9172`) — a Promise is never awaited. An async `onLoad` runs to its first `await`; the rest resumes later in the frame drain.
- There is **no load-window / phase concept anywhere**. Subscriptions are fully lazy: any subscribe native resolves its owner per-call from the context, so you can hook anything, anywhere, anytime — including from inside a running handler, or after `await`s that leave the plugin half-wired.
- Consequences visible in shipped plugins: `clientprefs` registers its client hooks *after* `await Database.open`, so a client connecting in that window is missed and a load failure is swallowed into a **zombie plugin** (loaded, `db` null, zero hooks). `zones` hand-rolls a `dbReady` promise every write must remember to await — a workaround the framework's missing load window forces on the author.
- Load order is directory-scan order (`loader.rs:335-441`) with no topological sort against `pluginDependencies`, so a consumer can evaluate before its producer published.

### 4.2 Design: the plugin is a factory over a load-scoped context

```ts
export default plugin(async (ctx) => {
  const db = await Database.open("prefs");        // ambient APIs: unchanged, callable anywhere
  ctx.clients.onPutInServer(onPutInServer);        // registration: ONLY on ctx, ONLY during load
  ctx.events.on("player_death", onPlayerDeath);
  return { onUnload, state: () => ({ /* handoff */ }) };
});
```

- **Load window = the factory promise, awaited.** The window is open from context creation until the returned promise settles.
- **Phase state machine:** `Loading → Active → Unloading`, plus **`Failed`**. Necessary regardless of anything else, because an awaited async load spans frames: reload-while-`Loading` queues; a load timeout → `Failed` with a named reason (the per-plugin analog of degrade-per-descriptor); unload-while-`Loading` seals `ctx` and walks the partial ledger. Enables dependency-ordered activation (producers reach `Active` before dependent factories run — ends the publish/import race by construction).
- **Arm-at-Active.** Subscriptions registered during the factory go live *together*, only when the plugin reaches `Active`. No handler ever runs against a half-constructed plugin; the subscription set is a **pure function of one factory run** (deterministic hot-reload). This is the principled fix for what `clientprefs`/`dbReady` symptomize.
- **Registration is unrepresentable outside load.** Registration verbs are **not importable**; they exist only as members of `ctx`. A handler cannot subscribe because nothing in its scope can — "subscribe-at-load" stops being a *rule* and becomes the *shape of the API*. The only residual escape (capturing `ctx` in a handler closure) is (a) sealed at runtime when the phase leaves `Loading` and (b) a trivially *local* lint (`no-ctx-escape`), not a whole-program analysis.
- **Fail loud, not silent.** A throwing factory → `Failed` (named reason, clean teardown of the partial ledger), **not** the current swallow-and-continue that produces zombies. *(Locked decision.)*

### 4.3 The `ctx` surface and the teachable line

> **If it registers a persistent handler, it's a method on `ctx` (load-only). If it's a one-shot action, a query, or data, it stays a free import (call anytime).**

| Moves onto `ctx` (registration, load-only) | Stays a free import (action / data, anytime) |
|---|---|
| `ctx.events.on` / `.onPre` | `Events.fire` / `fireToClient` |
| `ctx.clients.on{Connect,PutInServer,Active,FullyConnect,Disconnect,SettingsChanged}` | `Client.fromSlot`, `ctx.clients.all()` |
| `ctx.commands.register{,Admin,Server}` | `Player.fromSlot` / `.target`, `pickPlayer` |
| `ctx.chat.onMessage` | `Chat.to` / `Chat.toAll` (prints) |
| `ctx.damage.onPre` | entity accessors, schema reads, `createEntity` |
| `ctx.server.onMapStart` (+ future `onMapEnd`) | `Server.command`, cvars |
| `ctx.entities.on{Create,Spawn,Delete,Output}` | `Database.open`, `fetch`, math, `HookResult`, `ADMFLAG` |
| `ctx.frame.each`, `ctx.usercmd.onRun`, `ctx.sound.onPrecache`, `ctx.config.onChange` | |
| `ctx.publish(...)`, `ctx.use("dep")` / `ctx.tryUse("dep")` | |

### 4.4 The one deliberate escape hatch (dynamic subscriptions)

Static plugins (`basecomm`, `clientprefs`) need nothing more. Plugins with genuinely dynamic subscription lifecycles (`menu` hooks WASD input only while open; `zones`, `basevotes`) get **one explicit door**: `ctx.createScope()` (allocated at load, so the *capability* still originates at load) returns a disposable, ledgered scope you drive later and `scope.dispose()` when done. Persistent subs = `ctx.*` (late use unrepresentable); dynamic subs = an explicit owned scope. Never an anonymous late `Events.on`. Exact scope shape is an L1 open item (§7).

### 4.5 Teardown unification (a smell to fix while we're here)

Teardown is currently **three-way**: (1) the ledger reverse-walk (`v8host.rs:10020-10088`); (2) a hand-maintained ~16-store `remove_by_owner` cascade (`:9860-9958`); (3) ad-hoc owner-scoped tables cleaned inline (`CONCOMMANDS`, `COMMAND_META`, `TOPMENU_ITEMS`, transmit rules, config watches). Every new capability slice must remember to append a line to (2)/(3); forgetting is a silent leak. Fix: keep the ledger as the single **ordered** authority (interfaces, imports, connections); give every owner-scoped store a self-registration hook (`EventMux::new` & friends register in an `OWNER_SCOPED_STORES` list exposing `remove_by_owner`). Unload becomes: sweep the store registry → best-effort `onUnload` + handoff capture (unchanged, `v8host.rs:9986-10003`) → ledger reverse-walk → drop exports/context. The phase machine gives teardown-during-`Loading` defined semantics for free.

### 4.6 Naming (locked)

- Plugin factory parameter = **`ctx`** (it *is* the plugin context).
- The command-invocation object (today confusingly also named `ctx` in `basecomm`) is renamed **`cmd`** (`cmd.arg(0)`, `cmd.callerSlot`, `cmd.reply(...)`). *(Locked decision.)*

### 4.7 Breakage (accepted)

Every plugin's export shape (`export function onLoad` → `export default plugin(factory)`); the SDK `.d.ts` restructure (registration verbs move to `ctx` namespaces; non-registration APIs stay ambient — the subscription *model* is kept per the standing constraint, only *when/how registration is legal* changes); `onLoad(prev)` → `ctx.previous`; loader → async phase machine + topological load order; the `s2s create` template + 5E.1 gate fixtures + docs. The base-plugin suite is the acceptance test (`clientprefs`/`zones` get *simpler* — the `dbReady` hack and null guards delete). External consumers (s2s-ttt) rewrite their entry shape.

---

## 5. build ⊇ load + LSP as one model (slices B1, B2)

### 5.1 Frame: a plugin is a statically-verifiable artifact with a formal lifecycle

`plugin(factory)` gives the artifact a **type**. `tsc` — already the only static gate, already run at build and (as the typecheck gate) at reload — verifies the lifecycle shape with no custom machinery: the default export is `PluginDefinition`, the factory return is typed, registration verbs don't exist outside `ctx`. **The declarative model moves the biggest rules into the compiler**, where editor/build parity is already solved.

### 5.2 Loader checks → build

| Check | Today | Disposition |
|---|---|---|
| `apiVersion` major (`loader.rs:368/392`; build copies `""` verbatim, `build.ts:69,113`) | load-only | **Derive at build.** The SDK ships the host major it types; `s2s build` *stamps* `apiVersion` from that constant. The field stops being author-input → the drift class (green build, refused load) is *deleted*, not merely detected. Load keeps its gate as the backstop. |
| `publishes` reconciliation (`loader.rs:84` → `v8host.rs:4471`) | load, dynamic | Derive the manifest `publishes` from the typed definition / statically-analyzable `ctx.publish` calls → reconciliation becomes generation. Runtime keeps the residual check. |
| `typesSha256` | stored, **never verified at load** (inert) | Wire the load-side verify: consumer compiled-against hash vs producer published hash → contract drift fails fast at load, completing the "fails at typecheck **and again** at load" doctrine. |
| config validation | build already stricter than load | Done — the model to copy. |
| single-producer; dep resolution/version | load | Legitimately runtime (cross-plugin). Build adds *lint-grade* advisories (declared dep never imported; import not declared) — easy since imports become `ctx.use("name")`. |

### 5.3 Editor+build rule engine: ESLint (not a TS language-service plugin)

Ship `eslint-plugin-s2script`, pinned by the SDK, scaffolded by `s2s create`, executed **in-process by `s2s build`** after the tsc gate. Deciding argument: a TS language-service plugin runs **only** when the editor uses the *workspace* TypeScript (`typescript.tsdk`); VS Code defaults to its bundled TS, and the failure is **silent** — the plugin doesn't run, the editor shows green, the build shows red. A parity mechanism whose default misconfiguration *manufactures* the editor-green/build-red divergence we're eliminating is disqualified for this job. ESLint's properties are the inverse: its own everywhere-supported editor integration; the *identical* engine + rule versions run programmatically in the build (as the build already runs `tsc` via the compiler API, `typecheck/typecheck.ts:39`); and when the editor side is broken it is *visibly* broken while the build stays authoritative. **Green editor ⇒ green build holds because both ends execute one pinned engine — and green build is guaranteed even when the editor lies.**

Two parity debts to clear in the same work:
1. **The tsconfig fork** — the build gate builds its own in-memory program (baseUrl/paths + `globals.d.ts`) while `s2s create` scaffolds a *different* tsconfig (node_modules resolution) and `tsconfig.base.json` is "editor only." That is an editor-vs-build divergence in the *primary* gate, today, before any custom rules. Fix: one tsconfig source of truth.
2. **The residual rule surface** (small & local, the point of the declarative model): `no-ctx-escape`, `no-floating-promise-in-factory`, `no-bigint-in-interface-payloads` (the known silent-drop footgun), `no-await-in-raw-view`. Everything global ("subscribe only at load", "export the right lifecycle shape") is carried by *types*.

---

## 6. Shared primitive + migration sequence

### 6.1 Shared primitive (extract during E1, reuse in L1)

`core/src/liveness.rs` — monotonic id allocation + keyed `is_live` tables — with `plugin::Registry` refactored onto it, so the CLAUDE.md doctrine line points at **one module** instead of three parallel idioms.

### 6.2 Sequenced path

- **E0 — verification battery.** Instrumented shim on the Docker CS2 gate answering V1–V6 (§3.5). Small, first, cheap.
- **E1 — entity liveness core-authority** (the urgent correctness slice; a Graphite stack of ~4 PRs). Ships **independently** of everything below (no lifecycle coupling — the books don't care *when* subscriptions happen). Acceptance = §3.6.
- **E2 — teardown-gap hardening** (conditional on E0). Only if V1 shows deletes don't fire on mass-free *and* V3 shows JS runs in the gap → acquire the pre-teardown signal (V5) and clear books at gap-start. Otherwise log-and-close.
- **L1 — lifecycle v2.** `plugin(factory)` + `PluginContext` + phase machine + awaited factory + arm-at-Active + topological load order + unified teardown + handoff as `ctx.previous`. Port the base-plugin suite (its awkwardness is the acceptance test).
- **B1 — build ⊇ load.** Derived `apiVersion`; tsconfig unification; `typesSha256` load-verify; `publishes` derived from code.
- **B2 — `eslint-plugin-s2script`.** The four rules, scaffolded config, in-process execution in `s2s build`, pinned via the SDK.

**Ordering rationale:** E0/E1 stop the crashes now and couple to nothing. L1 precedes B1/B2 because the typed artifact shape is what makes B1's derivations and B2's tiny rule surface possible — building the lint layer against the *old* imperative surface means writing, then discarding, the whole-program rules the declarative model exists to delete.

### 6.3 Plugin rewrite scope (expect this to grow)

Every base plugin (`basecommands`, `basechat`, `playercommands`, `antiflood`, `adminhelp`, `basecomm`, `basebans`, `reservedslots`, `basetriggers`, `funcommands`, `clientprefs`) + the opt-in set (`nominations`, `rockthevote`, `funvotes`, `nextmap`) + examples + the SDK menu/topmenu/votes internals migrate to the `plugin(ctx)` shape and the `{index,id}` `EntityRef`. This is the std lib's acceptance test per CLAUDE.md; awkwardness in the port is a framework bug, not a plugin bug.

---

## 7. Open items to resolve in per-slice specs

- **`ctx.createScope()` exact shape** (L1): allocation, disposal, ledger interaction, and how `menu` drives per-open subscriptions through it.
- **Load timeout** (L1): the bound after which a never-settling factory promise → `Failed` (analog to the applyWhenValid settle bound).
- **Already-connected clients at load** (L1): the `for (const c of ctx.clients.all()) …` seed pattern vs any framework replay — pick one, make it the documented idiom.
- **Repair sweep timing** (E1/E0): if V4 shows some `OnEntityCreated` precede `StartupServer` POST, seed the books with a sweep at the first simulating frame — and only at a verified-clean moment.
- **`u64` entity id on the JS wire** (E1): confirm f64 range handling and the `{__s2ref:[index,id]}` replacer/reviver form.

## 8. Decisions locked this session

1. Entity fix = **Candidate D** (`EntityRef = {index, host-id}`, listener-fed books, slot-validation defense-in-depth). The bare epoch is a rejected bandaid.
2. Lifecycle = **declarative-by-construction** (`plugin(ctx)`, registration only on `ctx`) + a formal phase machine — not runtime-policed imperative.
3. Rule engine = **ESLint** (the LS-plugin workspace-TS trap disqualifies it for the parity guarantee).
4. Command-invocation object renamed **`cmd`**; plugin factory param is **`ctx`**.
5. A throwing factory **fails loud → `Failed`**, replacing today's swallow-and-continue zombie.
6. `apiVersion` is **derived/stamped at build**, not authored.
7. Sequence: **E0 → E1** (urgent, independent) then **L1 → B1 → B2**. One north-star doc (this), per-slice specs follow.

---

*Primary sources: the four current-state code traces + Fable's first-principles distillation, preserved in the session scratchpad; verified against `main`@`427a2ae`. Related memory: `changelevel-entity-invalidation-gap`, `ttt-dataflow-recalibration`, `plugin-lifecycle-map-changes`, `re-gamedata-strategy`.*
