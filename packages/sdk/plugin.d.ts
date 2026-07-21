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

export interface CtxEvents {
  on(name: string, handler: (ev: GameEvent) => void): void;
  onPre(name: string, handler: (ev: GameEvent) => HookResultValue | void): void;
}
/**
 * Handlers fire for clients that connect AFTER Active. To cover already-connected clients, seed
 * explicitly in the factory: `for (const c of Clients.all()) { … }` — there is no framework
 * replay (replaying `onConnect` for pre-existing clients would fire auth/ban/reservation logic
 * out of its real order).
 */
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
