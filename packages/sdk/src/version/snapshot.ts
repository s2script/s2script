/** Temporary CI versions. Stable changesets and interface versions remain authored source data. */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { valid } from "semver";
import { packageKind } from "../libraries.ts";
import { indexInterfaces } from "../workspace/interfaces.ts";
import { preflightWorkspace } from "../workspace/preflight.ts";
import { readBundles } from "../workspace/bundle.ts";
import { loadWorkspace } from "../workspace/workspace.ts";
import type { Workspace, WorkspaceMember } from "../workspace/workspace.ts";
import { readJsonFile } from "./ranges.ts";

export interface SnapshotOptions {
  id: string;
  sinceRef?: string;
  statePath?: string;
  outputPath?: string;
}

export interface SnapshotPackage {
  name: string;
  version: string;
  /** Directory relative to the workspace root; "." for a root bundle. */
  path: string;
  kind: "plugin" | "library" | "bundle";
  private: boolean;
  access?: "public" | "restricted";
}

/** Persist this report only after its registry publication succeeds. */
export interface SnapshotReport {
  schemaVersion: 1;
  source: string;
  id: string;
  /** Only packages assigned the current snapshot, including private shared code. */
  packages: SnapshotPackage[];
  /** Cumulative effective versions for every current package and bundle. */
  versions: Record<string, string>;
}

function snapshotVersion(id: string): string {
  const version = `0.0.0-${id}`;
  if (typeof id !== "string" || !/^[A-Za-z][A-Za-z0-9.-]{0,127}$/.test(id) || valid(version) !== version) {
    throw new Error(`--snapshot requires a safe semver prerelease identifier such as dev.12.gabc123 (got ${JSON.stringify(id)})`);
  }
  return version;
}

function git(root: string, args: string[]): string {
  try {
    return execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch (e) {
    const error = e as Error & { stderr?: string };
    throw new Error(`s2s version --snapshot: git ${args[0]} failed: ${error.stderr?.trim() || error.message}`);
  }
}

function commit(root: string, ref: string): string {
  return git(root, ["rev-parse", "--verify", "--end-of-options", `${ref}^{commit}`]).trim();
}

function readState(path: string): SnapshotReport {
  try {
    const state = JSON.parse(readFileSync(path, "utf8")) as SnapshotReport;
    if (state.schemaVersion !== 1 || !/^[0-9a-f]{40,64}$/.test(state.source) || !Array.isArray(state.packages) ||
        !state.versions || typeof state.versions !== "object" || Array.isArray(state.versions)) {
      throw new Error("expected schemaVersion 1, source, id, packages, and versions");
    }
    snapshotVersion(state.id);
    for (const [name, version] of Object.entries(state.versions)) {
      if (!name || typeof version !== "string" || valid(version) !== version) throw new Error(`invalid version for ${name}`);
    }
    for (const pkg of state.packages) {
      if (!pkg || typeof pkg.name !== "string" || state.versions[pkg.name] !== pkg.version ||
          typeof pkg.path !== "string" || !["plugin", "library", "bundle"].includes(pkg.kind) || typeof pkg.private !== "boolean") {
        throw new Error("invalid packages entry");
      }
    }
    return state;
  } catch (e) {
    throw new Error(`cannot read snapshot state ${path}: ${(e as Error).message}`);
  }
}

function under(path: string, dir: string): boolean {
  return dir === "" || path === dir || path.startsWith(`${dir}/`);
}

/** Documentation, stable release intent, and tests cannot change shipped runtime bytes. */
function incidental(path: string): boolean {
  return /(^|\/)(docs?|\.changeset|tests?|__tests__|__snapshots__)(\/|$)/i.test(path) ||
    /\.(md|mdx|rst)$/i.test(path) || /\.(test|spec)\.[^/]+$/i.test(path) ||
    /(^|\/)(jest|vitest|playwright)\.config\.[^/]+$/.test(path) ||
    /(^|\/)scripts\/(test|check-tests)[^/]*$/.test(path) ||
    /(^|\/)(LICENSE[^/]*|CHANGELOG[^/]*)$/i.test(path);
}

/** Every workspace edge whose source bytes or contract can be embedded in a consumer build. */
function reverseDependencies(ws: Workspace, members: WorkspaceMember[]): Map<string, Set<string>> {
  const { producers } = indexInterfaces(ws.plugins);
  const known = new Set(members.map((m) => m.name));
  const reverse = new Map<string, Set<string>>();
  const edge = (producer: string, consumer: string): void => {
    if (!known.has(producer) || producer === consumer) return;
    const consumers = reverse.get(producer) ?? new Set<string>();
    consumers.add(consumer);
    reverse.set(producer, consumers);
  };
  for (const m of members) {
    for (const field of ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"]) {
      for (const name of Object.keys((m.pkg[field] ?? {}) as Record<string, string>)) edge(name, m.name);
    }
    for (const name of Object.keys((m.pkg.s2script?.libraries ?? {}) as Record<string, string>)) edge(name, m.name);
    for (const field of ["pluginDependencies", "optionalPluginDependencies"] as const) {
      for (const iface of Object.keys(m.pkg.s2script?.[field] ?? {})) {
        const producer = producers.get(iface);
        if (producer) edge(producer.plugin.name, m.name);
      }
    }
  }
  return reverse;
}

/** A root bundle also owns workspace tooling; only its descriptor metadata is bundle-only. */
function rootDescriptorOnly(ws: Workspace, since: string): boolean {
  const tooling = (ref: string): Record<string, unknown> => {
    const pkg = JSON.parse(git(ws.root, ["show", `${ref}:./package.json`])) as Record<string, unknown>;
    for (const field of ["name", "version", "description", "keywords", "author", "contributors", "license", "homepage", "bugs", "repository", "funding", "publishConfig", "files"]) delete pkg[field];
    const s2 = pkg.s2script as Record<string, unknown> | undefined;
    if (s2) { delete s2.bundle; delete s2.kind; }
    return pkg;
  };
  return isDeepStrictEqual(tooling(since), tooling("HEAD"));
}

function affected(ws: Workspace, members: WorkspaceMember[], since: string | undefined): Set<string> {
  const bundles = readBundles(ws);
  if (since === undefined) return new Set([...members, ...bundles].map((m) => m.name));
  // --no-renames retains both old and new paths; -z handles whitespace and unusual filenames.
  const paths = git(ws.root, ["diff", "--relative", "--name-only", "--no-renames", "-z", since, "HEAD", "--", "."]).split("\0").filter(Boolean);
  const selected = new Set<string>();
  const byDepth = [...members].sort((a, b) => b.relDir.length - a.relDir.length);
  for (const path of paths) {
    if (path.endsWith("/package.json") && !existsSync(join(ws.root, path))) {
      throw new Error(`s2s version --snapshot: removed package manifest ${path}; reconcile removed or renamed packages and bundle membership before snapshotting`);
    }
    const member = byDepth.find((m) => under(path, m.relDir));
    // Classify paths relative to their package: a runtime package may itself be named "test".
    if (incidental(member ? path.slice(member.relDir.length + 1) : path)) continue;
    if (member) {
      selected.add(member.name);
      continue;
    }
    const descriptor = bundles.find((b) => path === (b.relDir ? `${b.relDir}/package.json` : "package.json"));
    if (descriptor && (descriptor.relDir !== "" || rootDescriptorOnly(ws, since))) {
      selected.add(descriptor.name);
      continue;
    }
    // Product shared files live outside nested plugin directories. The deepest common containing
    // directory owns them. Files shared at the workspace root (tooling, lockfiles, runtime) affect all.
    const ancestors = members.flatMap((m) => {
      const out: string[] = [];
      let dir = dirname(m.relDir);
      while (dir !== ".") { out.push(dir); dir = dirname(dir); }
      return out;
    }).filter((dir) => under(path, dir)).sort((a, b) => b.length - a.length);
    const owner = ancestors[0];
    for (const m of members) if (owner === undefined || under(m.relDir, owner)) selected.add(m.name);
    if (owner === undefined) for (const b of bundles) selected.add(b.name);
  }
  const reverse = reverseDependencies(ws, members);
  const queue = [...selected];
  for (let i = 0; i < queue.length; i++) {
    for (const consumer of reverse.get(queue[i]) ?? []) {
      if (!selected.has(consumer)) { selected.add(consumer); queue.push(consumer); }
    }
  }
  for (const bundle of bundles) if (bundle.members.some((m) => selected.has(m.name))) selected.add(bundle.name);
  return selected;
}

/**
 * Overlay effective snapshot versions onto an ephemeral checkout. All validation/planning happens
 * before mutation; a write failure restores manifests, and the report is written last atomically.
 * Does not load changesets, write changelogs, or rewrite runtime interface dependency ranges.
 */
export function snapshotWorkspace(root: string, opts: SnapshotOptions): SnapshotReport {
  const version = snapshotVersion(opts.id);
  const ws = loadWorkspace(root);
  preflightWorkspace(ws);
  const members = [...ws.plugins, ...ws.libs].sort((a, b) => a.relDir.localeCompare(b.relDir));
  const bundles = readBundles(ws);
  const source = commit(ws.root, "HEAD");
  const since = opts.sinceRef === undefined ? undefined : commit(ws.root, opts.sinceRef);
  const state = opts.statePath === undefined ? undefined : readState(resolve(opts.statePath));
  if (since !== undefined) {
    git(ws.root, ["merge-base", "--is-ancestor", since, source]);
    if (state !== undefined && state.source !== since) throw new Error("snapshot state source must match --since");
  }
  if (state?.id === opts.id && state.source !== source) throw new Error("--snapshot id was already used for a different source in snapshot state");
  const selected = affected(ws, members, since);
  const all = [
    ...members.map((m) => ({ ...m, kind: (ws.libs.includes(m) ? "library" : packageKind(m.pkg)) as SnapshotPackage["kind"] })),
    ...bundles.map((b) => ({ ...b, kind: "bundle" as const })),
  ].sort((a, b) => a.relDir.localeCompare(b.relDir));
  const names = new Set<string>();
  for (const m of all) {
    if (names.has(m.name)) throw new Error(`snapshot: duplicate package or bundle name ${m.name}`);
    names.add(m.name);
    if (valid(m.version) !== m.version) throw new Error(`snapshot: ${m.name} has invalid version ${JSON.stringify(m.version)}`);
  }
  const versions: Record<string, string> = Object.create(null);
  for (const m of all) {
    const hasPrevious = state !== undefined && Object.prototype.hasOwnProperty.call(state.versions, m.name);
    if (state !== undefined && !selected.has(m.name) && !hasPrevious) {
      throw new Error(`snapshot state is missing effective version for unchanged package ${m.name}`);
    }
    const previous = hasPrevious ? state!.versions[m.name] : m.version;
    versions[m.name] = selected.has(m.name) ? version : previous;
  }
  const packages: SnapshotPackage[] = all.filter((m) => selected.has(m.name)).map((m) => {
    const access = (m.pkg.publishConfig as { access?: string } | undefined)?.access;
    if (access !== undefined && access !== "public" && access !== "restricted") throw new Error(`snapshot: ${m.name} has invalid publishConfig.access`);
    return { name: m.name, version, path: m.relDir || ".", kind: m.kind, private: m.pkg.private === true, ...(access === undefined ? {} : { access }) };
  });
  const report: SnapshotReport = { schemaVersion: 1, source, id: opts.id, packages, versions };
  const writes = new Map<string, string>();
  for (const m of all) {
    const path = join(m.dir, "package.json");
    const { json, indent, eol } = readJsonFile(path);
    const next = structuredClone(m.pkg);
    // Freeze the AUTHORED interface version before the package's distribution version changes.
    if (typeof next.s2script?.publishes === "string" && next.s2script.publishes.trim() === "self") {
      next.s2script.publishes = { [m.name]: m.version };
    }
    next.version = versions[m.name];
    for (const field of ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"]) {
      const dependencies = next[field] as Record<string, string> | undefined;
      for (const name of Object.keys(dependencies ?? {})) if (names.has(name)) dependencies![name] = versions[name];
    }
    const libraries = next.s2script?.libraries as Record<string, string> | undefined;
    for (const name of Object.keys(libraries ?? {})) if (names.has(name)) libraries![name] = versions[name];
    if (JSON.stringify(next) !== JSON.stringify(json)) writes.set(path, JSON.stringify(next, null, indent) + eol);
  }
  const output = opts.outputPath === undefined ? undefined : resolve(opts.outputPath);
  if (output !== undefined) {
    const rel = relative(ws.root, output).split("\\").join("/");
    const tracked = !rel.startsWith("../") && git(ws.root, ["--literal-pathspecs", "ls-files", "-z", "--", rel]) !== "";
    if (all.some((m) => join(m.dir, "package.json") === output) || rel === "package.json" ||
        tracked || /(^|\/)(\.git|\.changeset|node_modules)(\/|$)/.test(rel)) {
      throw new Error(`--output must not overwrite tracked source, a package manifest, or workspace metadata: ${output}`);
    }
  }
  const originals = new Map<string, string>();
  let temporary: string | undefined;
  let temporaryCreated = false;
  try {
    for (const [path, contents] of writes) {
      originals.set(path, readFileSync(path, "utf8"));
      writeFileSync(path, contents);
    }
    if (output !== undefined) {
      mkdirSync(dirname(output), { recursive: true });
      temporary = `${output}.${process.pid}.tmp`;
      writeFileSync(temporary, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
      temporaryCreated = true;
      renameSync(temporary, output);
    }
  } catch (e) {
    for (const [path, contents] of originals) writeFileSync(path, contents);
    if (temporaryCreated && temporary !== undefined) rmSync(temporary, { force: true });
    throw e;
  }
  return report;
}
