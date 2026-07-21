/**
 * @s2script/eslint-plugin — the s2script residual rule set (north-star §5.3). Flat-config plugin
 * object; `configs` is populated by configs.ts (recommended: editor/projectService; build:
 * s2s-build/provided-program). One implementation, two consumers, zero drift.
 */
import { noCtxEscape } from "./rules/no-ctx-escape.ts";

const plugin = {
  meta: { name: "@s2script/eslint-plugin", version: "0.1.0" },
  rules: {
    "no-ctx-escape": noCtxEscape,
  } as Record<string, unknown>,
  configs: {} as {
    recommended?: (opts?: { tsconfigRootDir?: string }) => unknown[];
    build?: (programs: unknown[]) => unknown[];
  },
};

export default plugin;
