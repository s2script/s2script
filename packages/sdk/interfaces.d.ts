/**
 * @s2script/interfaces — author-time type stubs for typed inter-plugin interfaces.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * Handle returned by {@link publishInterface}: lets the producer emit forwarded
 * events to every plugin subscribed to this interface via its `on(event, …)`.
 */
export interface PublishHandle {
  /** Emit a forwarded event to all consumers subscribed via `interface.on(event, …)`. */
  emit(event: string, payload: unknown): void;
}

/**
 * Publish a typed inter-plugin interface under `name`. `impl`'s methods become the
 * natives consumers call (`interface.method(...)`); the returned handle's `emit` fans
 * forwarded events out to consumers' `on(event, …)` subscriptions.
 *
 * The interface's VERSION is injected by the host from this plugin's manifest
 * `publishes` map — never passed here, and never written in TypeScript source.
 * Publishing a name the manifest does not declare is refused at load.
 *
 * `impl` is generic over `object` rather than `Record<string, Function>` so that a
 * producer can bind it to its contract — `const impl: Zones = {…}` — which is what
 * actually proves the implementation matches the published `.d.ts`. A TypeScript
 * `interface` has no implicit index signature (only a `type` alias does), so a
 * `Record<…>` parameter would reject every interface-typed contract. The host
 * enumerates `impl`'s own function properties; non-function properties are ignored.
 *
 * Auto-ledgered: the interface is withdrawn (and hard-dep consumers degraded) on unload.
 *
 * @deprecated moved to ctx.publish (L1 lifecycle v2) — removed after the port fan-out
 */
export declare function publishInterface<T extends object>(
  name: string,
  impl: T,
): PublishHandle;
