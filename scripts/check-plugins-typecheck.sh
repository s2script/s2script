#!/usr/bin/env bash
# Typecheck every example, plugin, and dev tool against the shipped engine .d.ts (the Slice-5E.1 gate).
# Fails if any has a type error — a .d.ts regression that breaks them is caught here.
set -euo pipefail
cd "$(dirname "$0")/.."

# plugins/*/ also globs the bare plugins/disabled/ dir; the package.json guard skips it.
# An unmatched glob stays literal and is skipped by the same guard (no set -e trip).
candidates=()
for d in examples/*/ plugins/*/ plugins/disabled/*/ tools/*/; do
    [ -f "$d/package.json" ] || continue
    candidates+=("$d")
done

# Expand any WORKSPACE ROOT into its member plugins (design spec 2026-07-27 §3.8): a root has no
# entry point of its own, so typecheckPlugin() on one throws `no entry point` and this gate would
# fail on a perfectly good workspace. Everything else passes through untouched — a plugin that
# merely LIVES inside a workspace still typechecks as itself, which is what keeps the repo's own
# plugins/*/ entries here after the plugins/ tree became a workspace.
# `read -r` loop rather than `mapfile`: mapfile is bash 4+, and macOS still ships bash 3.2,
# so this gate could not run locally at all. CI-only gates defeat the point of a gate.
targets=()
while IFS= read -r __line; do [ -n "$__line" ] && targets+=("$__line"); done < <(node --experimental-strip-types --no-warnings -e "
  import('./packages/sdk/src/workspace/workspace.ts').then(({ findWorkspaceRoot, loadWorkspace }) => {
    const { relative, resolve } = require('node:path');
    for (const d of process.argv.slice(1)) {
      const abs = resolve(d);
      if (findWorkspaceRoot(abs) !== abs) { console.log(d); continue; }
      for (const p of loadWorkspace(abs).plugins) console.log(relative(process.cwd(), p.dir));
    }
  }).catch((e) => { console.error(String(e && e.message || e)); process.exit(1); });
" "${candidates[@]}")

fail=0
for d in "${targets[@]}"; do
    echo "=== typecheck $d ==="
    if ! node --experimental-strip-types --no-warnings -e "
      import('./packages/sdk/src/typecheck/typecheck.ts').then(({typecheckPlugin, formatDiagnostics}) => {
        const r = typecheckPlugin('$d', { packagesDir: 'packages' });
        if (!r.ok) { console.error(formatDiagnostics(r.diagnostics)); process.exit(1); }
        console.log('  OK');
      });
    "; then fail=1; fi
done
[ "$fail" = 0 ] && echo "PASS: all plugins and examples typecheck" || { echo "FAIL: a plugin or example has type errors"; exit 1; }
