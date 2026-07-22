/**
 * @s2script/console — author-time type stubs for the engine console.
 * NO runtime code: the engine injects the implementation at load time.
 */

/** Engine-provided console (log/error/warn/info). Also available as the global `console`. */
export declare const console: {
  /**
   * Write a line to the server console (and the log). Arguments are stringified and space-joined.
   * @example
   * import { console } from "@s2script/sdk/console";
   * console.log("[antiflood] onLoad — chat flood protection active");
   */
  log(...data: any[]): void;
  /** Write a line at error severity. */
  error(...data: any[]): void;
  /** Write a line at warning severity. */
  warn(...data: any[]): void;
  /** Write a line at info severity. */
  info(...data: any[]): void;
};
