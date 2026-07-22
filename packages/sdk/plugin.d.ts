/**
 * @s2script/sdk/plugin — the typed plugin artifact: `plugin(factory)`, the load-scoped `PluginContext`,
 * and `Scope`. NO runtime code: the engine injects the implementation at load time
 * (`__s2pkg_plugin`, resolved via the existing `@s2script/sdk/*` prefix strip).
 *
 * See docs/superpowers/specs/2026-07-20-L1-lifecycle-v2-design.md §1-§3 for the full contract.
 */
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

/** Game-event subscriptions on this plugin's load-scope ({@link PluginContext.events}). */
export interface CtxEvents {
  /**
   * Subscribe to a fired game event (post-phase). The {@link GameEvent} is valid only synchronously.
   * @example
   * // plugins/disabled/nextmap/src/plugin.ts:133
   * ctx.events.on("round_end", () => console.log("round ended"));
   */
  on(name: string, handler: (ev: GameEvent) => void): void;
  /** Pre-hook a game event: return a {@link HookResultValue} (`Handled`/`Stop` suppress the client broadcast). */
  onPre(name: string, handler: (ev: GameEvent) => HookResultValue | void): void;
}
/**
 * Handlers fire for clients that connect AFTER Active. To cover already-connected clients, seed
 * explicitly in the factory: `for (const c of Clients.all()) { … }` — there is no framework
 * replay (replaying `onConnect` for pre-existing clients would fire auth/ban/reservation logic
 * out of its real order).
 */
export interface CtxClients {
  /** A client began connecting (pre-auth). */
  onConnect(handler: (client: Client) => void | Promise<void>): void;
  /** A client's entity was put in the server (`ClientPutInServer`). */
  onPutInServer(handler: (client: Client) => void | Promise<void>): void;
  /** A client became fully active (in-game, receiving snapshots). */
  onActive(handler: (client: Client) => void | Promise<void>): void;
  /** A client finished authenticating (Steam ticket validated). */
  onFullyConnect(handler: (client: Client) => void | Promise<void>): void;
  /** A client disconnected. */
  onDisconnect(handler: (client: Client) => void): void;
  /** A client's convars/settings changed (`ClientSettingsChanged`). */
  onSettingsChanged(handler: (client: Client) => void): void;
  /** A client sent a voice packet (per-frame while speaking). */
  onVoice(handler: (client: Client) => void): void;
  /** A client's persisted cookies finished loading and are now readable. */
  onCookiesCached(handler: (client: Client) => void): void;
  /** A client sent chat: return a {@link HookResultValue} to suppress it. @param teamonly - team-channel say. */
  onSay(handler: (slot: number, text: string, teamonly: boolean) => HookResultValue | void): void;
  /** Per-tick usercmd hook (SM `OnPlayerRunCmd`): read/modify {@link UserCmdView}; return `Handled` to block the tick. */
  onRunCmd(handler: (cmd: UserCmdView, info: { slot: number }) => HookResultValue | void): void;
}
/** Entity lifecycle + damage subscriptions on this plugin's load-scope ({@link PluginContext.entities}). */
export interface CtxEntities {
  /** An entity of `className` was created (not yet spawned). @param className - match, or `"*"` for all. */
  onCreate(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** An entity of `className` spawned (post-`DispatchSpawn`). */
  onSpawn(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** An entity of `className` is being deleted; the ref goes stale right after. */
  onDelete(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** Hook a named entity output (`FireOutputInternal`); return a {@link HookResultValue} to suppress it. */
  onOutput(classname: string, output: string, handler: (ev: OutputEvent) => HookResultValue | void): void;
  /**
   * Damage pre-hook (SDKHooks-equivalent): read/modify {@link DamageInfo}; return `Handled` semantics via `info`.
   * @example
   * // plugins/basecommands/src/plugin.ts:81 — halve incoming damage
   * ctx.entities.onDamage((info) => { info.damage = info.damage / 2; });
   */
  onDamage(handler: (info: DamageInfo) => HookResultValue | void): void;
}
/** Per-frame + map/precache hooks on this plugin's load-scope ({@link PluginContext.server}). */
export interface CtxServer {
  /** Run `fn` every game frame. @param opts - `priority` orders it within the frame (`monitor` runs last, read-only). */
  onGameFrame(fn: () => void, opts?: { priority?: "high" | "normal" | "low" | "monitor" }): void;
  /** A new map became live; `mapName` is the BSP name. */
  onMapStart(handler: (mapName: string) => void): void;
  /** Precache window — register models/sounds to precache for the current map. */
  onPrecache(handler: (pc: PrecacheContext) => void): void;
}
/** Console/chat command registration on this plugin's load-scope ({@link PluginContext.commands}). */
export interface CtxCommands {
  /** Register a public command (any client may run it). */
  register(name: string, handler: (cmd: CommandInvocation) => void): void;
  /** Register a server-only command (console/rcon, not client-runnable). */
  registerServer(name: string, handler: (cmd: CommandInvocation) => void): void;
  /** Register an admin command gated by `flags` (an `ADMFLAG` bitmask; fail-safe default-deny). */
  registerAdmin(name: string, flags: number, handler: (cmd: CommandInvocation) => void): void;
}
/** Config live-reload subscription on this plugin's load-scope ({@link PluginContext.config}). */
export interface CtxConfig {
  /** Fires when the plugin's config file is re-materialized on disk; re-read values inside. */
  onChange(handler: (cfg: Config) => void): void;
}
/** TopMenu (adminmenu) contribution on this plugin's load-scope ({@link PluginContext.topmenu}). */
export interface CtxTopMenu {
  /** Add (or reuse) a top-level menu category. */
  addCategory(name: string): void;
  /** Add an item under an existing category. */
  addItem(category: string, item: TopMenuItem): void;
}

/** A producer-backed inter-plugin interface: its methods, plus forward subscriptions. */
export type InterfaceHandle<T extends object> = T & {
  /** Subscribe to a producer forward. Load-window only (buffered, armed at Active) — like every registration. */
  on(event: string, handler: (payload: any) => void): void;
};

/**
 * A disposable bundle of subscriptions ({@link PluginContext.createScope}). Registering through a scope
 * lets you drop the whole group at once with {@link Scope.clear} without unloading the plugin.
 */
export interface Scope {
  /** Game-event subscriptions bound to this scope. */
  readonly events: CtxEvents;
  /** Client-lifecycle subscriptions bound to this scope. */
  readonly clients: CtxClients;
  /** Entity/damage subscriptions bound to this scope. */
  readonly entities: CtxEntities;
  /** Per-frame/map subscriptions bound to this scope. */
  readonly server: CtxServer;
  /** Remove every subscription this scope holds; the scope stays usable (re-register on next open). */
  clear(): void;
  /** clear() + permanently retire the scope. Idempotent. */
  dispose(): void;
  /** True once {@link Scope.dispose} has run. */
  readonly disposed: boolean;
}

/**
 * The load-scoped context passed to a plugin's {@link PluginFactory}. Its sub-objects register every
 * subscription; all registration is load-window only (buffered, armed at Active).
 */
export interface PluginContext {
  /** This plugin's id (manifest `id`). */
  readonly id: string;
  /** The revived hot-reload handoff (the previous instance's `state()` return), or undefined. */
  readonly previous: unknown;
  /** Game-event subscriptions ({@link CtxEvents}). */
  readonly events: CtxEvents;
  /** Client-lifecycle subscriptions ({@link CtxClients}). */
  readonly clients: CtxClients;
  /** Entity/damage subscriptions ({@link CtxEntities}). */
  readonly entities: CtxEntities;
  /** Per-frame/map/precache subscriptions ({@link CtxServer}). */
  readonly server: CtxServer;
  /** Console/chat command registration ({@link CtxCommands}). */
  readonly commands: CtxCommands;
  /** Config live-reload subscription ({@link CtxConfig}). */
  readonly config: CtxConfig;
  /** TopMenu (adminmenu) contribution ({@link CtxTopMenu}). */
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

/** Optional lifecycle hooks a {@link PluginFactory} may return to participate in unload + hot-reload. */
export interface PluginHooks {
  /** Best-effort cleanup at unload (the ledger remains the teardown authority). */
  onUnload?(): void;
  /** Hot-reload handoff capture; JSON-serialized (EntityRef-aware) and revived as the next instance's ctx.previous. */
  state?(): unknown;
}
/**
 * A plugin's body: called once at load with the {@link PluginContext}, optionally returning
 * {@link PluginHooks}. May be async (the load waits for it to settle).
 */
export type PluginFactory = (ctx: PluginContext) => void | PluginHooks | Promise<void | PluginHooks>;
/** The opaque artifact {@link plugin} returns; a module must `export default` it to be a valid plugin. */
export interface PluginDefinition {
  /** Brand tag proving this object came from {@link plugin} (host-checked at load). */
  readonly __s2plugin: 1;
}
/**
 * Define a plugin from its factory. `export default` the result — the host calls the factory once at load.
 * @example
 * import { plugin } from "@s2script/sdk/plugin";
 * // examples/greeter-plugin/src/plugin.ts:8
 * export default plugin((ctx) => {
 *   const handle = ctx.publish("@demo/greeter", impl);
 *   ctx.server.onGameFrame(() => handle.emit("greeted", { slot: 0 }));
 * });
 */
export declare function plugin(factory: PluginFactory): PluginDefinition;
