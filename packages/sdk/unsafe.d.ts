/**
 * @s2script/sdk/unsafe — plugin-declared engine calls and inbound hooks. NO runtime code: the
 * engine injects the implementation at load (`__s2pkg_unsafe`).
 *
 * UNSAFE by design. A declared call reaches a real engine function; a declared hook patches
 * that function's prologue. The framework validates every descriptor at load and degrades a
 * failure to `null`, but it cannot prove your signature or vtable index names the function you
 * meant. Calls require manifest `permissions: ["engine:calls"]`; hooks require the separate
 * `"engine:hooks"` permission. Both also need an operator allow-list entry
 * (see docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md §6 and ARCHITECTURE.md §2.0.7).
 */

/**
 * The calls this plugin declares. The base interface is EMPTY — `s2s build` generates
 * `.s2script/gamedata.d.ts`, which augments this from your gamedata's `calls` section.
 */
export interface EngineCalls {}

/**
 * The inbound hooks this plugin declares. The base interface is EMPTY — `s2s build` generates
 * `.s2script/hooks.d.ts`, which augments this from your gamedata's `hooks` section.
 */
export interface EngineHooks {}

/**
 * Plugin-declared engine calls and inbound hooks, resolved from this plugin's own gamedata.
 *
 * @example
 * import { Engine } from "@s2script/sdk/unsafe";
 * // Resolve ONCE at load — null means the descriptor failed a load-time gate.
 * const ignite = Engine.call("ignite");
 * if (!ignite) console.log(`unavailable: ${Engine.status("ignite")}`);
 * const onFoo = Engine.hook("onFoo");
 * if (onFoo) onFoo((view) => { view.reason = 0; });
 */
export declare const Engine: {
  /**
   * The declared call, or `null` when its descriptor failed a load-time gate (signature miss,
   * validator rejection, slot outside `.text`, missing platform entry, or the plugin is not
   * operator-allow-listed). Guard once at load; the returned function is a plain callable.
   */
  call<K extends keyof EngineCalls>(name: K): EngineCalls[K] | null;
  /** Why a call descriptor is unavailable; `"available"` when it resolved. For diagnostics/operator reports. */
  status(name: string): string;
  /**
   * The declared hook's subscribe function, or `null` when its descriptor failed a load-time
   * gate (same class of reasons as {@link Engine.call}, plus a missing `validate` or an unknown
   * thunk shape). Guard once at load; the returned function records the handler and lazily
   * installs the detour. The owner is always the calling plugin — you cannot subscribe to
   * another plugin's hook through this factory (game-package hooks hang off `ctx.*`).
   */
  hook<K extends keyof EngineHooks>(name: K): EngineHooks[K] | null;
  /** Why a hook descriptor is unavailable; `"available"` when it resolved. */
  hookStatus(name: string): string;
};
