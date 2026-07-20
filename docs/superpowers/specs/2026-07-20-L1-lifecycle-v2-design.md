# L1 — Lifecycle v2: the typed plugin artifact, the phase machine, and the `ctx` surface

**Status:** per-slice design (final surface). Child of `2026-07-20-safety-by-construction-north-star-design.md` (§4, §8 — the locked decisions are NOT relitigated here; this doc only finalizes the corners the north star deferred).
**Date:** 2026-07-20.
**Scope:** the plugin becomes a **typed artifact** (`export default plugin(factory)`), loaded through a formal **phase machine** (`Loading → Active → Unloading`, plus `Failed`), with **all registration on a load-scoped `ctx`**, one **unified teardown walk**, dependency-ordered activation, and the whole base-plugin suite ported.
**Explicit non-goals (later slices):** B1 (derived `apiVersion`, `typesSha256` load-verify, `publishes` derivation, build⊇load) and B2 (`eslint-plugin-s2script`). L1 only makes the artifact *typed* so B1/B2 become straightforward. No new capabilities (no `Hud.*` — that is send-side user-message work, not lifecycle work). No E1 coupling (entity books ship independently; L1 never touches `EntityRef` shape).

Verified against the working tree at `main`@`427a2ae` (all file:line refs re-checked 2026-07-20).

---

## 1. The artifact

### 1.1 Authoring shape

```ts
import { plugin } from "@s2script/sdk/plugin";
import { Database } from "@s2script/sdk/db";

export default plugin(async (ctx) => {
  const db = await Database.open("prefs");          // ambient APIs: unchanged, callable anywhere
  ctx.clients.onPutInServer((c) => loadCookies(db, c));
  ctx.events.on("player_death", onPlayerDeath);
  return {
    onUnload() { /* cleanup beyond the ledger, rarely needed */ },
    state: () => ({ /* hot-reload handoff, revives as ctx.previous */ }),
  };
});
```

- `plugin(factory)` returns a tagged `PluginDefinition` (`{ __s2plugin: 1, factory }`). The **default export** is the artifact; the host validates the tag at load.
- The factory may be sync or async. **The load window = the factory run until its (awaited) settlement.**
- The factory's return (`PluginHooks`) replaces today's named `onUnload` export. `state()` — not `onUnload`'s return — is the handoff capture (§7).
- **Legacy shape is refused loudly.** A bundle whose exports carry `onLoad`/`onUnload` but no valid tagged default export → `Failed("legacy plugin shape (export onLoad) — rebuild with @s2script/sdk >= 0.2: export default plugin(factory)")`. No silent zombie.
- **`apiVersion` major bumps 1 → 2** (`HOST_API_VERSION_MAJOR = 2`, `core/src/loader.rs:56`). This is the doctrine's "breaking the engine `.d.ts` is a major bump that fails fast at load": an un-rebuilt 1.x `.s2sp` is refused with the *versioned* reason before it can hit the shape error. Every in-repo plugin + the `s2s create` template declares `"apiVersion": "2.x"`. (The value is still hand-authored in L1; B1 derives it.)

### 1.2 The types (`packages/sdk/plugin.d.ts` — new subpath `@s2script/sdk/plugin`)

One new **subpath of the one package** (locked decision #10 — no new npm package; `__s2require` already maps `@s2script/sdk/plugin` → `__s2pkg_plugin` via the existing prefix strip at `core/src/v8host.rs:4332-4338`).

```ts
import type { GameEvent, HookResultValue } from "./events";
import type { Client } from "./clients";
import type { EntityRef, OutputEvent } from "./entity";
import type { DamageInfo } from "./damage";
import type { CommandInvocation } from "./commands";
import type { Config } from "./config";
import type { PublishHandle } from "./interfaces";
import type { TopMenuItem } from "./topmenu";
import type { PrecacheContext } from "./sound";
import type { UserCmdView } from "./usercmd";

export interface CtxEvents {
  on(name: string, handler: (ev: GameEvent) => void): void;
  onPre(name: string, handler: (ev: GameEvent) => HookResultValue | void): void;
}
export interface CtxClients {
  onConnect(handler: (client: Client) => void | Promise<void>): void;
  onPutInServer(handler: (client: Client) => void | Promise<void>): void;
  onActive(handler: (client: Client) => void | Promise<void>): void;
  onFullyConnect(handler: (client: Client) => void | Promise<void>): void;
  onDisconnect(handler: (client: Client) => void): void;
  onSettingsChanged(handler: (client: Client) => void): void;
  onVoice(handler: (client: Client) => void): void;
  onCookiesCached(handler: (client: Client) => void): void;
  onSay(handler: (slot: number, text: string, teamonly: boolean) => HookResultValue | void): void;
  onRunCmd(handler: (cmd: UserCmdView, info: { slot: number }) => HookResultValue | void): void;
}
export interface CtxEntities {
  onCreate(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  onSpawn(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  onDelete(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  onOutput(classname: string, output: string, handler: (ev: OutputEvent) => HookResultValue | void): void;
  onDamage(handler: (info: DamageInfo) => HookResultValue | void): void;
}
export interface CtxServer {
  onGameFrame(fn: () => void, opts?: { priority?: "high" | "normal" | "low" | "monitor" }): void;
  onMapStart(handler: (mapName: string) => void): void;
  onPrecache(handler: (pc: PrecacheContext) => void): void;
}
export interface CtxCommands {
  register(name: string, handler: (cmd: CommandInvocation) => void): void;
  registerServer(name: string, handler: (cmd: CommandInvocation) => void): void;
  registerAdmin(name: string, flags: number, handler: (cmd: CommandInvocation) => void): void;
}
export interface CtxConfig { onChange(handler: (cfg: Config) => void): void; }
export interface CtxTopMenu {
  addCategory(name: string): void;
  addItem(category: string, item: TopMenuItem): void;
}

/** A producer-backed inter-plugin interface: its methods, plus forward subscriptions. */
export type InterfaceHandle<T extends object> = T & {
  /** Subscribe to a producer forward. Load-window only (buffered, armed at Active) — like every registration. */
  on(event: string, handler: (payload: any) => void): void;
};

export interface Scope {
  readonly events: CtxEvents;
  readonly clients: CtxClients;
  readonly entities: CtxEntities;
  readonly server: CtxServer;
  /** Remove every subscription this scope holds; the scope stays usable (re-register on next open). */
  clear(): void;
  /** clear() + permanently retire the scope. Idempotent. */
  dispose(): void;
  readonly disposed: boolean;
}

export interface PluginContext {
  /** This plugin's id (manifest `id`). */
  readonly id: string;
  /** The revived hot-reload handoff (the previous instance's `state()` return), or undefined. */
  readonly previous: unknown;
  readonly events: CtxEvents;
  readonly clients: CtxClients;
  readonly entities: CtxEntities;
  readonly server: CtxServer;
  readonly commands: CtxCommands;
  readonly config: CtxConfig;
  readonly topmenu: CtxTopMenu;
  /** Publish this plugin's manifest-declared interface. Buffered; goes live at Active. */
  publish<T extends object>(name: string, impl: T): PublishHandle;
  /** Resolve a HARD dep (must be in `pluginDependencies`). Immediate — the proxy is callable inside the factory. */
  use<T extends object>(name: string): InterfaceHandle<T>;
  /** Resolve an OPTIONAL dep (must be in `optionalPluginDependencies`); null while unpublished. */
  tryUse<T extends object>(name: string): InterfaceHandle<T> | null;
  /** Allocate a disposable subscription scope (load-window only — the capability originates at load). */
  createScope(): Scope;
}

export interface PluginHooks {
  /** Best-effort cleanup at unload (the ledger remains the teardown authority). */
  onUnload?(): void;
  /** Hot-reload handoff capture; JSON-serialized (EntityRef-aware) and revived as the next instance's ctx.previous. */
  state?(): unknown;
}
export type PluginFactory = (ctx: PluginContext) => void | PluginHooks | Promise<void | PluginHooks>;
export interface PluginDefinition { readonly __s2plugin: 1; }
export declare function plugin(factory: PluginFactory): PluginDefinition;
```

Notes:
- `InterfaceHandle.on`'s payload is `any` deliberately: `unknown` would make every consumer handler (`(p: ZoneEvent) => …`) a contravariance error. The payload's real type comes from the producer's contract docs; typed forwards are a B-series follow-up.
- `Cmd` (the usercmd per-tick view in `usercmd.d.ts`) is renamed **`UserCmdView`** so the locked `cmd` naming (§4.6 north star: the command-invocation object is `cmd`) has exactly one referent. `CommandContext` is renamed **`CommandInvocation`** (same members; handlers name the parameter `cmd`).

---

## 2. The two axes, applied — full disposition of every registration verb

The rules (locked #8): **namespace = subject; return type = power** (`void` = notify, `→ HookResult` = modify/block). Registration lives ONLY on `ctx`/`Scope`. Everything that is an action, query, value type, or a callback scoped to an owned resource object stays an **ambient free import**.

| Today (ambient import) | L1 home | Notes |
|---|---|---|
| `Events.on` / `Events.onPre` | `ctx.events.on` / `.onPre` | unchanged handler shapes |
| `Events.off` | **dropped** | load-window subs never need `off`; dynamic subs live in a `Scope` (`clear()`/`dispose()`) |
| `OnGameFrame.subscribe` | `ctx.server.onGameFrame` | returns `void` (no `{dispose}`); dynamic = `scope.server.onGameFrame`. `frame.d.ts` is deleted |
| `Server.onMapStart` | `ctx.server.onMapStart` | |
| `Sound.onPrecache` | `ctx.server.onPrecache` | `Sound.emit` stays ambient |
| `Chat.onMessage` | `ctx.clients.onSay` | same `(slot, text, teamonly)` shape; `Chat.toSlot/toAll/color` stay ambient |
| `Clients.onConnect…onVoice` (7) | `ctx.clients.*` | `Clients.fromSlot/all` + `Client` stay ambient |
| `Cookies.onCached` | `ctx.clients.onCookiesCached` | `Cookies.register/get/set/…` stay ambient |
| `UserCmd.onRun` | `ctx.clients.onRunCmd` | `Cmd` type → `UserCmdView` |
| `Damage.onPre` | `ctx.entities.onDamage` | in-place modify kept; NEW: returning `>= HookResult.Handled` zeroes the damage (return-type carries the block power; `damage.d.ts` keeps only `DamageInfo`) |
| `Entity.onCreate/onSpawn/onDelete/onOutput` | `ctx.entities.*` | `Entity.findByClass`, `createEntity`, `EntityRef` stay ambient |
| `Commands.register/registerServer/registerAdmin` | `ctx.commands.*` | `Commands.dispatch/parseChatTrigger/handleChatTrigger/triggers/list` stay ambient |
| `config.onChange` | `ctx.config.onChange` | `config.get*/readFile/writeFile` stay ambient |
| `publishInterface` | `ctx.publish` | `PublishHandle` type stays in `interfaces.d.ts` |
| ESM interface import (`import { on, getZones } from "@s2script/zones"`) | `ctx.use` / `ctx.tryUse` | the zones-consumer "defer until producer live" hack is DELETED — topological activation (§5.4) makes the producer Active before the consumer factory runs |
| `TopMenu.addCategory/addItem` | `ctx.topmenu.*` | `TopMenu.snapshot/select` stay ambient (the adminmenu renderer reads them at command time) |
| `UserMessages.onPre` | **stays ambient — documented exception** | locked #9: raw user-message interception is the advanced boundary, "never on ctx"; it remains runtime-managed (ledgered, owner-swept) until the unsafe-module slice formalizes it |
| ws/net/db connection `on*`, `Menu.onSelect/onCancel`, `Vote.start{onEnd}` | **stay on the resource object** | callbacks scoped to an owned, ledgered resource are not plugin-persistent registrations; the resource's lifecycle (ledger) owns them |
| `Menu.registerRenderer` / `Vote.registerTallyRenderer` | **stay ambient** | game-prelude seams — the CS2 prelude (raw context scope, no ctx exists there) registers them; not a plugin surface |

**Finalized deferred corners:**

- **Timers are ambient** (`delay`/`nextTick`/`nextFrame`/`threadSleep` stay free imports from `@s2script/sdk/timers`). Justification: they are one-shot *actions* returning promises, not subscriptions — there is no persistent handler registry to make deterministic; each is auto-ledgered (`Resource::Timer`/`Job`) and liveness-gated at resolve (`v8host.rs` resolver owner tags), so teardown is already host-owned; and they are *legitimately* called from handlers (`funcommands` un-freeze, `basetriggers` next-frame reply). Forcing them onto `ctx` would only push handlers to capture `ctx` — the exact escape the design closes.
- **Admin is ambient** (`ADMFLAG`, `Admin.*`): queries/mutations of the host-global admin cache, no handlers. Only `registerAdmin` (a registration) moves, as `ctx.commands.registerAdmin`.
- **Votes are ambient** (`Vote.start/isActive/cancel`): starting a vote is an action; `onEnd` is a callback on the one-shot vote resource.
- **Menus are ambient** (`Menu`, `MenuStyle`): displaying a menu is an action. The menu system's own dynamic input subscriptions are SDK-internal prelude code (host-authored, runs over raw natives, unchanged in L1). A *plugin* that needs menu-like dynamics uses `ctx.createScope()` (§3).
- **Already-connected clients at load** (north-star open item): the documented idiom is an explicit seed — `for (const c of Clients.all()) { … }` inside the factory. No framework replay: replaying `onConnect` for clients that connected before the plugin existed would fire auth/ban/reservation logic out of its real order. The idiom is documented on `CtxClients` in `plugin.d.ts`.

---

## 3. `ctx.createScope()` — the one escape hatch, exact shape

- **Allocation is load-window-only.** `createScope()` throws after the ctx seals. A plugin allocates the scopes it needs in the factory (usually one) and drives them any time until unload.
- **Registration through a scope is legal at any time** (that is its purpose), on the four *subscription* subjects only (`events`/`clients`/`entities`/`server`). No `commands`/`config`/`publish`/`use` on scopes — those are plugin-lifetime by nature.
- **`clear()`** removes every subscription the scope holds and leaves it reusable (the per-open pattern). **`dispose()`** = `clear()` + retire (further registration throws). Both idempotent.
- **Ledger interaction:** scope subscriptions register in the same mux stores under the *plugin's* owner id (so dispatch, liveness, and crash-breadcrumb attribution are unchanged), but each mux row now carries a **subscription id** from the global `NEXT_SUB_ID` counter (`v8host.rs:588`). The scope collects its ids (plus `{dispose}` closures for frame subs) and `clear()` calls the new native `__s2_scope_dispose(ids)` → every registered owner-scoped store runs `remove_by_ids`. **Unload needs no special casing**: undisposed scope rows are swept by the same `remove_by_owner(plugin)` walk as everything else (§6). Scope registrations made *while still Loading* buffer with the plugin's pending set and arm at Active like everything else (arm-at-Active is unconditional — nothing dispatches into a not-yet-Active plugin).
- **Concrete usage — the menu/editor pattern** (this is `zones`' in-game E-mark editor, which today leaks a permanently-subscribed frame poll that early-returns when idle):

```ts
export default plugin((ctx) => {
  const editPoll = ctx.createScope();            // allocated at load; empty & free until used
  const edits = new Map<number, EditSession>();

  function startEdit(slot: number, name: string) {
    if (edits.size === 0) {
      editPoll.server.onGameFrame(pollEditSessions);   // subscribed only while a session exists
    }
    edits.set(slot, newSession(name));
  }
  function endEdit(slot: number) {
    edits.delete(slot);
    if (edits.size === 0) editPoll.clear();            // poll fully unhooked between sessions
  }

  ctx.commands.registerAdmin("sm_zone_edit", ADMFLAG.GENERIC, (cmd) => startEdit(cmd.callerSlot, cmd.arg(0)));
});
```

---

## 4. Runtime mechanics of the load window

- The ctx is built by the injected engine prelude (`__s2_make_ctx` in `INJECTED_STD_PRELUDE`, `v8host.rs:927`). Every ctx/scope verb pushes a **thunk** onto a pending list; the thunks close over the *existing* prelude module functions (`__s2pkg_events.Events.on`, …), so **no subscribe native changes for arming** — arm = replay the thunks.
- `__s2_run_factory(def)` (prelude) builds the ctx (reviving `ctx.previous` via a new `__s2_handoff_take()` native), calls `def.factory(ctx)`, and settles:
  - non-thenable return → `__s2_load_settled(hooks)` **synchronously** (the sync fast-path: a sync factory reaches Active inside the same `load_plugin_js` call — today's boot semantics preserved for the whole base suite);
  - thenable → `.then(hooks => __s2_load_settled(hooks), e => __s2_load_failed(String(e?.stack ?? e)))` — continuations run at the frame drain's microtask checkpoint; a new `finalize_loading_plugins()` step at the tail of `frame_async_drain` (`v8host.rs:9684`) performs the transition.
  - a synchronous factory throw → `__s2_load_failed` immediately.
- **Arm-at-Active:** the finalizer (or the sync fast-path) calls `ctx.__arm()` → replays every pending thunk → **seals** the ctx (every verb thereafter throws `"registration outside the load window — use a Scope from ctx.createScope()"`). Then `reconcile_publishes(id)` runs (it MOVES here from `loader::load_and_reconcile`, `loader.rs:84` — with buffered `ctx.publish` the publish only exists after arm); success → phase `Active` + `breadcrumb::plugin_loaded`. Any arm/reconcile failure → the Failed path.
- **`ctx.use`/`tryUse` are immediate** (acquisitions, not subscriptions — the factory may call producer methods): they validate the name against the declared dependency map (`use` requires `pluginDependencies`, `tryUse` requires `optionalPluginDependencies`; a mismatch throws — a manifest bug fails loud) and return the same lazy proxy `__s2_require` builds today (`makeIfaceProxy`, `v8host.rs:945`). Only the handle's `.on()` is buffered.
- **Raw natives stay permissive.** The seal is the ctx layer + the typed surface; SDK-internal prelude code (menu/votes/topmenu implementations) keeps calling natives directly. The residual `no-ctx-escape` lint is B2.

## 5. The phase machine

`Phase = Loading | Active | Unloading | Failed` (new, in `core/src/plugin.rs`, stored on the `PluginInstance`). Host state: `LOADING: id → { started_frame, state: InFlight|Settled|Failed(reason), pending_reload }`; `FAILED_PLUGINS: id → reason` (survives context teardown, for `sm plugins list`; cleared on the next successful load or file removal).

1. **`Loading → Active`** — factory settled + arm OK + `reconcile_publishes` OK (§4).
2. **`Loading → Failed`** — factory threw/rejected, arm threw, reconcile failed, **or load timeout**: `LOAD_TIMEOUT_FRAMES = 1920` (~30 s at 64 Hz) drains after `started_frame` → `Failed("factory did not settle within ~30s")`. Failed teardown = seal ctx → sweep owner-scoped stores (a no-op for buffered subs) → walk the **partial ledger** (DB conns, timers, imports acquired before the failure) → dispose context. Named WARN + `crash::report_js_error(id, "factory", …)`. **This replaces the swallow-and-continue zombie** (locked #5) — `clientprefs`' old catch-and-log dies with the port.
3. **Reload-while-Loading queues**: the loader marks `pending_reload` instead of tearing down a mid-flight load; when the load settles (either way) the queued reload runs (unload if needed → fresh load from disk).
4. **Unload-while-Loading seals**: seal ctx (buffered subs are dropped unarmed), skip `onUnload`/`state()` (never Active — defined semantics), sweep stores + walk the partial ledger + dispose. Used by both operator unload and shutdown.
5. **`Active → Unloading`** — the ordered teardown (§6). `Unloading` is transient (no re-entrant dispatch: teardown runs at the frame boundary as today).

### 5.4 Dependency-ordered activation

- `poll_plugins` topologically sorts each scan's Load batch: producers (manifest `publishes` names) before consumers (`pluginDependencies` names). Cycles fall back to name order with a WARN.
- A consumer whose **hard** dep interfaces lack an Active producer does not start; it parks in `WAITING: id → {manifest, js, cfg, since_frame}`. Every `finalize_loading_plugins()` pass starts newly-unblocked waiters (their producer just reached Active). Optional deps never gate (they'd deadlock on absence).
- **Wait bound = `LOAD_TIMEOUT_FRAMES`**: on expiry the consumer loads anyway with a WARN — the hard-dep proxy keeps today's lazy contract (`InterfaceUnavailable` at call time), so a genuinely absent producer degrades exactly as before instead of bricking the consumer.
- Boot order end state: **producers reach `Active` before dependent factories run** — the publish/import race and the zones-consumer polling hack are gone by construction.

## 6. Teardown unification

`unload_plugin` (`v8host.rs:9855`) currently: (a) a hand-maintained ~16-store `remove_by_owner` cascade (`:9860-9958`), (b) best-effort `onUnload` + handoff, (c) the ledger reverse-walk, (d/e) Global/context drop. L1 replaces (a) with a **self-registered store walk**:

- New `core/src/owner_stores.rs`: `register_owner_scoped_store(name, remove_by_owner: fn(&str), remove_by_ids: fn(&[u64]))` + `sweep_owner(owner)` + `sweep_ids(ids)`. Each store's closure carries its OWN follow-up (the `event_unsubscribe` engine-op for emptied names, the PRE-mux global-hook removal, the usermsg bitmap unsub, transmit re-push, CONCOMMANDS/COMMAND_META/TOPMENU_ITEMS retain, config unwatch) — the whole (a) block moves verbatim into per-store closures registered by one `register_builtin_stores()` called from `init()`.
- Unload becomes: seal ctx → `sweep_owner(id)` → `state()` capture + best-effort `onUnload` (off the stored `PluginHooks` Global) → ledger reverse-walk (unchanged, `v8host.rs:10020-10088`) → drop hooks Global → dispose context. A future capability slice registers its store next to the store's definition — forgetting is now impossible to express, not merely easy to forget.
- `EventMux` (`core/src/event_mux.rs`) gains a per-row `id: u64` (allocated from `NEXT_SUB_ID`) + `remove_by_ids(&[u64]) -> Vec<String>` (emptied names, same contract as `remove_by_owner`); subscribe natives return the id (previously `undefined` — additive). This is what `Scope.clear()` rides (§3).

## 7. Handoff: `state()` → `ctx.previous`

- Capture moves from "the `onUnload` return" to the explicit `hooks.state()` — called at `Active → Unloading`, **before** `onUnload()` (capture while resources are alive), serialized with the existing `iface_to_json` (JSON + EntityRef replacer; BigInt still throws → WARN + no handoff) into `PENDING_HANDOFF` (`v8host.rs:732`), consume-once.
- Revival: `__s2_handoff_take()` runs `iface_from_json` in the NEW context at factory start; the value is `ctx.previous`. `clear_pending_handoff` on final removal is unchanged. `onUnload`'s return is now ignored (WARN once if non-undefined, to catch un-ported habits).

## 8. Loader & operator surface

- `loader::load_and_reconcile` → `start_load` (no reconcile; §4). apiVersion gate unchanged (major now 2). WATCH_STATE/mtime retry semantics unchanged.
- `plugin_list()` grows a state string: `running | loading | waiting | failed | unloaded`; `plugins.d.ts` `PluginInfo` gains `readonly state: string` (additive; `loaded` kept). `sm plugins list` prints it.
- `Plugins.unload/reload/load` honor the phase rules (§5.3/5.4). `unload_all` (shutdown) uses the same reverse-dependency order + unload-while-Loading sealing.

## 9. Build/SDK surface changes (L1 slice of the toolchain)

- **`packages/sdk/plugin.d.ts`** (new, §1.2). Registration verbs REMOVED from: `events` (on/off/onPre), `frame` (file deleted), `server` (onMapStart), `sound` (onPrecache), `chat` (onMessage), `clients` (the 7 on*), `cookies` (onCached), `usercmd` (onRun; `Cmd`→`UserCmdView`), `damage` (const deleted; `DamageInfo` kept), `entity` (onCreate/onSpawn/onDelete/onOutput; `Entity.findByClass` kept), `commands` (register*; `CommandContext`→`CommandInvocation`), `config` (onChange), `interfaces` (publishInterface; `PublishHandle` kept), `topmenu` (addCategory/addItem). Removal lands as the LAST stack step (after all ports) so every intermediate PR keeps the 5E.1 gate green; until then the new surface is additive and the old carries `@deprecated`.
- **tsconfig-fork fix (one source of truth):** new `packages/sdk/src/tsconfig-shared.ts` exporting the semantic compiler options (`strict`, `noEmit`, `moduleResolution: bundler`, `module: ESNext`, `target/lib: ES2020`, `types: []`, `skipLibCheck`, `allowImportingTsExtensions`); both the gate's in-memory program (`typecheck.ts:81-102`) and the `s2s create` scaffold (`create.ts:208`) consume it. Resolution mapping legitimately differs (in-repo `baseUrl`/`paths` vs node_modules) and stays; the *semantics* can no longer fork.
- **`s2s create` template** emits the `plugin(ctx)` shape + `"apiVersion": "2.x"`.
- `s2s build` itself is unchanged (the bundle's `exports.default` carries the definition through the existing CJS wrapper; static default-export verification is B1).
- Changeset: `@s2script/sdk` minor bump (pre-1.0 breaking), `@s2script/cs2` untouched.

## 10. Port scope & acceptance

Ports (one task each, §north-star 6.3): `basecommands, basechat, playercommands, antiflood, adminhelp, basecomm, basebans, reservedslots, basetriggers, funcommands, clientprefs, adminmenu, basevotes, zones` + opt-in `nominations, rockthevote, funvotes, nextmap` + all `examples/*` (4 batches; the 5E.1 gate typechecks them). Mechanical recipe: imports move per §2 table; `export function onLoad` → `export default plugin((ctx) => …)`; command handler param `ctx` → `cmd`; `export function onUnload` → returned `onUnload` hook; `apiVersion` → `2.x`.

**The suite is the acceptance test** (CLAUDE.md): `clientprefs` loses the try/catch zombie + the `db: Database | null` guards (the factory awaits the open; a failure is a loud `Failed`); `zones` loses the IIFE + `dbReady` promise + (optionally, done in its port) the always-on editor poll via a Scope; `zones-consumer-demo` loses the poll-until-producer hack. Awkwardness discovered in any port is a framework bug to fix in the spine, not to paper over in the plugin.

**Live gate (held, human):** Docker CS2 — boot with the full suite (all Active, `sm plugins list` states correct), hot-reload clientprefs (cookie state survives via `state()`/`ctx.previous`), kill the DB path to prove Failed-not-zombie, `sm_zone_edit` scope open/clear, reload-while-Loading, and a legacy 1.x `.s2sp` refused with the apiVersion reason.

---

## Open questions for human review

1. **`LOAD_TIMEOUT_FRAMES = 1920` (~30 s)** — picked as the applyWhenValid-style settle bound; generous for DB/HTTP init, short enough to surface a hung factory within one operator attention span. Same bound reused for the hard-dep wait (§5.4). A config knob (`s2script.json`) is deferred until an operator needs it.
2. **Dep-wait expiry loads the consumer anyway** (lazy `InterfaceUnavailable` at call, today's contract) rather than `Failed("producer absent")`. Chosen to keep hard-dep semantics identical to the shipped contract-grammar behavior; the alternative (fail the consumer) is stricter but turns a producer crash into a cascade of consumer failures. Flag if you want the strict version.
3. **`ctx.clients.onSay` keeps the `(slot, text, teamonly)` shape** (today's `Chat.onMessage`) instead of `(client: Client, …)`. Minimal churn for the 5 porting plugins; a `Client`-first shape would be more subject-consistent — cheap to change now, expensive later.
4. **`Events.off` is dropped** from the typed surface (scopes replace it). The runtime native remains (SDK-internal). If a port turns up a legitimate load-window `off` need, it comes back as a `Scope`-only verb.
5. **usercmd's `Cmd` → `UserCmdView` rename** — done purely to keep `cmd` (command invocation) unambiguous. Bikeshed-level; veto costs nothing before the ports land.
6. **`UserMessages.onPre` stays an ambient registration** (the documented advanced-boundary exception, per locked #9). It is ledgered and owner-swept, but it is the one surface where "registration is unrepresentable outside load" does not hold until the unsafe-module slice.
7. **Buffered `ctx.publish`** means a producer's interface is visible only at Active — consistent with topo-activation, but it makes "publish then immediately self-call via use()" impossible within one plugin (no known use; a plugin can call its own impl directly).
