/**
 * @s2script/std — author-time type stubs for the injected standard library.
 * NO runtime code: the s2script engine injects the real implementation at load time.
 * Plugins consume this package for TypeScript type checking only.
 */

export interface SubscribeOptions {
  priority?: number;
}

export declare const OnGameFrame: {
  /** Register a callback that fires every game frame. */
  subscribe(fn: () => void, opts?: SubscribeOptions): void;
};

/** Await a delay of `ms` milliseconds before continuing. */
export declare function delay(ms: number): Promise<void>;

/** Yield to the next microtick. */
export declare function nextTick(): Promise<void>;

/** Yield until the next game frame. */
export declare function nextFrame(): Promise<void>;

/**
 * Block the current thread (fiber) for `ms` milliseconds.
 * Only valid inside a threadSleep-capable fiber context.
 */
export declare function threadSleep(ms: number): void;

/** Engine-provided console (same interface as globalThis.console). */
export declare const console: typeof globalThis.console;

/**
 * Handle returned by {@link publishInterface}: lets the producer emit forwarded
 * events to every plugin subscribed to this interface via its `on(event, …)`.
 */
export interface PublishHandle {
  /** Emit a forwarded event to all consumers subscribed via `interface.on(event, …)`. */
  emit(event: string, payload: unknown): void;
}

/**
 * Publish a typed inter-plugin interface under `name`@`version`. `impl`'s methods
 * become the natives consumers call (`interface.method(...)`); the returned handle's
 * `emit` fans forwarded events out to consumers' `on(event, …)` subscriptions.
 * Auto-ledgered: the interface is withdrawn (and hard-dep consumers degraded) on unload.
 */
export declare function publishInterface(
  name: string,
  version: string,
  impl: Record<string, (...args: any[]) => any>,
): PublishHandle;
