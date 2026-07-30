/**
 * The workspace model (design spec 2026-07-27 §4): a directory holding several plugins that
 * build and publish together.
 *
 * The ONLY thing that makes a directory a workspace root is `s2script.workspace` in its
 * package.json — plugin package.json files carry no marker at all. npm's own `workspaces`
 * field says which directories are packages (and therefore which get the node_modules symlink
 * that makes sibling contract resolution work); the `s2script.workspace.plugins` globs say
 * which of those packages are plugins. Standard npm fields for what npm models, the `s2script`
 * block for the engine facts — the standing convention.
 *
 * `findWorkspaceRoot` returning null is the backward-compatibility hinge: every command then
 * behaves exactly as it does today, in single-plugin mode.
 */

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { matchSegment, splitSegments, toPosix } from "./glob.ts";

/** The fields of a package.json this model reads. Everything else passes through in `pkg`. */
export interface WorkspacePackageJson {
  name?: string;
  version?: string;
  main?: string;
  types?: string;
  private?: boolean;
  /** npm's own field. The object form (`{ packages: [...] }`) is accepted too — npm honours it. */
  workspaces?: string[] | { packages?: string[] };
  s2script?: {
    main?: string;
    /** The root marker. Its presence — not its contents — is what defines a workspace root. */
    workspace?: { plugins?: string[] };
    pluginDependencies?: Record<string, string>;
    optionalPluginDependencies?: Record<string, string>;
    publishes?: string | Record<string, string>;
    /** Absent means "plugin" — see `packageKind` (libraries.ts), which `resolveLibrarySibling` and
     *  `workspace/build-all.ts`'s `declaredLibraries` call on a `WorkspaceMember.pkg`. Declared here
     *  (not left to the index signature below) so that call satisfies `packageKind`'s parameter
     *  type — a `[key: string]: unknown` index signature does not, by itself, give TS a property
     *  "in common" with `{ kind?: string }` for its weak-type assignability check. */
    kind?: string;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

/** A workspace member: a directory npm `workspaces` covers and that carries a package.json. */
export interface WorkspaceMember {
  /** package.json `name` — a plugin's id, a lib's import specifier. */
  name: string;
  version: string;
  /** Absolute directory. */
  dir: string;
  /** Directory relative to the workspace root, always `/`-separated. */
  relDir: string;
  pkg: WorkspacePackageJson;
}

/** A member matched by `s2script.workspace.plugins`. */
export interface WorkspacePlugin extends WorkspaceMember {
  /** `s2script.main ?? main`, relative to `dir`. Guaranteed present — §4.3 rule 2 is a hard error. */
  entry: string;
  /** npm's own `private`. `private: true` = built, never published (§9.1). */
  private: boolean;
}

export interface Workspace {
  /** Absolute path of the directory carrying the `s2script.workspace` marker. */
  root: string;
  /** The root package.json, already parsed. */
  pkg: WorkspacePackageJson;
  /** Plugins, ordered by directory (deterministic; dependency order comes from graph.ts). */
  plugins: WorkspacePlugin[];
  /** Every other workspace member — shared libraries, bundled into each .s2sp at build time. */
  libs: WorkspaceMember[];
}

/** Parse a package.json, naming the file on a syntax error rather than throwing `Unexpected token`. */
function readPackageJson(path: string): WorkspacePackageJson {
  try {
    return JSON.parse(readFileSync(path, "utf8")) as WorkspacePackageJson;
  } catch (e) {
    throw new Error(`cannot read ${path}: ${(e as Error).message}`);
  }
}

/**
 * The textual shape of the root marker, for the one case that cannot be parsed: a package.json
 * that carries `s2script.workspace` but does not parse. Deliberately loose — it decides only
 * whether an unparseable file is worth failing on, and a false positive is a named error on a file
 * that is broken either way.
 */
const MARKER_TEXT = /"s2script"[\s\S]*"workspace"\s*:/;

/**
 * Does the package.json at `pkgPath` carry the `s2script.workspace` marker?
 *
 * An unreadable or unparseable package.json is WALKED PAST, not fatal. §4.2 makes
 * "`findWorkspaceRoot` returns null" the backward-compatibility hinge, and the walk-up passes over
 * files the plugin author does not own — an ancestor three directories up with a stray comma must
 * never break `s2s build ./standalone-plugin`, which today it does, for a plugin in no workspace
 * at all.
 *
 * The ONE file whose parse failure is still fatal is a file that actually carries the marker:
 * degrading the real workspace root to single-plugin mode over a stray comma is exactly the silent
 * failure this design exists to remove. A file that will not parse cannot be asked, so its raw
 * text is matched for the marker instead.
 */
function carriesWorkspaceMarker(pkgPath: string): boolean {
  let text: string;
  try {
    text = readFileSync(pkgPath, "utf8");
  } catch {
    return false; // absent, a directory, unreadable — not this walk's problem
  }
  try {
    return (JSON.parse(text) as WorkspacePackageJson | null)?.s2script?.workspace !== undefined;
  } catch (e) {
    if (MARKER_TEXT.test(text)) throw new Error(`cannot read ${pkgPath}: ${(e as Error).message}`);
    return false;
  }
}

/**
 * Walk UP from `from` looking for a package.json carrying `s2script.workspace`; null when there
 * is none (single-plugin mode, unchanged). The FIRST marker wins, so a nested workspace — the
 * `examples/monorepo/` case — resolves to itself rather than to the repo above it.
 */
export function findWorkspaceRoot(from: string): string | null {
  let dir = resolve(from);
  for (;;) {
    if (carriesWorkspaceMarker(join(dir, "package.json"))) return dir;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/** True when `path` is a directory (following symlinks — a workspace member may be one). */
function isDir(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

/** Immediate subdirectories of `dir`, excluding the ones a wildcard must never descend into. */
function subdirs(dir: string): string[] {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return []; // a glob prefix that does not exist matches nothing; it is not an error
  }
  return entries
    // node_modules is where npm PUTS the workspace symlinks, so walking into it would rediscover
    // every member under a second path. Dot-directories are tool state, never packages. A literal
    // pattern segment still reaches both — this filter only applies to wildcard expansion.
    .filter((e) => e.name !== "node_modules" && !e.name.startsWith("."))
    .filter((e) => e.isDirectory() || (e.isSymbolicLink() && isDir(join(dir, e.name))))
    .map((e) => e.name);
}

function joinRel(rel: string, name: string): string {
  return rel === "" ? name : `${rel}/${name}`;
}

/**
 * Expand one glob against the directories actually on disk, relative to `root` and posix-separated.
 * Directories only: both npm `workspaces` and `s2script.workspace.plugins` name packages.
 */
export function expandDirGlob(root: string, pattern: string): string[] {
  const segments = splitSegments(pattern);
  const out = new Set<string>();

  const walk = (index: number, rel: string): void => {
    if (index === segments.length) {
      if (rel !== "") out.add(rel);
      return;
    }
    const head = segments[index];
    const here = rel === "" ? root : join(root, rel);
    if (head === "**") {
      walk(index + 1, rel); // `**` matches zero segments too
      for (const name of subdirs(here)) walk(index, joinRel(rel, name));
      return;
    }
    if (!head.includes("*")) {
      const next = joinRel(rel, head);
      if (isDir(join(root, next))) walk(index + 1, next);
      return;
    }
    for (const name of subdirs(here)) {
      if (matchSegment(head, name)) walk(index + 1, joinRel(rel, name));
    }
  };

  walk(0, "");
  return [...out].sort();
}

/** npm accepts both the array form and `{ packages: [...] }`. */
/**
 * The `packages:` globs from `pnpm-workspace.yaml`, or `[]` when there is no such file.
 *
 * Deliberately a line scanner rather than a YAML dependency: the file's schema for this key is a
 * flat list of scalars, which is the one YAML shape that needs no parser. Anything more exotic
 * (anchors, flow sequences, multi-document) simply yields no globs, and the caller then reports the
 * plugin as uncovered — the same named error a genuinely-unlisted directory gets, never a silent
 * miss. Quotes are optional in YAML, so both `- "plugins/*"` and `- plugins/*` are accepted.
 */
export function pnpmWorkspaceGlobs(root: string): string[] {
  const path = join(root, "pnpm-workspace.yaml");
  if (!existsSync(path)) return [];
  let body: string;
  try {
    body = readFileSync(path, "utf8");
  } catch {
    return [];
  }
  const globs: string[] = [];
  let inPackages = false;
  for (const raw of body.split(/\r?\n/)) {
    const line = raw.replace(/#.*$/, "").trimEnd();
    if (/^packages\s*:/.test(line)) {
      inPackages = true;
      continue;
    }
    if (!inPackages) continue;
    const item = /^\s+-\s*(.+)$/.exec(line);
    if (item === null) {
      // A non-indented, non-item line ends the block; a blank line does not.
      if (line.trim() !== "") break;
      continue;
    }
    globs.push(item[1].trim().replace(/^['"]|['"]$/g, ""));
  }
  return globs;
}

export function workspaceGlobs(field: WorkspacePackageJson["workspaces"]): string[] {
  if (Array.isArray(field)) return field.filter((g) => typeof g === "string");
  if (field && Array.isArray(field.packages)) return field.packages.filter((g) => typeof g === "string");
  return [];
}

/** A workspace as read off disk, with every SHAPE problem collected rather than thrown. */
export interface WorkspaceScan {
  /** The coherent part: every member that had no problem of its own. */
  workspace: Workspace;
  /** The §4.3 rule violations, attributed and report-ready. Empty for a coherent workspace. */
  problems: string[];
}

/**
 * Read the workspace at `root`, COLLECTING the §4.3 rule violations instead of throwing.
 *
 * `loadWorkspace` is the aggregate-and-throw wrapper and is what every workspace-mode command
 * calls. This lenient form exists for one caller: sibling contract resolution, which runs on the
 * single-plugin path too (`s2s build plugins/one-plugin`). §4.2 promises that path keeps working
 * inside a workspace, and the standing rule is degrade per-descriptor, never crash globally — so
 * one malformed sibling elsewhere in the tree must not fail a targeted build of a healthy plugin.
 * Whole-workspace shape validation belongs to `preflightWorkspace`, which the workspace-mode paths
 * already run before they build anything.
 *
 * Problems that are properties of the ROOT itself still throw: without the marker or the plugin
 * globs there is no workspace to be lenient about.
 */
export function scanWorkspace(root: string): WorkspaceScan {
  return readWorkspace(root);
}

/**
 * Load the workspace rooted at `root`.
 *
 * The three §4.3 discovery rules, each chosen against a named failure mode:
 *
 *   1. glob match, NO package.json           -> skip silently. It is just a directory:
 *      `plugins/disabled/` matches `plugins/*`. Mirrors build-base-plugins.sh's `[ -f … ]` guard.
 *   2. glob match, package.json, NO entry    -> HARD ERROR. Silent-skip is how you ship 14 of 15
 *      plugins and never notice.
 *   3. plugin dir not covered by `workspaces` -> HARD ERROR. Without npm's symlink there is no
 *      sibling resolution, and the failure mode is a SILENT degradation to `any` rather than a
 *      diagnostic — the one thing this design exists to remove.
 *
 * Every violation is collected and reported at once (spec §11), never the first one only.
 */
export function loadWorkspace(root: string): Workspace {
  const { workspace, problems } = readWorkspace(root);
  if (problems.length > 0) {
    throw new Error(
      `workspace at ${workspace.root} has ${problems.length} problem${problems.length === 1 ? "" : "s"}:\n  ` +
        problems.join("\n  "),
    );
  }
  return workspace;
}

/** The shared reader behind `loadWorkspace` (throws) and `scanWorkspace` (collects). */
function readWorkspace(root: string): WorkspaceScan {
  const absRoot = resolve(root);
  const pkg = readPackageJson(join(absRoot, "package.json"));
  const marker = pkg.s2script?.workspace;
  if (marker === undefined) {
    throw new Error(
      `loadWorkspace: ${absRoot} is not a workspace root — its package.json has no s2script.workspace block`,
    );
  }
  const pluginGlobs = marker.plugins;
  if (!Array.isArray(pluginGlobs) || pluginGlobs.length === 0 || pluginGlobs.some((g) => typeof g !== "string")) {
    throw new Error(
      `loadWorkspace: s2script.workspace.plugins must be a non-empty array of globs naming which ` +
        `workspace packages are plugins (e.g. ["plugins/*"])`,
    );
  }

  // Workspace members, from EITHER manifest. npm/yarn/bun declare them in package.json
  // `workspaces`; pnpm uses `pnpm-workspace.yaml` and leaves that field absent entirely. Reading
  // only the npm field made every pnpm workspace fail rule 3 with advice naming a field pnpm users
  // deliberately do not have. Union rather than either/or: a repo may carry both (pnpm ignores the
  // npm field, so people often keep it for tooling that reads it).
  const memberDirs = new Set<string>();
  for (const glob of [...workspaceGlobs(pkg.workspaces), ...pnpmWorkspaceGlobs(absRoot)]) {
    for (const rel of expandDirGlob(absRoot, glob)) memberDirs.add(rel);
  }

  const problems: string[] = [];
  const plugins: WorkspacePlugin[] = [];
  const pluginDirs = new Set<string>();

  const candidates = new Set<string>();
  for (const glob of pluginGlobs) {
    for (const rel of expandDirGlob(absRoot, glob)) candidates.add(rel);
  }

  for (const rel of [...candidates].sort()) {
    const dir = join(absRoot, rel);
    const pkgPath = join(dir, "package.json");
    if (!existsSync(pkgPath)) continue; // rule 1
    pluginDirs.add(rel);

    let pluginPkg: WorkspacePackageJson;
    try {
      pluginPkg = readPackageJson(pkgPath);
    } catch (e) {
      problems.push(`${rel}: ${(e as Error).message}`);
      continue;
    }

    if (typeof pluginPkg.name !== "string" || pluginPkg.name === "") {
      problems.push(`${rel}: package.json has no "name" — a plugin's name is its id on the wire`);
      continue;
    }
    const entry = pluginPkg.s2script?.main ?? pluginPkg.main;
    if (typeof entry !== "string" || entry === "") {
      // Rule 2.
      problems.push(
        `${rel}: matches s2script.workspace.plugins but has no entry point (set s2script.main or main in its package.json)`,
      );
      continue;
    }
    if (!memberDirs.has(rel)) {
      // Rule 3, now a WARNING rather than a hard error.
      //
      // It was an error because sibling resolution rode npm's `node_modules` symlink, so an
      // uncovered plugin degraded silently to an `any` stub. `typecheck.ts` no longer uses the
      // symlink at all — it maps each interface straight to the producer's contract — so the
      // failure mode that justified failing the build no longer exists, and keeping the error
      // would reject workspaces that now work perfectly well.
      //
      // Still worth saying: an uncovered directory gets no install, no hoisting and no editor
      // resolution for its OTHER (npm) dependencies, which is nearly always an oversight in the
      // manifest rather than a deliberate choice.
      console.warn(
        `WARN: ${rel}: is a plugin but is not listed in package.json "workspaces" or ` +
          `pnpm-workspace.yaml — s2script builds it either way, but your package manager will not ` +
          `install or link its dependencies`,
      );
    }

    plugins.push({
      name: pluginPkg.name,
      version: typeof pluginPkg.version === "string" ? pluginPkg.version : "0.0.0",
      dir,
      relDir: rel,
      pkg: pluginPkg,
      entry,
      private: pluginPkg.private === true,
    });
  }

  // §4.3's fourth failure mode, and the one that hid the longest: a plugin's `name` is its ID —
  // on the wire, in `--filter`, in the dependency graph, and in the loader. Two plugins sharing a
  // name do not merely make a message ambiguous, they COLLAPSE: `topoOrder` keys indegree by name,
  // so a two-plugin workspace orders one, `buildWorkspace` builds one, the command exits 0, and
  // the only trace is a nonsense "plugin dependency cycle ()" pointing at the wrong problem. Rule
  // 2 chose a hard error over exactly this class of silence, so this is one too.
  const firstByName = new Map<string, string>();
  for (const p of plugins) {
    const first = firstByName.get(p.name);
    if (first === undefined) {
      firstByName.set(p.name, p.relDir);
      continue;
    }
    problems.push(
      `${p.relDir}: duplicate plugin name ${JSON.stringify(p.name)} — already declared by ${first}. ` +
        `A plugin's name is its id on the wire, so the two cannot coexist: the build order keys on ` +
        `it and would silently ship one of the two.`,
    );
  }

  // Everything npm covers that is not a plugin is a shared library. A member without a package.json
  // is skipped for the same reason rule 1 skips one: npm ignores it too.
  const libs: WorkspaceMember[] = [];
  for (const rel of [...memberDirs].sort()) {
    if (pluginDirs.has(rel)) continue;
    const dir = join(absRoot, rel);
    const pkgPath = join(dir, "package.json");
    if (!existsSync(pkgPath)) continue;
    let libPkg: WorkspacePackageJson;
    try {
      libPkg = readPackageJson(pkgPath);
    } catch (e) {
      // Collected like every other member problem, so one unparseable library is a named entry in
      // the preflight report rather than a stack trace out of the middle of a scan.
      problems.push(`${rel}: ${(e as Error).message}`);
      continue;
    }
    if (typeof libPkg.name !== "string" || libPkg.name === "") continue;
    libs.push({
      name: libPkg.name,
      version: typeof libPkg.version === "string" ? libPkg.version : "0.0.0",
      dir,
      relDir: rel,
      pkg: libPkg,
    });
  }

  return { workspace: { root: absRoot, pkg, plugins, libs }, problems };
}

/** The workspace containing `from`, already loaded — or null when there is none. */
export function loadWorkspaceFrom(from: string): Workspace | null {
  const root = findWorkspaceRoot(from);
  return root === null ? null : loadWorkspace(root);
}

/** The member whose directory is `dir` (absolute), or null. Used to decide single-plugin mode. */
export function memberForDir(ws: Workspace, dir: string): WorkspaceMember | null {
  const abs = toPosix(resolve(dir));
  for (const m of [...ws.plugins, ...ws.libs]) if (toPosix(m.dir) === abs) return m;
  return null;
}
