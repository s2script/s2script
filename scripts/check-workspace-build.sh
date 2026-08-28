#!/usr/bin/env bash
# The workspace gate (design spec 2026-07-27 §12): build examples/monorepo/ — a real multi-plugin
# workspace — and prove the one thing no other gate can see.
#
# Every other gate would still be green if sibling contract resolution silently regressed to the
# `any` stub it replaced (§3.3): the consumer would still typecheck (against `any`), still bundle,
# still produce a .s2sp. The ONLY visible difference is `manifest.compiledAgainst` — so this gate
# asserts the full triangle the runtime depends on:
#
#   sha256(producer's api.d.ts on disk)
#     == producer manifest publishes[<iface>].typesSha256
#     == consumer manifest compiledAgainst[<iface>]
#
# That middle equality is what `core/src/loader.rs:192` checks at load. In a workspace both sides
# are a hash of the SAME file, so the check passes for structural reasons rather than by luck —
# and this gate is what keeps that structural claim true.
#
# Usage: scripts/check-workspace-build.sh [workspace-dir]     (default: examples/monorepo)
set -euo pipefail
cd "$(dirname "$0")/.."

WS="${1:-examples/monorepo}"
if [ ! -f "$WS/package.json" ]; then
    echo "ERROR: $WS/package.json missing — this gate needs the example workspace" >&2
    exit 1
fi

# Inside the repo, not $TMPDIR: the module imports bare specifiers (fflate, the SDK sources), and
# Node resolves those from the FILE's directory upward — a /tmp file finds no node_modules.
GATE_JS="./.s2-workspace-gate.$$.mjs"
trap 'rm -f "$GATE_JS"' EXIT
cat > "$GATE_JS" <<'NODE'
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";

const { unzipSync, strFromU8 } = await import("fflate");
const { loadWorkspace } = await import("./packages/sdk/src/workspace/workspace.ts");
const { buildWorkspace, formatBuildFailures } = await import("./packages/sdk/src/workspace/build-all.ts");
const { resolveSiblingContracts } = await import("./packages/sdk/src/workspace/siblings.ts");
const { localContractPath } = await import("./packages/sdk/src/contracts.ts");

const wsDir = resolve(process.env.S2_WS_DIR);
const fail = (msg) => { console.error(`FAIL: ${msg}`); process.exitCode = 1; };
const manifestOf = (s2sp) => JSON.parse(strFromU8(unzipSync(readFileSync(s2sp))["manifest.json"]));
const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

const ws = loadWorkspace(wsDir);

// "Several plugins, one repo" is the lesson the example exists to carry; one plugin would make
// every assertion below vacuous.
if (ws.plugins.length < 2) {
  fail(`${relative(process.cwd(), wsDir)} has ${ws.plugins.length} plugin(s) — a workspace example must hold at least 2`);
  process.exit(1);
}

const result = await buildWorkspace({
  workspace: ws,
  // The in-tree stubs, exactly as check-plugins-typecheck.sh does: this gate must catch a .d.ts
  // regression in THIS working tree, not typecheck against whatever the example has installed.
  packagesDir: resolve("packages"),
  warn: console.warn,
});

if (result.failed.length > 0) {
  console.error(formatBuildFailures(result.failed, result.outcomes.length));
  process.exit(1);
}

// Artifact count: every plugin in the workspace produced exactly one .s2sp, and it is on disk.
if (result.built.length !== ws.plugins.length) {
  fail(`built ${result.built.length} artifact(s) from ${ws.plugins.length} plugin(s)`);
}
for (const o of result.built) {
  if (!existsSync(o.outPath)) fail(`${o.name} reported ${o.outPath} but no such file exists`);
}
console.log(`  ${result.built.length} plugin(s) built from ${relative(process.cwd(), wsDir)}`);

// The sibling triangle. `pairs` counting zero is itself a failure: an example that lost its
// producer/consumer edge would make this gate pass while proving nothing at all.
let pairs = 0;
for (const consumer of ws.plugins) {
  const deps = [
    ...Object.keys(consumer.pkg.s2script?.pluginDependencies ?? {}),
    ...Object.keys(consumer.pkg.s2script?.optionalPluginDependencies ?? {}),
  ];
  const { siblings } = resolveSiblingContracts(consumer.dir, deps);
  if (siblings.size === 0) continue;

  const outPath = result.built.find((o) => o.name === consumer.name)?.outPath;
  const compiledAgainst = manifestOf(outPath).compiledAgainst ?? {};

  for (const [dep, sib] of siblings) {
    pairs++;
    // Resolve-in-place: the whole point is that the consumer keeps NO copy of the contract.
    let clean = true;
    if (localContractPath(consumer.dir, dep) !== null) {
      fail(`${consumer.name} still carries a .s2script/types/${dep}/ copy — the example must prove resolve-in-place`);
      clean = false;
    }

    const onDisk = sha256(sib.typesPath);
    if (compiledAgainst[dep] !== onDisk) {
      fail(
        `${consumer.name} compiledAgainst[${dep}] = ${compiledAgainst[dep] ?? "(absent)"} but ` +
          `sha256(${relative(wsDir, sib.typesPath)}) = ${onDisk} — sibling resolution regressed to an \`any\` stub`,
      );
      continue;
    }

    // The producer's own published hash — the value core/src/loader.rs:192 compares against.
    const producerOut = result.built.find((o) => o.name === sib.name)?.outPath;
    const published = manifestOf(producerOut).publishes?.[dep]?.typesSha256;
    if (published !== onDisk) {
      fail(`${sib.name} publishes[${dep}].typesSha256 = ${published ?? "(absent)"} but its own contract hashes to ${onDisk}`);
      continue;
    }
    if (clean) console.log(`  ${consumer.name} -> ${dep} @ ${onDisk.slice(0, 12)}… (no local copy)`);
  }
}

if (pairs === 0) {
  fail(`no plugin in ${relative(process.cwd(), wsDir)} depends on a workspace sibling — the gate would assert nothing`);
}
if (process.exitCode) process.exit(1);
console.log(`PASS: workspace build + ${pairs} sibling contract(s) resolved in place`);
NODE
S2_WS_DIR="$WS" node --experimental-strip-types --no-warnings "$GATE_JS"
