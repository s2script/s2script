/**
 * buildPlugin: reads a plugin directory, bundles the TypeScript entry with
 * esbuild (CJS, @s2script/* external), derives manifest.json, and zips both
 * into <dir>/dist/<sanitized-id>.s2sp.
 *
 * The .s2sp format is consumed by core's read_s2sp (loader.rs):
 *   - zip must contain exactly "manifest.json" + "plugin.js"
 *   - manifest.json must have keys: id, version, apiVersion (serde rename on
 *     api_version field in loader::Manifest), pluginDependencies, publishes
 */

import * as esbuild from "esbuild";
import AdmZip from "adm-zip";
import { readFileSync, mkdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { typecheckPlugin, formatDiagnostics } from "./typecheck/typecheck.ts";

/** Shape of plugin package.json (the fields we care about). */
interface PluginPackageJson {
  name: string;
  version: string;
  main?: string;
  s2script?: {
    apiVersion?: string;
    main?: string;
    pluginDependencies?: Record<string, string>;
    optionalPluginDependencies?: Record<string, string>;
    publishes?: Record<string, unknown>;
  };
}

/**
 * Bundle the plugin at `dir`, produce a .s2sp archive, return the output path.
 * @param dir         Path to the plugin directory (absolute or relative to cwd).
 * @param packagesDir Path to the monorepo packages/ directory (for @s2script/* .d.ts resolution).
 */
export async function buildPlugin(dir: string, packagesDir: string): Promise<string> {
  const absDir = resolve(dir);

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on error. ---
  const tc = typecheckPlugin(absDir, { packagesDir });
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }

  // --- Read package.json ---   (existing code continues unchanged) ---
  const pkgPath = join(absDir, "package.json");
  const pkg: PluginPackageJson = JSON.parse(readFileSync(pkgPath, "utf8"));

  const { name, version } = pkg;
  const s2 = pkg.s2script ?? {};
  const apiVersion = s2.apiVersion ?? "";
  const pluginDependencies = s2.pluginDependencies ?? {};
  const optionalPluginDependencies = s2.optionalPluginDependencies ?? {};
  const publishes = s2.publishes;

  // Every builtin package + every inter-plugin dependency name is esbuild-external (resolved at
  // runtime by core, never bundled).
  const external = Array.from(new Set([
    "@s2script/*",
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]));

  // Entry point: s2script.main takes precedence, then package.main.
  const entryRelative = s2.main ?? pkg.main;
  if (!entryRelative) {
    throw new Error(
      `buildPlugin: no entry point found in ${pkgPath} (set s2script.main or main)`
    );
  }
  const entryPoint = join(absDir, entryRelative);

  // --- Bundle with esbuild ---
  const result = await esbuild.build({
    entryPoints: [entryPoint],
    bundle: true,
    platform: "neutral",
    format: "cjs",
    external,
    target: "es2020",
    write: false,
  });

  const pluginJs = result.outputFiles[0].text;

  // --- Derive manifest (keys must match loader::Manifest serde fields) ---
  // loader.rs: id (String), version (String), api_version (#[serde(rename="apiVersion")] String)
  const manifest: Record<string, unknown> = {
    id: name,
    version,
    apiVersion,           // <-- MUST be "apiVersion" to match #[serde(rename = "apiVersion")]
    pluginDependencies,
    optionalPluginDependencies,
  };
  if (publishes !== undefined) {
    manifest.publishes = publishes;
  }

  // --- Zip manifest.json + plugin.js ---
  const zip = new AdmZip();
  zip.addFile("manifest.json", Buffer.from(JSON.stringify(manifest, null, 2)));
  zip.addFile("plugin.js", Buffer.from(pluginJs));

  // --- Write to dir/dist/<sanitized-id>.s2sp ---
  const sanitizedId = name.replace(/[^a-zA-Z0-9._-]/g, "_");
  const outDir = join(absDir, "dist");
  mkdirSync(outDir, { recursive: true });
  const outPath = join(outDir, `${sanitizedId}.s2sp`);
  zip.writeZip(outPath);

  return outPath;
}
