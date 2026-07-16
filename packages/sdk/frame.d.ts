/**
 * @s2script/frame — author-time type stubs for the per-frame subscription API.
 * NO runtime code: the engine injects the implementation at load time.
 */

export interface SubscribeOptions {
  priority?: number;
}

export declare const OnGameFrame: {
  /** Register a callback that fires every game frame. */
  subscribe(fn: () => void, opts?: SubscribeOptions): void;
};
