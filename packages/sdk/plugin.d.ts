/**
 * @s2script/sdk/plugin — load-window authoring (`hook`, `publish`, `translations`, `Scope`)
 * and the internal load-scoped {@link PluginContext} types. NO runtime code: the engine injects
 * the implementation at load time (`__s2pkg_plugin`).
 *
 * The plugin artifact is `export function OnPluginStart` (plus optional named publics).
 * See docs/superpowers/specs/2026-08-30-drop-plugin-factory-design.md.
 */
import type { GameEvent, HookResultValue } from "./events";
import type { Client } from "./clients";
import type { EntityRef, OutputEvent } from "./entity";
import type { DamageInfo } from "./damage";
import type { CommandHandler } from "./commands";
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
 * explicitly in OnPluginStart: `for (const c of Clients.all()) { … }` — there is no framework
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
  /**
   * Run `fn` every game frame.
   *
   * `phase` picks WHERE in the frame it runs and defaults to `"pre"` (before simulation). Use
   * `"post"` when the work must land after the engine's own per-frame writes — re-asserting a netvar
   * the engine re-derives during simulation is overwritten if written in `"pre"`, because the
   * derivation happens after and the outgoing snapshot carries the engine's value.
   */
  onGameFrame(
    fn: () => void,
    opts?: { priority?: "high" | "normal" | "low" | "monitor"; phase?: "pre" | "post" },
  ): void;
  /** A new map became live; `mapName` is the BSP name. */
  onMapStart(handler: (mapName: string) => void): void;
  /** Precache window — register models/sounds to precache for the current map. */
  onPrecache(handler: (pc: PrecacheContext) => void): void;
}
/** Console/chat command registration on this plugin's load-scope ({@link PluginContext.commands}). */
export interface CtxCommands {
  /** Register a public command (any client may run it). */
  register(name: string, handler: CommandHandler): void;
  /** Register a server-only command (console/rcon, not client-runnable). */
  registerServer(name: string, handler: CommandHandler): void;
  /** Register an admin command gated by `flags` (an `ADMFLAG` bitmask; fail-safe default-deny). */
  registerAdmin(name: string, flags: number, handler: CommandHandler): void;
  /**
   * Observe an existing CLIENT command by name — SourceMod's `AddCommandListener`.
   *
   * For engine-owned commands (`player_ping`, `jointeam`, `drop`), which {@link register} cannot
   * claim. Observe-by-default: the engine still handles it unless the handler returns
   * `>= HookResult.Handled`. Unsubscribed automatically when the plugin unloads.
   */
  onClientCommand(
    name: string,
    handler: (slot: number, argString: string) => HookResultValue | void,
  ): void;
}
/**
 * The phrase files this plugin uses ({@link PluginContext.translations}).
 *
 * Nothing is loaded automatically — a plugin declares what it needs, the same rule SourceMod's
 * `LoadTranslations` enforces. The build reads this call to work out which keys `cmd.replyT` and
 * `Translations.translate` will accept, so a key from a file you did not load is a compile error.
 */
export interface CtxTranslations {
  /**
   * Load `translations/<name>.phrases.json` for each name, in the order given.
   *
   * Order is significant: `translate` takes the first hit within each of its two passes (the
   * client's language, then English), so list your own set before any shared one if you want to be
   * able to override a shared phrase.
   *
   * @example ctx.translations.load("basecomm", "common");
   */
  load(...names: string[]): void;
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
 * The load-scoped context the host builds for one plugin load. Public authoring uses
 * {@link hook} / {@link command} / named publics instead of receiving this object.
 * {@link Scope} reuses the same subscription namespaces.
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
  /** Phrase files this plugin uses ({@link CtxTranslations}). */
  readonly translations: CtxTranslations;
  /** TopMenu (adminmenu) contribution ({@link CtxTopMenu}). */
  readonly topmenu: CtxTopMenu;
  /** Publish this plugin's manifest-declared interface. Buffered; goes live at Active. */
  publish<T extends object>(name: string, impl: T): PublishHandle;
  /** Resolve a HARD dep (must be in `pluginDependencies`). Immediate — the proxy is callable during OnPluginStart. */
  use<T extends object>(name: string): InterfaceHandle<T>;
  /** Resolve an OPTIONAL dep (must be in `optionalPluginDependencies`); null while unpublished. */
  tryUse<T extends object>(name: string): InterfaceHandle<T> | null;
  /** Allocate a disposable subscription scope (load-window only — the capability originates at load). */
  createScope(): Scope;
}

/** Optional lifecycle hooks a plugin may return to participate in unload + hot-reload.
 *  Public authoring uses `export function OnPluginEnd` / `OnPluginState` instead. */
export interface PluginHooks {
  /** Best-effort cleanup at unload (the ledger remains the teardown authority). */
  onUnload?(): void;
  /** Hot-reload handoff capture; JSON-serialized (EntityRef-aware) and revived as the next instance's {@link previous}. */
  state?(): unknown;
}

/** Load-window client lifecycle + input. Same contracts as {@link CtxClients}. */
export interface HookClient {
  /** A client began connecting (pre-auth). SM `OnClientConnected`. */
  onConnect(handler: (client: Client) => void | Promise<void>): void;
  /** A client's entity was put in the server. SM `OnClientPutInServer`. */
  onPutInServer(handler: (client: Client) => void | Promise<void>): void;
  /** A client became fully active (in-game, receiving snapshots). SM `OnClientActive`. */
  onActive(handler: (client: Client) => void | Promise<void>): void;
  /**
   * Steam ticket validated (engine `fullyconnect`). Closest SM analog is
   * `OnClientPostAdminCheck` — admin cache is host-global.
   */
  onFullyConnected(handler: (client: Client) => void | Promise<void>): void;
  /** A client disconnected. SM `OnClientDisconnect`. */
  onDisconnect(handler: (client: Client) => void): void;
  /** A client's convars/settings changed. */
  onSettingsChanged(handler: (client: Client) => void): void;
  /** A client sent a voice packet (per-frame while speaking). */
  onVoice(handler: (client: Client) => void): void;
  /** A client's persisted cookies finished loading. */
  onCookiesCached(handler: (client: Client) => void): void;
  /** A client sent chat: return a {@link HookResultValue} to suppress it. */
  onSay(handler: (slot: number, text: string, teamonly: boolean) => HookResultValue | void): void;
  /** Per-tick usercmd hook (SM `OnPlayerRunCmd`). */
  onRunCmd(handler: (cmd: UserCmdView, info: { slot: number }) => HookResultValue | void): void;
}

/** Load-window entity lifecycle + damage + I/O. Same contracts as {@link CtxEntities}. */
export interface HookEntity {
  /** Entity created (not yet spawned). `className` is a match, or `"*"` for all. */
  onCreate(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** Entity spawned (post-`DispatchSpawn`). */
  onSpawn(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** Entity is being deleted; the ref goes stale right after. */
  onDelete(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** Entity I/O pre-hook: same contract as {@link CtxEntities.onOutput}. */
  onOutput(classname: string, output: string, handler: (ev: OutputEvent) => HookResultValue | void): void;
  /** Damage pre-hook (SDKHooks-equivalent): same contract as {@link CtxEntities.onDamage}. */
  onDamage(handler: (info: DamageInfo) => HookResultValue | void): void;
}

/** Load-window server / map / frame hooks. Same contracts as {@link CtxServer}. */
export interface HookServer {
  /** Precache window — register models/sounds to precache for the current map. */
  onPrecache(handler: (pc: PrecacheContext) => void): void;
  /**
   * Run `fn` every game frame. Prefer this over `export function OnGameFrame` when you need
   * `phase` / `priority` (HUD paint in `"post"`).
   */
  onGameFrame(
    fn: () => void,
    opts?: { priority?: "high" | "normal" | "low" | "monitor"; phase?: "pre" | "post" },
  ): void;
  /** A new map became live; `mapName` is the BSP name. */
  onMapStart(handler: (mapName: string) => void): void;
}

/**
 * The load-window subscription surface. Throws after settle — the same window as {@link command}.
 *
 * Named publics (`OnGameFrame`, `OnClientConnected`, …) remain the SourceMod-shaped path for a
 * single plugin module. Use `hook.<subject>.*` when registering from `OnPluginStart` (cookbook
 * recipes, multiple subscriptions with options).
 */
export declare const hook: {
  /**
   * Game-event subscription. Default is post (`ctx.events.on`). Pass `"pre"` for `onPre`
   * (`Handled`/`Stop` suppress the client broadcast). The {@link GameEvent} is valid only synchronously.
   */
  event(name: string, handler: (ev: GameEvent) => HookResultValue | void, phase?: "pre" | "post"): void;
  /** TopMenu contribution. Same object as the {@link topmenu} export. */
  readonly topmenu: CtxTopMenu;
  /** Client lifecycle, chat, voice, and usercmd. */
  readonly client: HookClient;
  /** Entity create/spawn/delete, I/O, and damage. */
  readonly entity: HookEntity;
  /** Game frame, map start, and precache. */
  readonly server: HookServer;
};

/**
 * The revived hot-reload handoff (the previous instance's `OnPluginState` return), or `undefined`.
 * Load-window only.
 */
export declare function previous(): unknown;
/**
 * This plugin's id (manifest `id`). Load-window only.
 */
export declare function pluginId(): string;

/**
 * Allocate a disposable subscription scope. Load-window only — same contract as
 * {@link PluginContext.createScope}.
 */
export declare function createScope(): Scope;

/**
 * Publish this plugin's manifest-declared interface. Load-window only (buffered, armed at Active).
 * Same contract as {@link PluginContext.publish}.
 */
export declare function publish<T extends object>(name: string, impl: T): PublishHandle;
/**
 * Resolve a HARD dep (must be in `pluginDependencies`). Load-window only.
 * Same contract as {@link PluginContext.use}. Prefer `import { greet } from "@demo/greeter"` for
 * the producer-as-import form; `use()` remains the explicit load-window form and the optional-dep path.
 */
export declare function use<T extends object>(name: string): InterfaceHandle<T>;
/**
 * Resolve an OPTIONAL dep (must be in `optionalPluginDependencies`); null while unpublished.
 * Load-window only. Same contract as {@link PluginContext.tryUse}.
 */
export declare function tryUse<T extends object>(name: string): InterfaceHandle<T> | null;

/**
 * Load-window TopMenu contribution. Same contract as {@link PluginContext.topmenu}.
 * Throws after settle.
 */
export declare const topmenu: CtxTopMenu;

/**
 * Load-window phrase-file declaration. Same contract as {@link PluginContext.translations}.
 * Throws after settle. `s2s build` / `sync-phrase-types.mjs` collect `translations.load(...)`
 * the same way they collect `ctx.translations.load(...)`.
 */
export declare const translations: CtxTranslations;
