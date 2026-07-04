/**
 * @s2script/console — author-time type stubs for the engine console.
 * NO runtime code: the engine injects the implementation at load time.
 */

/** Engine-provided console (log/error/warn/info). Also available as the global `console`. */
export declare const console: {
  log(...data: any[]): void;
  error(...data: any[]): void;
  warn(...data: any[]): void;
  info(...data: any[]): void;
};
