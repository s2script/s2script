/**
 * The `s2script.libraries` field: build-time packages a plugin bundles INTO its own `.s2sp`.
 *
 * Two resolution sources, tried in this order:
 *
 *   1. a vendored copy at `.s2script/libs/<name>/` — mirrors the verified-copy convention for
 *      contracts (contracts.ts) rather than npm: the tree is committed, so builds are reproducible
 *      and offline, and no lockfile is needed. This is the registry-distributed case: a library
 *      pulled via `s2s add` and vendored at publish-time, pre-bundled, so there is no transitive
 *      graph to resolve here.
 *
 *   2. a workspace sibling whose own package.json opts in with `s2script.kind === "library"` —
 *      the monorepo case. `.s2script/libs/` is right for a library pulled from the registry, but
 *      re-vendoring a sibling's copy after every edit is untenable inside a workspace, so a sibling
 *      resolves straight to its own `main`/`types` on disk — no copy anywhere, same "one copy of
 *      the bytes" doctrine `workspace/siblings.ts` applies to plugin contracts. A sibling has no
 *      published artifact to stamp an `apiVersion` on, so `assertLibrariesResolved` skips that
 *      gate for it (see the `source` field below).
 *
 * A declared library resolved by neither source is a hard error — never an ambient `any` stub.
 * This mirrors the doctrine typecheck.ts applies to builtins: a miss is a real error.
 */

import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { findWorkspaceRoot, loadWorkspace, memberForDir } from "./workspace/workspace.ts";

export interface LibraryManifest {
  version: string;
  /**
   * The apiVersion the library was built against. A workspace sibling has no published artifact to
   * read one from — "" is a sentinel, never compared (see `source`).
   */
  apiVersion: string;
  /**
   * Which resolution source produced this entry. `assertLibrariesResolved`'s apiVersion gate
   * applies only to "vendored": a "workspace" sibling is built straight from source, not from a
   * versioned artifact, so there is nothing to gate.
   */
  source: "vendored" | "workspace";
}

export interface ResolvedLibraries {
  /** tsc `paths` entries: library name -> [absolute .d.ts]. */
  paths: Record<string, string[]>;
  /** Declared but resolved by neither source. */
  missing: string[];
  manifests: Record<string, LibraryManifest>;
  /**
   * name -> absolute directory, WORKSPACE-resolved entries only. `build.ts` esbuild-aliases each
   * one straight at this directory so the sibling's actual source (not a vendored artifact) gets
   * bundled in. A vendored entry needs no alias: its directory is already named for the package
   * (`.s2script/libs/<name>`), which esbuild's own `nodePaths` resolution finds unaided.
   */
  siblingDirs: Record<string, string>;
}

export function librariesRoot(pluginDir: string): string {
  return join(pluginDir, ".s2script", "libs");
}

/**
 * Absolute dir of a VENDORED library, or null (absent / traversal-unsafe).
 * Vendored only — see `resolveLibrarySibling` below for the workspace source.
 */
export function localLibraryDir(pluginDir: string, name: string): string | null {
  const segs = name.split("/");
  if (segs.some((s) => s === "" || s === "." || s === "..")) return null;
  const p = join(librariesRoot(pluginDir), ...segs);
  return existsSync(join(p, "index.d.ts")) ? p : null;
}

/**
 * Resolve `name` to a workspace sibling library, or null.
 *
 * Three things must hold, matched against a real failure mode each:
 *   - `pluginDir` sits inside an s2script workspace at all (`findWorkspaceRoot`) — otherwise
 *     there is no sibling tree to search, the single-plugin path, unchanged.
 *   - `pluginDir` is itself a member that workspace COVERS (`memberForDir`) — the same
 *     blast-radius guard `workspace/siblings.ts` applies to plugin contracts: a tree that merely
 *     sits BENEATH a workspace root (an `examples/*` dir, an SDK test fixture) must not go
 *     sibling-hunting for a name that happens to collide with some unrelated member elsewhere in
 *     that tree.
 *   - a workspace member's own `name` matches AND it opted in with `s2script.kind === "library"`
 *     — `ws.libs` already excludes every plugin (a plugin sibling sharing the name is therefore
 *     never a candidate), and a plain shared-code member with no `kind` at all is not a library
 *     either: a name collision alone must not silently start bundling the wrong thing in.
 */
function resolveLibrarySibling(
  pluginDir: string,
  name: string,
): { dir: string; typesPath: string; version: string } | null {
  const absDir = resolve(pluginDir);
  const root = findWorkspaceRoot(absDir);
  if (root === null) return null;

  const ws = loadWorkspace(root);
  if (memberForDir(ws, absDir) === null) return null;

  const lib = ws.libs.find((m) => m.name === name && m.pkg.s2script?.kind === "library");
  if (lib === undefined) return null;

  const typesRel = lib.pkg.types;
  if (typeof typesRel !== "string" || typesRel === "") {
    throw new Error(
      `library ${name} (workspace sibling at ${lib.relDir}) has no "types" in its package.json`,
    );
  }
  const typesPath = resolve(lib.dir, typesRel);
  if (!existsSync(typesPath)) {
    throw new Error(
      `library ${name} (workspace sibling at ${lib.relDir}) types file not found: ${typesRel}`,
    );
  }
  return { dir: lib.dir, typesPath, version: lib.version };
}

export function resolveLibraries(
  pluginDir: string,
  declared: Record<string, string>,
): ResolvedLibraries {
  const paths: Record<string, string[]> = {};
  const manifests: Record<string, LibraryManifest> = {};
  const missing: string[] = [];
  const siblingDirs: Record<string, string> = {};

  for (const name of Object.keys(declared)) {
    const dir = localLibraryDir(pluginDir, name);
    if (dir !== null) {
      paths[name] = [join(dir, "index.d.ts")];
      let version = "0.0.0";
      let apiVersion = "1.x";
      try {
        const m = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
        if (typeof m.version === "string") version = m.version;
        if (typeof m.s2script?.apiVersion === "string") apiVersion = m.s2script.apiVersion;
      } catch {
        // A vendored tree without a readable package.json still typechecks and bundles;
        // only the apiVersion gate below loses its input, which defaults to compatible.
      }
      manifests[name] = { version, apiVersion, source: "vendored" };
      continue;
    }

    const sib = resolveLibrarySibling(pluginDir, name);
    if (sib !== null) {
      paths[name] = [sib.typesPath];
      manifests[name] = { version: sib.version, apiVersion: "", source: "workspace" };
      siblingDirs[name] = sib.dir;
      continue;
    }

    missing.push(name);
  }
  return { paths, missing, manifests, siblingDirs };
}

function major(apiVersion: string): number {
  return Number.parseInt(apiVersion.split(".")[0] ?? "1", 10) || 1;
}

/**
 * Fail the build on a declared-but-unresolvable library or an api-major mismatch.
 * A missing library must NEVER degrade to an ambient `any` stub — the same doctrine
 * typecheck.ts applies to builtins: a miss is a real error, not a silent `any`.
 */
export function assertLibrariesResolved(
  pluginDir: string,
  declared: Record<string, string>,
  sdkApiVersion: string,
): ResolvedLibraries {
  const resolved = resolveLibraries(pluginDir, declared);
  if (resolved.missing.length) {
    throw new Error(
      `declared in s2script.libraries but not vendored under .s2script/libs and no workspace ` +
        `sibling found:\n` +
        resolved.missing.map((n) => `  ${n} — run \`s2s add ${n}\``).join("\n"),
    );
  }
  const sdkMajor = major(sdkApiVersion);
  for (const [name, m] of Object.entries(resolved.manifests)) {
    // A workspace sibling has no published apiVersion to compare against (see LibraryManifest) —
    // skip the gate rather than compare against the "" sentinel.
    if (m.source === "workspace") continue;
    if (major(m.apiVersion) > sdkMajor) {
      throw new Error(
        `library ${name}@${m.version} was built against apiVersion ${m.apiVersion}, ` +
          `newer than this SDK's ${sdkApiVersion} — upgrade @s2script/sdk`,
      );
    }
  }
  return resolved;
}
