import type ts from "typescript";

/**
 * The single source of truth for the plugin compiler-options shape, shared by the typecheck
 * gate (`typecheck.ts`, which needs the `ts.CompilerOptions` enum form) and `s2s create`'s
 * scaffolded `tsconfig.json` (which needs the plain-JSON string form an editor's tsserver reads).
 * Keeping one literal here means the two can never drift (design plan Task 6).
 */
export const sharedCompilerOptionsJson = {
  strict: true,
  noEmit: true,
  moduleResolution: "bundler",
  module: "ESNext",
  target: "ES2020",
  lib: ["ES2020"],
  types: [],
  skipLibCheck: true,
  allowImportingTsExtensions: true,
} as const;

/** Map `sharedCompilerOptionsJson` to the `ts.CompilerOptions` enum form the Program API expects.
 *  Takes the `typescript` module as a parameter so this file itself never imports it at the
 *  value level (the JSON form above must stay import-free for `create.ts`'s plain scaffolding). */
export function sharedProgramOptions(tsMod: typeof ts): ts.CompilerOptions {
  return {
    strict: sharedCompilerOptionsJson.strict,
    noEmit: sharedCompilerOptionsJson.noEmit,
    allowImportingTsExtensions: sharedCompilerOptionsJson.allowImportingTsExtensions,
    moduleResolution: tsMod.ModuleResolutionKind.Bundler,
    module: tsMod.ModuleKind.ESNext,
    target: tsMod.ScriptTarget.ES2020,
    lib: sharedCompilerOptionsJson.lib.map((l) => `lib.${l.toLowerCase()}.d.ts`),
    types: [...sharedCompilerOptionsJson.types],
    skipLibCheck: sharedCompilerOptionsJson.skipLibCheck,
  };
}
