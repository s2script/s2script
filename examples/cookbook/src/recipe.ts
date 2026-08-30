/**
 * Cookbook recipes are copy-pasteable plugin modules: named publics
 * (`OnPluginStart`, `OnGameFrame`, …) plus `name` / `describe` for `sm_list`.
 *
 * The cookbook plugin is the only loadable entry (`src/plugin.ts`). It imports
 * every recipe and fans each public — a plugin may export each public once.
 * Copy one recipe file to your plugin's `src/plugin.ts` and it typechecks and
 * loads on its own; `name` / `describe` are unused in that case. `unsafe` also
 * needs this plugin's gamedata; `zones` needs the verified `@s2script/zones`
 * contract copy.
 *
 * Recipes must be side-effect-light at start — register commands and `hook.on`
 * subscriptions, do not start work. Commands are prefixed `sm_` so the whole
 * cookbook is greppable in a console autocomplete.
 */
import type { Client, EntityRef, HookResultValue, PrecacheContext, UserCmdView } from "@s2script/sdk";

export interface Recipe {
  /** Short id, matching the file name (e.g. "http"). */
  readonly name: string;
  /** One line shown by `sm_list`. */
  readonly describe: string;
  OnPluginStart(): void;
  OnGameFrame?(): void;
  OnMapStart?(map: string): void;
  OnPrecache?(pc: PrecacheContext): void;
  OnEntityCreated?(entity: EntityRef | null, className: string): void;
  OnPlayerRunCmd?(cmd: UserCmdView, info: { slot: number }): HookResultValue | void;
  OnClientConnected?(c: Client): void | Promise<void>;
  OnClientPutInServer?(c: Client): void | Promise<void>;
  OnClientActive?(c: Client): void | Promise<void>;
  OnClientPostAdminCheck?(c: Client): void | Promise<void>;
  OnClientDisconnect?(c: Client): void;
  OnClientSettingsChanged?(c: Client): void;
  OnClientVoice?(c: Client): void;
}
