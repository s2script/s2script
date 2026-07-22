// @s2script/globals — ambient declarations for the globals the engine injects into EVERY plugin
// context. NO runtime code, NO import/export (this is a global/script .d.ts). Included by the
// typecheck gate as a root file so plugins that use these globals WITHOUT importing type-check against
// the real sandbox — NOT lib.dom (the sandbox has no window/document/etc.).

/**
 * The engine-injected console (a subset of the browser/node console). Global — no import needed.
 * @example
 * // plugins/antiflood/src/plugin.ts:48 — write a line to the server console/log
 * console.log("[antiflood] onLoad — chat flood protection active");
 */
declare const console: {
  log(...data: any[]): void;
  error(...data: any[]): void;
  warn(...data: any[]): void;
  info(...data: any[]): void;
};
