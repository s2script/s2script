/**
 * @s2script/interfaces — author-time type stubs for typed inter-plugin interfaces.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * Handle returned by `ctx.publish(name, impl)`: lets the producer emit forwarded
 * events to every plugin subscribed to this interface via its `on(event, …)`.
 */
export interface PublishHandle {
  /**
   * Emit a forwarded event to all consumers subscribed via `interface.on(event, …)`. The payload is
   * structured-copied across the context boundary (JSON, EntityRef-aware — never a live reference).
   * @example
   * import type { PublishHandle } from "@s2script/sdk/interfaces";
   * // plugins/zones/src/plugin.ts:30 — notify consumers a zone was created
   * iface.emit("created", { zone: z.name, min: z.min, max: z.max, tags: z.tags });
   */
  emit(event: string, payload: unknown): void;
}
