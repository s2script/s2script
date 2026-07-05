#!/usr/bin/env bash
# Typecheck every example plugin against the shipped engine .d.ts (the Slice-5E.1 gate).
# Fails if any example has a type error — a .d.ts regression that breaks the examples is caught here.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
for d in examples/*/; do
  [ -f "$d/package.json" ] || continue
  echo "=== typecheck $d ==="
  if ! node --experimental-strip-types --no-warnings -e "
    import('./packages/cli/src/typecheck/typecheck.ts').then(({typecheckPlugin, formatDiagnostics}) => {
      const r = typecheckPlugin('$d', { packagesDir: 'packages' });
      if (!r.ok) { console.error(formatDiagnostics(r.diagnostics)); process.exit(1); }
      console.log('  OK');
    });
  "; then fail=1; fi
done
[ "$fail" = 0 ] && echo "PASS: all examples typecheck" || { echo "FAIL: an example has type errors"; exit 1; }
