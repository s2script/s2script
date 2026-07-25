/**
 * @s2script/timers — author-time type stubs for the async timing API.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * Await a delay of `ms` milliseconds before continuing. Tick-integrated (resumes on a game frame).
 * @example
 * import { delay } from "@s2script/sdk/timers";
 * // plugins/funcommands/src/plugin.ts:77 — sm_freeze auto-unfreeze after `secs`
 * delay(secs * 1000).then(() => { const q = Player.fromSlot(slot); if (q && q.pawn) q.pawn.moveType = WALK; });
 */
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

/**
 * A live callback timer, returned by {@link after} and {@link every}. Ledgered against the creating
 * plugin: unload kills it whether or not `kill()` was called, so a repeating timer can never outlive
 * its plugin and fire into a dead context.
 */
export interface Timer {
  /** False once the timer has fired (one-shot), been killed, or had its plugin unloaded. */
  readonly alive: boolean;
  /** Cancel it. Idempotent — returns false if it was already dead. Safe to call from inside the callback. */
  kill(): boolean;
}

/**
 * Run `fn` once after `ms` milliseconds (SourceMod `CreateTimer` without `TIMER_REPEAT`).
 *
 * Prefer {@link delay} when you can `await`; use this when you need something cancellable.
 * A throwing callback is reported and contained — it never kills the frame or the timer system.
 *
 * @example
 * import { after } from "@s2script/sdk/timers";
 * const t = after(5000, () => console.log("5s later"));
 * t.kill();   // ...unless cancelled first
 */
export declare function after(ms: number, fn: () => void): Timer;

/**
 * Run `fn` every `ms` milliseconds until killed (SourceMod `CreateTimer` with `TIMER_REPEAT`).
 *
 * The next firing is scheduled *after* the callback returns, so a slow callback cannot pile up.
 * `ms` must be greater than 0 — a zero-interval repeat would re-arm every drain and starve the
 * frame, so it throws rather than degrading.
 *
 * @example
 * import { every } from "@s2script/sdk/timers";
 * const tick = every(1000, () => console.log("tick"));
 * // later: tick.kill();
 */
export declare function every(ms: number, fn: () => void): Timer;
