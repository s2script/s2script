#!/usr/bin/env bash
# THE JS/TS gate suite. Single source of truth: .github/workflows/ci-js.yml runs exactly
# this script and nothing else, and so does `make ci-js`. If a gate is not in here, it
# does not run — do not add a gate step to the workflow YAML.
set -euo pipefail
cd "$(dirname "$0")/.."

# The package-lock.json drift guard: the same `npm ci` the changesets release pipeline
# runs on main, so drift fails on the PR instead of after merge. CI-only, because npm ci
# deletes node_modules and a gate script must not be destructive to a working tree.
if [ -n "${CI:-}" ]; then
  echo "== npm ci (package-lock.json in sync) =="
  npm ci
else
  echo "== npm ci SKIPPED (local run — use 'CI=1 make ci-js' to run the lockfile guard) =="
fi

# Codegen freshness. Globbed so a future check-*-generated.sh starts running here with no
# edit. check-licenses-generated.sh is excluded: it needs a Rust toolchain and a populated
# cargo registry, so it lives in ci-native.sh.
for f in scripts/check-*-generated.sh; do
  case "$f" in */check-licenses-generated.sh) continue ;; esac
  echo "== $f =="
  bash "$f"
done

echo "== check-plugins-typecheck.sh (the 5E.1 gate) =="
bash scripts/check-plugins-typecheck.sh

echo "== check-activity-test.sh =="
bash scripts/check-activity-test.sh

echo "== check-antiflood-test.sh =="
bash scripts/check-antiflood-test.sh

echo "== test-gate.sh =="
bash scripts/test-gate.sh

echo "ci-js: all JS gates passed"
