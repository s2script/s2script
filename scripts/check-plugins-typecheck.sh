#!/usr/bin/env bash
# Typecheck every example and plugin against the shipped engine .d.ts (the Slice-5E.1 gate).
# Fails if any plugin or example has a type error — a .d.ts regression that breaks them is caught here.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
# plugins/*/ also globs the bare plugins/disabled/ dir; the package.json guard skips it.
# An unmatched glob stays literal and is skipped by the same guard (no set -e trip).
for d in examples/*/ plugins/*/ plugins/disabled/*/; do
    [ -f "$d/package.json" ] || continue
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
