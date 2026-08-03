// Minimal ambient globals fake for the ISOLATED fake-packages-hooks tree — this directory exists
// only for the gamePackageDeclarationFiles regression test (typecheck.test.mjs) and is deliberately
// NOT the shared fake-packages/ dir: a minimal `sdk/plugin.d.ts` there would shadow the real
// packages/sdk/plugin.d.ts's PluginContext for every OTHER fixture that resolves it (see the
// "subpath" fixture's `ctx.tryUse`), which is exactly the collision this isolation avoids.
declare const console: {
  log(...data: any[]): void;
  error(...data: any[]): void;
  warn(...data: any[]): void;
  info(...data: any[]): void;
};
