/**
 * Ensure project .npmrc maps a package scope to the types-only npm-compat registry.
 * Does not touch @s2script (stays on public npm). Unscoped packages skip .npmrc
 * (default registry must remain public npm).
 */

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export function npmCompatRegistryUrl(registryUrl: string): string {
  return `${registryUrl.replace(/\/$/, "")}/npm/`;
}

export function scopeFromPackageName(name: string): string | null {
  if (!name.startsWith("@")) return null;
  const slash = name.indexOf("/");
  if (slash < 2) return null;
  return name.slice(1, slash);
}

/**
 * Upsert `@scope:registry=<npmCompatUrl>` into `<dir>/.npmrc`.
 * Returns the line written, or null if skipped (unscoped / @s2script).
 */
export function ensureScopeNpmrc(
  projectDir: string,
  packageName: string,
  registryUrl: string
): string | null {
  const scope = scopeFromPackageName(packageName);
  if (!scope || scope === "s2script") return null;

  const compat = npmCompatRegistryUrl(registryUrl);
  const key = `@${scope}:registry`;
  const line = `${key}=${compat}`;
  const npmrcPath = join(projectDir, ".npmrc");
  let existing = existsSync(npmrcPath) ? readFileSync(npmrcPath, "utf8") : "";
  const lines = existing.split(/\r?\n/);
  const idx = lines.findIndex((l) => l.trimStart().startsWith(`${key}=`) || l.trimStart().startsWith(`${key} =`));
  if (idx >= 0) {
    lines[idx] = line;
  } else {
    if (existing && !existing.endsWith("\n")) lines.push("");
    lines.push(line);
  }
  const out = lines.filter((l, i, a) => !(l === "" && a[i - 1] === "")).join("\n");
  writeFileSync(npmrcPath, out.endsWith("\n") ? out : out + "\n");
  return line;
}
