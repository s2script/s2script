#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";
import semver from "semver";

function fail(path, detail, cause) {
  return new Error(`plugin version check: ${path}: ${detail}`, cause ? { cause } : undefined);
}

export function readPluginVersion(path) {
  let entries;
  try {
    entries = unzipSync(readFileSync(path));
  } catch (error) {
    throw fail(path, "cannot read archive", error);
  }

  const entry = entries["manifest.json"];
  if (!entry) throw fail(path, "has no manifest.json");

  let manifest;
  try {
    manifest = JSON.parse(Buffer.from(entry).toString("utf8"));
  } catch (error) {
    throw fail(path, "manifest.json is not valid JSON", error);
  }

  const version = manifest?.version;
  if (typeof version !== "string" || semver.valid(version) !== version.split("+")[0]) {
    throw fail(path, `has invalid manifest version ${JSON.stringify(version)}`);
  }
  const id = typeof manifest.id === "string" && manifest.id ? manifest.id : basename(path, ".s2sp");
  return { path, id, version };
}

export function checkPluginVersions(expectedVersion, paths) {
  if (typeof expectedVersion !== "string" || semver.valid(expectedVersion) !== expectedVersion.split("+")[0]) {
    throw new Error(`plugin version check: invalid expected version ${JSON.stringify(expectedVersion)}`);
  }
  if (paths.length === 0) {
    throw new Error("plugin version check: provide at least one .s2sp archive");
  }

  const plugins = paths.map(readPluginVersion);
  for (const plugin of plugins) {
    if (plugin.version !== expectedVersion) {
      throw fail(plugin.path, `manifest version ${plugin.version}; expected ${expectedVersion}`);
    }
  }
  return plugins;
}

function main(argv) {
  if (argv.length < 2) {
    console.error("Usage: node scripts/check-plugin-versions.mjs <expected-version> <plugin.s2sp...>");
    return 2;
  }
  try {
    const plugins = checkPluginVersions(argv[0], argv.slice(1));
    console.log(`Plugin versions match ${argv[0]} (${plugins.length} archives)`);
    return 0;
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 1;
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  process.exitCode = main(process.argv.slice(2));
}
