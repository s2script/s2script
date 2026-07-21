/**
 * @s2script/eslint-plugin — the s2script residual rule set (north-star §5.3). Flat-config plugin
 * object; `configs` is populated by configs.ts (recommended: editor/projectService; build:
 * s2s-build/provided-program). One implementation, two consumers, zero drift.
 */
import { noCtxEscape } from "./rules/no-ctx-escape.ts";
import { noFloatingPromiseInFactory } from "./rules/no-floating-promise-in-factory.ts";
import { noBigintInInterfacePayloads } from "./rules/no-bigint-in-interface-payloads.ts";
import { noAwaitInRawView } from "./rules/no-await-in-raw-view.ts";
import { recommended, buildConfig } from "./configs.ts";

const plugin = {
  meta: { name: "@s2script/eslint-plugin", version: "0.1.0" },
  rules: {
    "no-ctx-escape": noCtxEscape,
    "no-floating-promise-in-factory": noFloatingPromiseInFactory,
    "no-bigint-in-interface-payloads": noBigintInInterfacePayloads,
    "no-await-in-raw-view": noAwaitInRawView,
  } as Record<string, unknown>,
  configs: {} as {
    recommended?: (opts?: { tsconfigRootDir?: string }) => unknown[];
    build?: (programs: unknown[]) => unknown[];
  },
};

plugin.configs = {
  recommended: (opts?: { tsconfigRootDir?: string }) => recommended(plugin, opts),
  build: (programs: unknown[]) => buildConfig(plugin, programs),
};

export default plugin;
