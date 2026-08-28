#!/usr/bin/env bash
# Fails when .changeset/config.json's `ignore` list disagrees with the on-disk plugin set, in
# EITHER direction.
#
# WHY THIS GATE EXISTS. Base plugins track the tag, not changesets (§9.2 of the workspace design)
# — so every plugin the workspace covers (`s2script.workspace.plugins`: `plugins/*` and
# `plugins/disabled/*`) is hand-listed in `ignore`, 18 names today. `@changesets/config` validates
# every `ignore` entry against the packages actually present in the project
# (`getUnmatchedPatterns`, node_modules/@changesets/config/dist/changesets-config.esm.js) and
# THROWS a ValidationError if one doesn't match anything — so deleting or renaming a plugin without
# editing this file makes every changesets invocation (including `changesets.yml` on a push to
# main, which auto-publishes @s2script/sdk and @s2script/cs2) blow up with "The package or glob
# expression @s2script/<removed> is specified in the ignore option but it is not found in the
# project". Nothing else catches this: check-changeset.sh only diffs packages/* for a MISSING
# changeset, and adding a plugin without listing it here doesn't throw at all — it just lets
# changesets silently start versioning a plugin that is supposed to track the release tag instead.
#
# A glob ("@s2script/*") is not a safe substitute: it would also swallow @s2script/sdk and
# @s2script/cs2, which must keep being released via changesets. So the list stays an explicit
# enumeration, and this gate is what keeps it honest as plugins are added, removed, or renamed.
#
# Reuses the SAME workspace scanner (`scanWorkspace`) the build/publish/version commands use to
# enumerate plugins, rather than re-deriving "plugins/*, plugins/disabled/*" as a second hardcoded
# glob pair here — one definition of "the plugin set", shared with packages/sdk/src/workspace/.
set -euo pipefail
cd "$(dirname "$0")/.."

node --experimental-strip-types --no-warnings --input-type=module <<'NODE'
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const { scanWorkspace } = await import("./packages/sdk/src/workspace/workspace.ts");

const CONFIG_PATH = ".changeset/config.json";

// Lenient scan, not loadWorkspace: an unrelated §4.3 problem (a plugin missing an entry point,
// say) is check-plugins-typecheck.sh's job to report. This gate cares about one thing only — the
// SET of plugin names — and must not fail for a reason that points the author at the wrong file.
const { workspace, problems } = scanWorkspace(resolve("."));
const onDisk = new Set(workspace.plugins.map((p) => p.name));

const config = JSON.parse(readFileSync(CONFIG_PATH, "utf8"));
const ignored = new Set(Array.isArray(config.ignore) ? config.ignore : []);

const missing = [...onDisk].filter((n) => !ignored.has(n)).sort(); // on disk, not ignored

// The REMOVE half is only trustworthy when the scan read EVERY member. A member dropped by a §4.3
// problem is absent from `onDisk` but still very much present to changesets, which reads the npm
// `workspaces` set and neither knows nor cares about entry points — so "REMOVE" would be exactly
// backwards, and following it makes changesets start versioning a plugin that is supposed to track
// the release tag. That is the failure this gate exists to prevent, so it must not cause it.
// The ADD half stays valid: a name the scan DID read is unambiguously on disk.
const stale = problems.length > 0 ? [] : [...ignored].filter((n) => !onDisk.has(n)).sort();
if (problems.length > 0) {
  console.log(
    `check-changeset-ignore: ${problems.length} workspace member(s) unreadable — skipping the ` +
      `REMOVE check (a dropped member is still a package to changesets). Fix these first:\n  ` +
      problems.join("\n  "),
  );
}

if (missing.length === 0 && stale.length === 0) {
  console.log(`check-changeset-ignore: ok (${onDisk.size} plugin(s) match ${CONFIG_PATH}'s ignore list)`);
  process.exit(0);
}

console.error(`check-changeset-ignore: FAIL — ${CONFIG_PATH}'s \`ignore\` list is stale.`);
console.error("");
if (missing.length > 0) {
  console.error("  ADD — on-disk plugin(s) not in \`ignore\` (changesets would try to version them):");
  for (const n of missing) console.error(`    "${n}",`);
  console.error("");
}
if (stale.length > 0) {
  console.error("  REMOVE — in \`ignore\` but no such plugin package exists on disk (this is what");
  console.error("  makes @changesets/config throw a ValidationError on every changesets invocation):");
  for (const n of stale) console.error(`    "${n}",`);
  console.error("");
}
console.error(`  Edit ${CONFIG_PATH}'s "ignore" array to match.`);
process.exit(1);
NODE
