/**
 * Resolve where @s2script/* type stubs live for typecheck.
 *
 * The monorepo `packages/` tree and a plugin's `node_modules/@s2script/` share
 * the same shape (globals/globals.d.ts, <name>/index.d.ts), so both work as a
 * "packagesDir" for the path-mapped typecheck.
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
    existsSync(join(abs, "globals", "globals.d.ts")) ||
    existsSync(join(abs, "entity", "index.d.ts")) ||
    existsSync(join(abs, "frame", "index.d.ts")) ||
    existsSync(join(abs, "commands", "index.d.ts"))
  );
}

/**
 * Walk from the CLI entry URL to find a packages-shaped directory.
 * Works for `dist/cli.js`, `src/cli.ts`, and a published install under
 * `node_modules/@s2script/cli/dist/cli.js` (→ sibling types packages).
 */
export function findPackagesDirNearCli(fromCliUrl: string = import.meta.url): string | undefined {
  const start = dirname(fileURLToPath(fromCliUrl));
  // dist/cli.js → @s2script/cli → @s2script  (or packages/cli → packages)
  // src/cli.ts  → packages/cli/src → packages/cli → packages  (one more ..)
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
    "cannot resolve @s2script/* types: no packages dir found.\n" +
      "  Install @s2script/globals (and other @s2script/* packages) in the plugin,\n" +
      "  or set S2SCRIPT_PACKAGES_DIR / pass --packages-dir."
  );
}
