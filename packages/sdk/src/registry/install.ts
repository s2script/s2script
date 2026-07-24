/**
 * s2s install — resolve a manifest/args to a transitive plan (server-side),
 * download each .s2sp, verify its sha256, and write version-less filenames
 * into a plugins directory. No credentials required (registry reads are public).
 */

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync, renameSync, existsSync } from "node:fs";
import { basename, join, resolve, sep } from "node:path";
import type { InstallPlan, PlanEntry } from "./client.ts";

export interface InstallInput {
  plugins: Record<string, string>;
}

export function parseManifest(input: unknown): InstallInput {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    throw new Error("manifest must be a JSON object");
  }
  const plugins = (input as { plugins?: unknown }).plugins;
  if (!plugins || typeof plugins !== "object" || Array.isArray(plugins)) {
    throw new Error('manifest must have a "plugins" object (name -> version range)');
  }
  const out: Record<string, string> = {};
  for (const [name, range] of Object.entries(plugins as Record<string, unknown>)) {
    if (typeof range !== "string") throw new Error(`range for ${name} must be a string`);
    out[name] = range;
  }
  return { plugins: out };
}

/** Split "name@range" (scoped-safe: only the LAST @ separates). Bare name -> "*". */
export function parseSpec(spec: string): { name: string; range: string } {
  const at = spec.lastIndexOf("@");
  if (at > 0) return { name: spec.slice(0, at), range: spec.slice(at + 1) || "*" };
  return { name: spec, range: "*" };
}

export function mergeSpecs(manifest: InstallInput, args: string[]): Record<string, string> {
  const merged: Record<string, string> = { ...manifest.plugins };
  for (const a of args) {
    const { name, range } = parseSpec(a);
    merged[name] = range;
  }
  return merged;
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Only bare filenames matching this shape are trusted — mirrors the sanitization in
 *  config/config-path.ts's configFileName ([A-Za-z0-9._-]). No directory separators, no ".". */
const SAFE_FILENAME_RE = /^[A-Za-z0-9._-]+$/;

/** entry.filename comes straight from the (possibly compromised) registry response — never trust
 *  it as a path. Derive a plain basename and refuse anything that isn't already exactly that. */
function safeFilename(e: PlanEntry): string {
  const raw = e.filename;
  const base = basename(raw);
  if (
    !base ||
    base === "." ||
    base === ".." ||
    base !== raw ||
    base.includes("/") ||
    base.includes("\\") ||
    base.includes("..") ||
    !SAFE_FILENAME_RE.test(base)
  ) {
    throw new Error(`refusing ${e.name}@${e.version}: unsafe filename "${raw}"`);
  }
  return base;
}

export async function resolveMerged(
  client: { plan(name: string, range?: string): Promise<InstallPlan> },
  specs: Record<string, string>
): Promise<{ install: PlanEntry[]; warnings: string[]; errors: string[] }> {
  const chosen = new Map<string, PlanEntry>();
  const warnings: string[] = [];
  const errors: string[] = [];

  for (const [name, range] of Object.entries(specs)) {
    const plan = await client.plan(name, range);
    if (plan.errors.length) {
      errors.push(...plan.errors);
      continue;
    }
    warnings.push(...plan.warnings);
    for (const entry of plan.install) {
      const prior = chosen.get(entry.name);
      if (prior && prior.version !== entry.version) {
        errors.push(
          `version conflict for ${entry.name}: ${prior.version} vs ${entry.version} across requested plugins`
        );
        continue;
      }
      chosen.set(entry.name, entry);
    }
  }

  return {
    install: [...chosen.values()].sort((a, b) => a.name.localeCompare(b.name)),
    warnings: [...new Set(warnings)],
    errors: [...new Set(errors)],
  };
}

export async function installPlan(opts: {
  client: {
    plan(name: string, range?: string): Promise<InstallPlan>;
    downloadS2sp(name: string, version: string): Promise<Buffer>;
  };
  specs: Record<string, string>;
  dir: string;
  reviewedOnly?: boolean;
  dryRun?: boolean;
  log?: (m: string) => void;
}): Promise<{ written: string[]; skipped: string[]; warnings: string[] }> {
  const log = opts.log ?? (() => {});
  const { install, warnings, errors } = await resolveMerged(opts.client, opts.specs);

  if (errors.length) throw new Error(`cannot resolve plugins:\n  - ${errors.join("\n  - ")}`);

  if (opts.reviewedOnly) {
    const unreviewed = install.filter((e) => e.reviewState !== "reviewed");
    if (unreviewed.length) {
      throw new Error(
        `--reviewed-only: refusing unreviewed plugins: ${unreviewed.map((e) => `${e.name}@${e.version}`).join(", ")}`
      );
    }
  }

  // Pre-write validation pass: every entry must have a verifiable sha256 and a filename we can
  // safely write inside opts.dir. This runs — and can throw — before any download or write, same
  // as the resolution-error check above, so a single bad entry aborts the whole install cleanly.
  const dirResolved = resolve(opts.dir);
  const targets = install.map((e) => {
    if (!e.sha256) {
      throw new Error(`refusing ${e.name}@${e.version}: registry did not provide a sha256 to verify against`);
    }
    const filename = safeFilename(e);
    const dest = resolve(join(opts.dir, filename));
    if (dest !== dirResolved && !dest.startsWith(dirResolved + sep)) {
      throw new Error(`refusing ${e.name}@${e.version}: destination "${dest}" escapes plugins dir "${dirResolved}"`);
    }
    return { entry: e, filename, dest };
  });

  for (const w of warnings) log(`warning: ${w}`);

  if (opts.dryRun) {
    for (const e of install) log(`would install ${e.name}@${e.version} -> ${e.filename}`);
    return { written: [], skipped: [], warnings };
  }

  mkdirSync(opts.dir, { recursive: true });
  const written: string[] = [];
  const skipped: string[] = [];

  for (const { entry: e, filename, dest } of targets) {
    if (existsSync(dest)) {
      const have = sha256Hex(readFileSync(dest));
      if (have === e.sha256) {
        skipped.push(filename);
        log(`up to date ${filename}`);
        continue;
      }
    }
    const bytes = await opts.client.downloadS2sp(e.name, e.version);
    const got = sha256Hex(bytes);
    if (got !== e.sha256) {
      throw new Error(`sha256 mismatch for ${e.name}@${e.version}: expected ${e.sha256}, got ${got}`);
    }
    const tmp = `${dest}.${process.pid}.tmp`;
    writeFileSync(tmp, bytes);
    renameSync(tmp, dest);
    written.push(filename);
    log(`installed ${e.name}@${e.version} -> ${filename}`);
  }

  return { written, skipped, warnings };
}

/** Read + parse a manifest file, or return an empty input if the path is absent. */
export function loadManifestFile(path: string): InstallInput {
  if (!existsSync(path)) return { plugins: {} };
  return parseManifest(JSON.parse(readFileSync(path, "utf8")));
}
