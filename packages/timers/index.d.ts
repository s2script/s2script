/**
 * @s2script/timers — author-time type stubs for the async timing API.
 * NO runtime code: the engine injects the implementation at load time.
 */

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
