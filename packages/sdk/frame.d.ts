/**
 * @s2script/frame — author-time type stubs for the per-frame subscription API.
 * NO runtime code: the engine injects the implementation at load time.
 */

export interface SubscribeOptions {
  priority?: number;
}

export declare const OnGameFrame: {
  /** @deprecated moved to ctx.server.onGameFrame; this module is deleted in the cleanup task */
  subscribe(fn: () => void, opts?: SubscribeOptions): void;
};
