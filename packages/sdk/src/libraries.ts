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
import { findWorkspaceRoot, scanWorkspace, memberForDir } from "./workspace/workspace.ts";
import type { WorkspaceMember } from "./workspace/workspace.ts";

export type PackageKind = "plugin" | "library";

/**
 * Absent means "plugin", so no existing package.json anywhere needs editing.
 *
 * Lives here (not build-library.ts, which re-exports it for its existing callers) because
 * `resolveLibrarySibling` below needs it too, and build-library.ts already imports FROM
 * libraries.ts — putting it there would make the two modules import each other.
 */
export function packageKind(pkg: { s2script?: { kind?: string } }): PackageKind {
  const k = pkg.s2script?.kind ?? "plugin";
  if (k !== "plugin" && k !== "library") {
    throw new Error(`unknown s2script.kind ${JSON.stringify(k)} — expected "plugin" or "library"`);
  }
  return k;
}

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
 *     — searched across BOTH `ws.libs` (the common `libs/*` case) and `ws.plugins` (a `kind:
 *     "library"` member the workspace's OWN `s2script.workspace.plugins` glob also matches —
 *     `loadWorkspace` does not filter by kind, so this topology is real and already
 *     build-and-publish-able, see `workspace/build-all.ts`'s `declaredLibraries` doc comment and
 *     the `ws-library-in-plugins-glob` fixture; searching `ws.libs` alone reported such a library
 *     "missing", with advice pointing at the registry for something sitting two directories away).
 *     A plain shared-code member with no `kind` at all, in either bucket, is not a library either:
 *     a name collision alone must not silently start bundling the wrong thing in.
 *
 * SCANNED, not loaded (`scanWorkspace`, not `loadWorkspace`): this is reached from
 * `assertLibrariesResolved` inside `buildPlugin`, the SINGLE-TARGET build path — `s2s build
 * plugins/healthy-plugin`. `loadWorkspace` aggregates and throws on ANY workspace-wide shape
 * problem, so a typo in some unrelated plugin's package.json would break a healthy plugin's build
 * merely for declaring a library. `workspace/siblings.ts` applies the identical reasoning (and the
 * identical fix) to sibling CONTRACT resolution on this same path; whole-workspace shape
 * validation stays `preflightWorkspace`'s job, which every workspace-mode command already runs
 * before it builds anything. Degrade per-descriptor, never crash globally.
 */
function resolveLibrarySibling(
  pluginDir: string,
  name: string,
): { dir: string; typesPath: string; version: string } | null {
  const absDir = resolve(pluginDir);
  const root = findWorkspaceRoot(absDir);
  if (root === null) return null;

  const scan = scanWorkspace(root);
  const ws = scan.workspace;
  if (memberForDir(ws, absDir) === null) return null;

  // Filter by NAME first, THEN check kind — packageKind THROWS on an unknown kind (design D6:
  // kind is explicit, never inferred), and doing it the other way around (checking every member's
  // kind while searching) would abort this whole resolution over an unrelated member's typo
  // anywhere in the workspace. Scoping the throw to name-matching candidates keeps the blast
  // radius to "a sibling actually named the thing being declared", which is the one case where
  // silently falling through to "missing" would report the WRONG problem (a kind typo, not an
  // absent library).
  const named = [...ws.libs, ...ws.plugins].filter((m) => m.name === name);
  let lib: WorkspaceMember | undefined;
  for (const m of named) {
    let kind: PackageKind;
    try {
      kind = packageKind(m.pkg);
    } catch (e) {
      throw new Error(`library ${name} (workspace member at ${m.relDir}): ${(e as Error).message}`);
    }
    if (kind === "library") { lib = m; break; }
  }
  if (lib === undefined) {
    // A member the scan could not read is ABSENT from `ws.libs`, so it is also absent from this
    // search — a dropped member may have been the very library being declared, and that would
    // silently fall through to "missing" (and from there to the hard `s2s add` error) instead of
    // naming the real problem. Mirrors `workspace/siblings.ts`'s identical "unread" warning.
    if (scan.problems.length > 0) {
      console.warn(
        `WARN: ${name}: no workspace sibling found, but ${scan.problems.length} workspace ` +
          `member${scan.problems.length === 1 ? "" : "s"} could not be read — one of them may be ` +
          `this library, in which case it will be reported as missing instead. Fix:\n  ` +
          scan.problems.join("\n  "),
      );
    }
    return null;
  }

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

/**
 * The major component of an `apiVersion` string like `"2.x"`. `Number.parseInt` returns `NaN` for
 * an unparseable/empty leading segment (the `""` sentinel `LibraryManifest` uses for a workspace
 * source, see its own doc) — that case defaults to 1 (treat as compatible), which is a real
 * decision, not an accident: `resolveLibraries` already defaults an unreadable vendored
 * manifest's apiVersion to `"1.x"` for the identical reason.
 *
 * The fallback is applied ONLY to that NaN case, deliberately — an earlier version wrote
 * `parseInt(...) || 1`, which ALSO rewrites a genuinely-parsed `0` into `1` (`0` is falsy in JS),
 * silently collapsing a real major-0 apiVersion into major-1. That is a live bug, not a
 * hypothetical one: a real 0 never fires the gate's "newer than this SDK" comparison it should,
 * and (found while fixing the vacuous test at libraries.test.mjs — see the "workspace sibling"
 * test below) a real 0 can never be produced from a `major()` call either, so nothing could ever
 * prove the workspace-sibling skip is load-bearing rather than coincidentally never triggered.
 */
function major(apiVersion: string): number {
  const n = Number.parseInt(apiVersion.split(".")[0] ?? "1", 10);
  return Number.isNaN(n) ? 1 : n;
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
  // @s2script/* is reserved for first-party publishing (CLAUDE.md) — that is not a restriction on
  // who may PUBLISH a library (design spec 2026-07-29), but it IS the one place the scope is
  // special: `typecheck.ts`'s `isAlwaysResolved` treats every `@s2script/*` name as a runtime
  // builtin, unconditionally, and `build.ts`/`build-library.ts` leave `@s2script/*` external in
  // their esbuild call for the identical reason. A `paths` entry for a declared library under this
  // scope would beat that prefix pattern (tsc picks the longest/most specific match) and typecheck
  // clean against the library's real .d.ts — but esbuild still externalizes it, so the bundle ships
  // a bare `require("@s2script/…")` with none of the library's code inlined, and it dies at load.
  // Refuse it outright rather than let tsc and esbuild silently disagree about what the name means.
  const reserved = Object.keys(declared).filter((n) => n === "@s2script" || n.startsWith("@s2script/"));
  if (reserved.length > 0) {
    throw new Error(
      `s2script.libraries declares ${reserved.map((n) => JSON.stringify(n)).join(", ")} — the ` +
        `@s2script/* scope is reserved and always resolved as a runtime builtin, so it can never ` +
        `be a bundled library`,
    );
  }

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
