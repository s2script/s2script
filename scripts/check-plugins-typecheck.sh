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

# Each target is an independent tsc program, so they run in parallel (one per CPU; S2_JOBS
# overrides). Each target's output is buffered to its own file and printed in target order, so
# the log reads exactly as the old serial loop's did.
jobs="${S2_JOBS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)}"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
i=0
for d in "${targets[@]}"; do
    printf '%s\0%s\0' "$i" "$d"
    i=$((i + 1))
done | xargs -0 -n 2 -P "$jobs" sh -c '
    if node --experimental-strip-types --no-warnings -e "
      import(\"./packages/sdk/src/typecheck/typecheck.ts\").then(({typecheckPlugin, formatDiagnostics}) => {
        const r = typecheckPlugin(process.argv[1], { packagesDir: \"packages\" });
        if (!r.ok) { console.error(formatDiagnostics(r.diagnostics)); process.exit(1); }
        console.log(\"  OK\");
      });
    " "$2" > "$0/$1.log" 2>&1; then :; else touch "$0/$1.fail"; fi
  ' "$out" || true

fail=0
i=0
for d in "${targets[@]}"; do
    echo "=== typecheck $d ==="
    cat "$out/$i.log"
    [ -e "$out/$i.fail" ] && fail=1
    i=$((i + 1))
done
[ "$i" -gt 0 ] || { echo "FAIL: no typecheck targets found"; exit 1; }
[ "$fail" = 0 ] && echo "PASS: all plugins and examples typecheck" || { echo "FAIL: a plugin or example has type errors"; exit 1; }
