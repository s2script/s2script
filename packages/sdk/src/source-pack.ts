/**
 * Pack a package's source the way `npm pack` does: the same file selection (npm-packlist's rules)
 * and the same tarball shape (gzip+ustar, every entry under `package/`, fixed mtime). `s2s deploy`
 * uploads it as `source.tgz`; `s2s pack` writes or lists it.
 *
 * File selection, in npm's order of precedence:
 *   1. Never packed, whatever else says: NEVER_PACKED below (VCS dirs, node_modules, .npmrc,
 *      lockfiles, editor litter) — plus `.env` files, which npm would pack but we refuse to upload.
 *   2. Always packed: package.json, README, COPYING, LICENSE/LICENCE (bare or with an extension),
 *      and the files `main` and `bin` name.
 *   3. With a `files` array, only what it lists (and directories' contents); without one,
 *      everything.
 *   4. Ignore files remove from that set: `.npmignore`, or `.gitignore` where a directory has no
 *      `.npmignore`, in every directory. With `files`, the package root's own ignore file is not
 *      read (npm: `files` replaces it).
 *   5. A package inside an npm workspace without `files` also honours the ignore files of each
 *      directory above it up to the workspace root. Like npm 10, their patterns are matched
 *      against paths relative to the PACKAGE, not to the directory holding the ignore file — so a
 *      root `.gitignore`'s `dist/` excludes the package's dist, while `plugins/greeter/dist/` does not.
 *
 * Symlinks are skipped, as npm skips them.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import ignore, { type Ignore } from "ignore";
import { minimatch } from "minimatch";
import { NPM_PACK_MTIME, packTar } from "./tar.ts";

/** The registry's cap on `source.tgz`, compressed. Refusing locally beats a build-then-413. */
export const MAX_SOURCE_BYTES = 16 * 1024 * 1024;

/**
 * npm-packlist's default ignores (any depth unless anchored with `/`): VCS and editor litter, the
 * ignore files themselves, the root node_modules (a nested one IS packed), and root lockfiles.
 */
const NEVER_PACKED = [
  ".gitignore",
  ".npmignore",
  ".git",
  "CVS",
  ".svn",
  ".hg",
  ".lock-wscript",
  ".wafpickle-*",
  ".*.swp",
  ".DS_Store",
  "._*",
  "npm-debug.log",
  ".npmrc",
  "/node_modules",
  "config.gypi",
  "*.orig",
  "/package-lock.json",
  "/yarn.lock",
  "/pnpm-lock.yaml",
  "/bun.lockb",
  "/bun.lock",
  // Not npm's: a source upload never carries local secrets. Templates stay.
  ".env",
  ".env.*",
  "!.env.example",
  "!.env.sample",
];

/** npm-packlist's `/readme{,.*[^~$]}` and friends: the bare name, or the name plus an extension. */
const ALWAYS_PACKED = /^(readme|copying|license|licence)(\..*[^~$])?$/i;

export interface PackagePkgJson {
  name?: string;
  version?: string;
  main?: unknown;
  bin?: unknown;
  files?: unknown;
  [key: string]: unknown;
}

interface IgnoreRules {
  /** Absolute directory the patterns are relative to. */
  base: string;
  rules: Ignore;
}

function toPosix(p: string): string {
  return sep === "/" ? p : p.split(sep).join("/");
}

/** `foo`, `./foo`, `/foo/` → `foo`. */
function normalizeEntry(p: string): string {
  return p.replace(/\\/g, "/").replace(/^\.\//, "").replace(/^\/+/, "").replace(/\/+$/, "");
}

function readIgnoreFile(dir: string): IgnoreRules | null {
  for (const name of [".npmignore", ".gitignore"]) {
    let text: string;
    try {
      text = readFileSync(join(dir, name), "utf8");
    } catch {
      continue;
    }
    return { base: dir, rules: ignore().add(text) };
  }
  return null;
}

/** Shallowest first: the last ruleset with an opinion wins, as with nested .gitignore files. */
function isIgnored(abs: string, isDir: boolean, rulesets: IgnoreRules[]): boolean {
  let ignored = false;
  for (const { base, rules } of rulesets) {
    const rel = toPosix(relative(base, abs));
    if (!rel || rel.startsWith("..")) continue;
    const r = rules.test(isDir ? `${rel}/` : rel);
    if (r.ignored) ignored = true;
    else if (r.unignored) ignored = false;
  }
  return ignored;
}

/** The `files` array as include/exclude glob lists. Null when package.json has none. */
function filesPatterns(files: unknown): { include: string[]; exclude: string[] } | null {
  if (!Array.isArray(files)) return null;
  const include: string[] = [];
  const exclude: string[] = [];
  for (const raw of files) {
    if (typeof raw !== "string" || !raw.trim()) continue;
    const negated = raw.startsWith("!");
    const entry = normalizeEntry(negated ? raw.slice(1) : raw);
    if (!entry) continue;
    (negated ? exclude : include).push(entry);
  }
  return { include, exclude };
}

/** True when `rel` (a file) or any of its parent directories matches `pattern`. */
function matchesWithParents(rel: string, pattern: string): boolean {
  const parts = rel.split("/");
  for (let i = parts.length; i > 0; i--) {
    if (minimatch(parts.slice(0, i).join("/"), pattern, { dot: true })) return true;
  }
  return false;
}

function selectedByFiles(rel: string, files: { include: string[]; exclude: string[] }): boolean {
  if (!files.include.some((p) => matchesWithParents(rel, p))) return false;
  return !files.exclude.some((p) => matchesWithParents(rel, p));
}

/** The files `main` and `bin` name, which npm packs even when nothing else would. */
function entryPoints(pkg: PackagePkgJson): string[] {
  const out: string[] = [];
  if (typeof pkg.main === "string") out.push(normalizeEntry(pkg.main));
  if (typeof pkg.bin === "string") out.push(normalizeEntry(pkg.bin));
  else if (pkg.bin && typeof pkg.bin === "object") {
    for (const v of Object.values(pkg.bin as Record<string, unknown>)) {
      if (typeof v === "string") out.push(normalizeEntry(v));
    }
  }
  return out;
}

/** The nearest directory above `absDir` whose package.json declares npm `workspaces`. */
function npmWorkspaceRoot(absDir: string): string | null {
  for (let d = dirname(absDir); ; d = dirname(d)) {
    try {
      const pj = JSON.parse(readFileSync(join(d, "package.json"), "utf8")) as { workspaces?: unknown };
      if (pj && pj.workspaces) return d;
    } catch {
      // no (readable) package.json here — keep walking up
    }
    if (dirname(d) === d) return null;
  }
}

/**
 * Ignore files above the package, from the workspace root down to the package's parent — each
 * rebased onto the package directory, which is how npm 10 evaluates them.
 */
function workspaceRulesets(absDir: string): IgnoreRules[] {
  const root = npmWorkspaceRoot(absDir);
  if (root === null) return [];
  const dirs: string[] = [];
  for (let d = dirname(absDir); ; d = dirname(d)) {
    dirs.unshift(d);
    if (d === root || dirname(d) === d) break;
  }
  return dirs
    .map(readIgnoreFile)
    .filter((r): r is IgnoreRules => r !== null)
    .map((r) => ({ base: absDir, rules: r.rules }));
}

/**
 * The files `npm pack` would put in this package's tarball, as `/`-separated paths relative to
 * the package directory, sorted.
 */
export function listPackageFiles(dir: string, pkg?: PackagePkgJson): string[] {
  const absDir = resolve(dir);
  const pj = pkg ?? (JSON.parse(readFileSync(join(absDir, "package.json"), "utf8")) as PackagePkgJson);
  const files = filesPatterns(pj.files);
  const never = ignore().add(NEVER_PACKED);
  const forced = new Set(entryPoints(pj));
  const out: string[] = [];

  const walk = (abs: string, rulesets: IgnoreRules[]): void => {
    const isRoot = abs === absDir;
    const own = isRoot && files ? null : readIgnoreFile(abs);
    const here = own ? [...rulesets, own] : rulesets;
    const entries = readdirSync(abs, { withFileTypes: true }).sort((a: { name: string }, b: { name: string }) =>
      a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
    );
    for (const entry of entries) {
      const childAbs = join(abs, entry.name);
      const rel = toPosix(relative(absDir, childAbs));
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) {
        if (never.ignores(`${rel}/`)) continue;
        if (isIgnored(childAbs, true, here)) continue;
        walk(childAbs, here);
        continue;
      }
      if (!entry.isFile()) continue;
      if (never.ignores(rel)) continue;
      const alwaysPacked =
        (isRoot && (entry.name === "package.json" || ALWAYS_PACKED.test(entry.name))) ||
        forced.has(rel);
      if (!alwaysPacked) {
        if (files && !selectedByFiles(rel, files)) continue;
        if (isIgnored(childAbs, false, here)) continue;
      }
      out.push(rel);
    }
  };

  walk(absDir, files ? [] : workspaceRulesets(absDir));
  return out.sort();
}

export interface SourcePack {
  tarball: Buffer;
  files: { path: string; size: number }[];
  /** Sum of the packed files' sizes, before compression. */
  unpackedSize: number;
}

/** The package's source tarball, as `npm pack` would build it. */
export function packSourceTarball(dir: string, pkg?: PackagePkgJson): SourcePack {
  const absDir = resolve(dir);
  const files = listPackageFiles(absDir, pkg).map((path) => {
    const abs = join(absDir, path);
    const content = readFileSync(abs);
    // npm normalizes modes: executables 0755, everything else 0644.
    const mode = statSync(abs).mode & 0o111 ? 0o755 : 0o644;
    return { path, content, mode };
  });
  const tarball = packTar(
    files.map((f) => ({ path: `package/${f.path}`, content: f.content, mode: f.mode })),
    { mtime: NPM_PACK_MTIME },
  );
  return {
    tarball,
    files: files.map((f) => ({ path: f.path, size: f.content.length })),
    unpackedSize: files.reduce((n, f) => n + f.content.length, 0),
  };
}

/** Refuse a source tarball the registry would 413, naming the cap. */
export function assertSourceSize(pack: SourcePack, label: string): void {
  if (pack.tarball.length > MAX_SOURCE_BYTES) {
    const mib = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MiB`;
    throw new Error(
      `${label}: the source tarball is ${mib(pack.tarball.length)}; the registry accepts at most ` +
        `${mib(MAX_SOURCE_BYTES)}. Run \`s2s pack --dry-run\` to see what is packed, then narrow ` +
        `package.json "files" or add a .npmignore.`,
    );
  }
}

/** npm's tarball filename: `@scope/name` → `scope-name-<version>.tgz`. */
export function tarballFilename(name: string, version: string): string {
  return `${name.replace(/^@/, "").replace(/\//g, "-")}-${version}.tgz`;
}
