// Consolidated @s2script/sdk globals fake — the gate injects this as a rootName once
// packages/sdk/globals.d.ts exists (existsSync check). Script (NO import/export) so these are
// ambient globals, superset of the legacy fake globals: the console-using fixtures
// (clean/broken/canary-*) still resolve `console` here, plus HookResult.

/** The engine-injected console (a subset of the browser/node console). */
declare const console: {
  log(...data: any[]): void;
  error(...data: any[]): void;
  warn(...data: any[]): void;
  info(...data: any[]): void;
};

declare const HookResult: { Continue: 0; Changed: 1; Handled: 2; Stop: 3 };
