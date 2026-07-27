/**
 * Version-gated, dynamic access to the WORKSPACE's own `@changesets/*` internals
 * (design spec 2026-07-27 §7, finding §3.6).
 *
 * `s2s version` does not reimplement changesets' dependents algorithm; it borrows it. Three
 * consequences, each deliberate:
 *
 *   - **Resolved from the workspace root, not from the SDK.** `createRequire` is pointed at the
 *     target workspace's `package.json`, so what runs is the changesets the user already installed
 *     (§3.6: these are all present as transitive deps of `@changesets/cli`). The SDK therefore
 *     gains NO runtime dependency, preserving its zero-runtime-dependency posture, and a repo
 *     cannot end up versioning through one changesets while `npm run version-packages` uses another.
 *   - **Imported dynamically.** The specifier is a resolved absolute path, so esbuild leaves the
 *     `import()` alone rather than trying to bundle changesets into `dist/cli.js`.
 *   - **Version-gated with a NAMED reason.** These are internals, not a stable public API. The
 *     coupling was accepted deliberately during design and THIS GATE IS ITS MITIGATION: an
 *     incompatible changesets fails fast naming the package, the version and the supported range,
 *     rather than drifting silently into a wrong release plan. Never skip it.
 *
 * The types below are hand-written structural mirrors of `@changesets/types`. Importing that
 * package for real would be a fourth coupling point and an undeclared SDK dependency, for types
 * that only ever describe values crossing this one boundary.
 */

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { satisfies } from "semver";

/** Bump kinds, in changesets' own vocabulary. */
export type BumpType = "major" | "minor" | "patch" | "none";

/** One `.changeset/*.md` file, parsed. */
export interface NewChangeset {
  id: string;
  summary: string;
  releases: Array<{ name: string; type: BumpType }>;
}

/** One package the plan releases — `type: "none"` entries exist so ranges can still be rewritten. */
export interface ComprehensiveRelease {
  name: string;
  type: BumpType;
  oldVersion: string;
  newVersion: string;
  changesets: string[];
}

export interface ReleasePlan {
  changesets: NewChangeset[];
  releases: ComprehensiveRelease[];
  preState: unknown;
}

/**
 * The subset of changesets' `Config` this code reads. `updateInternalDependencies` is the one that
 * governs §7 step 5 — everything else is passed straight back through to changesets.
 */
export interface ChangesetsConfig {
  updateInternalDependencies: "patch" | "minor";
  ignore: readonly string[];
  [key: string]: unknown;
}

/** `@manypkg/get-packages`' shape, which is what both changesets entry points consume. */
export interface ChangesetsPackage {
  packageJson: Record<string, unknown>;
  dir: string;
}

export interface ChangesetsPackages {
  tool: string;
  packages: ChangesetsPackage[];
  root: ChangesetsPackage;
}

/** The four calls `s2s version` makes, plus the versions the gate accepted (for reporting). */
export interface ChangesetsApi {
  assembleReleasePlan: (
    changesets: NewChangeset[],
    packages: ChangesetsPackages,
    config: ChangesetsConfig,
    preState: unknown,
  ) => ReleasePlan;
  applyReleasePlan: (
    plan: ReleasePlan,
    packages: ChangesetsPackages,
    config: ChangesetsConfig,
  ) => Promise<string[]>;
  readConfig: (cwd: string, packages: ChangesetsPackages) => Promise<ChangesetsConfig>;
  readChangesets: (rootDir: string, sinceRef?: string) => Promise<NewChangeset[]>;
  readPreState: (cwd: string) => Promise<unknown>;
  /** package name -> the resolved version the gate accepted. */
  versions: Record<string, string>;
}

/**
 * The supported range per package — the mitigation for coupling to unstable internals.
 *
 * `get-release-plan` is deliberately NOT here: `s2s version` replaces it (it is the one that hands
 * the REAL packages to `assembleReleasePlan`, which is exactly what the mirror has to displace).
 * `read` and `pre` are here because `get-release-plan` uses them and so must its replacement.
 */
const SUPPORTED: Record<string, string> = {
  "@changesets/assemble-release-plan": ">=6.0.0 <7.0.0",
  "@changesets/apply-release-plan": ">=7.0.0 <8.0.0",
  "@changesets/config": ">=3.0.0 <4.0.0",
  "@changesets/read": ">=0.6.0 <1.0.0",
  "@changesets/pre": ">=2.0.0 <3.0.0",
};

/**
 * Unwrap a CJS module imported through the ESM interop. Node sets the namespace's `default` to
 * `module.exports`; for a Babel/preconstruct-built CJS bundle that object carries its OWN `default`
 * (the real export). Checking the inner one first is what changesets itself does when it loads a
 * changelog module.
 */
function interopDefault(mod: Record<string, unknown>): unknown {
  const outer = mod.default as Record<string, unknown> | undefined;
  if (outer && typeof outer === "object" && "default" in outer) return outer.default;
  return outer ?? mod;
}

/** A named export off a CJS namespace, tolerating both the flat and the `default`-wrapped shape. */
function named(mod: Record<string, unknown>, key: string): unknown {
  if (typeof mod[key] === "function") return mod[key];
  const outer = mod.default as Record<string, unknown> | undefined;
  return outer?.[key];
}

/**
 * Resolve, gate and load the workspace's changesets. Throws a named error — never a bare
 * `ERR_MODULE_NOT_FOUND` — when a package is missing or out of range, reporting EVERY offender at
 * once (§11) so one `npm install` fixes the whole set.
 */
export async function loadChangesets(workspaceRoot: string): Promise<ChangesetsApi> {
  const require = createRequire(join(workspaceRoot, "package.json"));

  const missing: string[] = [];
  const wrongVersion: string[] = [];
  const resolved = new Map<string, string>();
  const versions: Record<string, string> = {};

  for (const [name, range] of Object.entries(SUPPORTED)) {
    let entry: string;
    let manifest: string;
    try {
      entry = require.resolve(name);
      // Every one of these packages maps "./package.json" in its `exports`, so this is a supported
      // read rather than a reach into internals.
      manifest = require.resolve(`${name}/package.json`);
    } catch {
      missing.push(name);
      continue;
    }
    const version = String((JSON.parse(readFileSync(manifest, "utf8")) as { version?: string }).version ?? "");
    if (!satisfies(version, range, { includePrerelease: true })) {
      wrongVersion.push(`${name}@${version} (supported: ${range})`);
      continue;
    }
    resolved.set(name, entry);
    versions[name] = version;
  }

  if (missing.length > 0 || wrongVersion.length > 0) {
    const lines: string[] = [];
    if (missing.length > 0) {
      lines.push(
        `  not installed: ${missing.join(", ")} — run \`npm install -D @changesets/cli\` at ${workspaceRoot}`,
      );
    }
    for (const w of wrongVersion) lines.push(`  unsupported: ${w}`);
    throw new Error(
      `s2s version cannot use this workspace's changesets:\n${lines.join("\n")}\n` +
        `These are @changesets INTERNALS, not a stable public API — s2s version pins the versions it ` +
        `has been verified against rather than drifting silently into a wrong release plan.`,
    );
  }

  const load = async (name: string): Promise<Record<string, unknown>> =>
    (await import(pathToFileURL(resolved.get(name) as string).href)) as Record<string, unknown>;

  const [assemble, apply, config, read, pre] = await Promise.all([
    load("@changesets/assemble-release-plan"),
    load("@changesets/apply-release-plan"),
    load("@changesets/config"),
    load("@changesets/read"),
    load("@changesets/pre"),
  ]);

  return {
    assembleReleasePlan: interopDefault(assemble) as ChangesetsApi["assembleReleasePlan"],
    applyReleasePlan: interopDefault(apply) as ChangesetsApi["applyReleasePlan"],
    readConfig: named(config, "read") as ChangesetsApi["readConfig"],
    readChangesets: interopDefault(read) as ChangesetsApi["readChangesets"],
    readPreState: named(pre, "readPreState") as ChangesetsApi["readPreState"],
    versions,
  };
}
