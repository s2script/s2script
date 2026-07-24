/**
 * @s2script/sdk/unsafe — plugin-declared engine calls. NO runtime code: the engine injects the
 * implementation at load (`__s2pkg_unsafe`).
 *
 * UNSAFE by design. A declared call reaches a real engine function. The framework validates every
 * descriptor at load and degrades a failure to `null`, but it cannot prove your signature or vtable
 * index names the function you meant. Requires manifest `permissions: ["engine:calls"]` AND an
 * operator allow-list entry (see docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md §6).
 */

/**
 * The calls this plugin declares. The base interface is EMPTY — `s2s build` generates
 * `.s2script/gamedata.d.ts`, which augments this from your gamedata's `calls` section.
 */
export interface EngineCalls {}

export declare const Engine: {
  /**
   * The declared call, or `null` when its descriptor failed a load-time gate (signature miss,
   * validator rejection, slot outside `.text`, missing platform entry, or the plugin is not
   * operator-allow-listed). Guard once at load; the returned function is a plain callable.
   */
  call<K extends keyof EngineCalls>(name: K): EngineCalls[K] | null;
  /** Why a descriptor is unavailable; `"available"` when it resolved. For diagnostics/operator reports. */
  status(name: string): string;
};
