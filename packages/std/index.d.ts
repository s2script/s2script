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
