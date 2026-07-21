/**
 * The two faces of ONE rule set (north-star §5.3 — parity by construction):
 *  - recommended(): editor flat config (tsserver-independent; projectService reads the plugin's
 *    own tsconfig.json — the same file the tsc gate's semantics are mirrored into).
 *  - build(programs): what `s2s build` runs in-process, parsing against the ALREADY-BUILT
 *    typecheck-gate program — byte-identical module resolution to the tsc gate, zero extra
 *    program construction, works for in-repo plugins with no eslint.config of their own.
 * Same rules, same severities, same parser — only the type-info source differs.
 */
import tsParser from "@typescript-eslint/parser";

const RULES = {
  "s2script/no-ctx-escape": "error",
  "s2script/no-floating-promise-in-factory": "error",
  "s2script/no-bigint-in-interface-payloads": "error",
  "s2script/no-await-in-raw-view": "error",
} as const;

const IGNORES = { ignores: ["dist/**", "node_modules/**"] };

export function recommended(plugin: unknown, opts?: { tsconfigRootDir?: string }): unknown[] {
  return [
    IGNORES,
    {
      files: ["**/*.ts"],
      languageOptions: {
        parser: tsParser,
        parserOptions: {
          projectService: true,
          ...(opts?.tsconfigRootDir !== undefined ? { tsconfigRootDir: opts.tsconfigRootDir } : {}),
        },
      },
      plugins: { s2script: plugin },
      rules: RULES,
    },
  ];
}

export function buildConfig(plugin: unknown, programs: unknown[]): unknown[] {
  return [
    IGNORES,
    {
      files: ["**/*.ts"],
      languageOptions: {
        parser: tsParser,
        parserOptions: { programs },
      },
      plugins: { s2script: plugin },
      rules: RULES,
    },
  ];
}
