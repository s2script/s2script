/**
 * Resolve where @s2script/* type stubs live for typecheck.
 *
 * The monorepo `packages/` tree and a plugin's `node_modules/@s2script/` share
 * the same shape (sdk/globals.d.ts, sdk/<cap>.d.ts, cs2/index.d.ts), so both work
 * as a "packagesDir" for the path-mapped typecheck.
 *
 * Priority:
 *   1. explicit packagesDir / --packages-dir
 *   2. S2SCRIPT_PACKAGES_DIR env
 *   3. monorepo packages/ next to this CLI install (in-tree)
 *   4. <pluginDir>/node_modules/@s2script
 */
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** True when `dir` looks like the monorepo packages/ tree, a test fake, or node_modules/@s2script. */
export function isPackagesDir(dir: string): boolean {
  const abs = resolve(dir);
  return (
    existsSync(join(abs, "sdk", "globals.d.ts")) ||
    existsSync(join(abs, "sdk", "entity.d.ts"))
  );
}

/**
 * Walk from the CLI entry URL to find a packages-shaped directory.
 * Works for `dist/cli.js`, `src/cli.ts`, and a published install under
 * `node_modules/@s2script/sdk/dist/cli.js` (→ sibling types packages).
 */
export function findPackagesDirNearCli(fromCliUrl: string = import.meta.url): string | undefined {
  const start = dirname(fileURLToPath(fromCliUrl));
  // dist/cli.js → packages/sdk/dist → packages/sdk → packages
  // src/cli.ts  → packages/sdk/src → packages/sdk → packages  (one more ..)
  for (const rel of ["../..", "../../.."] as const) {
    const candidate = join(start, rel);
    if (isPackagesDir(candidate)) return resolve(candidate);
  }
  return undefined;
}

export function resolvePackagesDir(opts?: {
  explicit?: string;
  pluginDir?: string;
  fromCliUrl?: string;
}): string {
  if (opts?.explicit) {
    const abs = resolve(opts.explicit);
    if (!isPackagesDir(abs)) {
      throw new Error(`packages dir does not look like @s2script stubs: ${abs}`);
    }
    return abs;
  }
  const env = process.env.S2SCRIPT_PACKAGES_DIR;
  if (env) {
    const abs = resolve(env);
    if (!isPackagesDir(abs)) {
      throw new Error(`S2SCRIPT_PACKAGES_DIR does not look like @s2script stubs: ${abs}`);
    }
    return abs;
  }
  const nearCli = findPackagesDirNearCli(opts?.fromCliUrl);
  if (nearCli) return nearCli;

  if (opts?.pluginDir) {
    const nm = join(resolve(opts.pluginDir), "node_modules", "@s2script");
    if (isPackagesDir(nm)) return nm;
  }

  throw new Error(
    "cannot resolve @s2script/sdk/* types: no packages dir found.\n" +
      "  Install `@s2script/sdk` in the plugin (npm i -D @s2script/sdk),\n" +
      "  or set S2SCRIPT_PACKAGES_DIR / pass --packages-dir."
  );
}
