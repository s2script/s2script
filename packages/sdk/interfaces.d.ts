/**
 * @s2script/interfaces — author-time type stubs for typed inter-plugin interfaces.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * Handle returned by `ctx.publish(name, impl)`: lets the producer emit forwarded
 * events to every plugin subscribed to this interface via its `on(event, …)`.
 */
export interface PublishHandle {
  /** Emit a forwarded event to all consumers subscribed via `interface.on(event, …)`. */
  emit(event: string, payload: unknown): void;
}
