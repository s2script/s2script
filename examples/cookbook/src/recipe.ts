/**
 * One cookbook recipe: a self-contained demonstration of a single API,
 * registered from the cookbook's OnPluginStart.
 *
 * Recipes must be side-effect-light at registration — register commands and
 * subscriptions, do not start work. Commands are prefixed `sm_` so the whole
 * cookbook is greppable in a console autocomplete.
 */
export interface Recipe {
  /** Short id, matching the file name (e.g. "http"). */
  readonly name: string;
  /** One line shown by `sm_list`. */
  readonly describe: string;
  /** Register this recipe's commands and subscriptions. */
  register(): void;
}
