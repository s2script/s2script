/**
 * One cookbook recipe: a self-contained demonstration of a single API,
 * registered from the cookbook's named publics.
 *
 * Recipes must be side-effect-light at registration — register commands and
 * `hook.on` subscriptions, do not start work. Engine callbacks are optional
 * methods the cookbook plugin fans out from `export function OnGameFrame` etc.
 * Commands are prefixed `sm_` so the whole cookbook is greppable in a console
 * autocomplete.
 */
import type { Client, DamageInfo, HookResultValue, PrecacheContext, UserCmdView } from "@s2script/sdk";

export interface Recipe {
  /** Short id, matching the file name (e.g. "http"). */
  readonly name: string;
  /** One line shown by `sm_list`. */
  readonly describe: string;
  /** Register this recipe's commands and game-event subscriptions. */
  register(): void;
  onGameFrame?(): void;
  onMapStart?(map: string): void;
  onPrecache?(pc: PrecacheContext): void;
  onTakeDamage?(info: DamageInfo): HookResultValue | void;
  onPlayerRunCmd?(cmd: UserCmdView, info: { slot: number }): HookResultValue | void;
  onClientConnected?(c: Client): void | Promise<void>;
  onClientPutInServer?(c: Client): void | Promise<void>;
  onClientActive?(c: Client): void | Promise<void>;
  onClientPostAdminCheck?(c: Client): void | Promise<void>;
  onClientDisconnect?(c: Client): void;
  onClientSettingsChanged?(c: Client): void;
  onClientVoice?(c: Client): void;
}
