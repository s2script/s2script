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

# The SDK's own unit suite (codegen models + emitters, manifest/gamedata validation, CLI). It was
# never wired in, so 230 tests across 30 files — including every codegen gate's underlying model —
# could go red without CI noticing. Found while adding the navgen writable-allowlist tests.
echo "== packages/sdk unit suite =="
( cd packages/sdk && node --experimental-strip-types --no-warnings --test test/*.test.mjs )

# Phrase-key declarations are DERIVED and gitignored, so CI must write them before anything
# typechecks a plugin — otherwise keys silently widen to `string` and the check passes vacuously.
echo "== sync-phrase-types.mjs (write the derived phrase-key declarations) =="
node --experimental-strip-types --no-warnings scripts/sync-phrase-types.mjs

echo "== check-phrase-types.sh (declarations match the loaded phrase files) =="
bash scripts/check-phrase-types.sh

echo "== check-changeset.sh (published package changes carry a changeset) =="
bash scripts/check-changeset.sh

echo "== check-changeset-ignore.sh (.changeset/config.json ignore matches the on-disk plugin set) =="
bash scripts/check-changeset-ignore.sh

echo "== check-core-js-lint.sh (core/js prelude references only natives core registers) =="
bash scripts/check-core-js-lint.sh

echo "== check-plugins-typecheck.sh (the 5E.1 gate) =="
bash scripts/check-plugins-typecheck.sh

echo "== check-workspace-build.sh (sibling contracts resolve in place, not to an any-stub) =="
bash scripts/check-workspace-build.sh

echo "== check-examples-coverage.sh (every shipped SDK module has a consumer) =="
bash scripts/check-examples-coverage.sh

echo "== check-activity-test.sh =="
bash scripts/check-activity-test.sh

echo "== check-colors-test.sh (colour-tag expander) =="
bash scripts/check-colors-test.sh

echo "== check-antiflood-test.sh =="
bash scripts/check-antiflood-test.sh

echo "== test-release-notes.sh (release notes keep the changelogs) =="
bash scripts/test-release-notes.sh

echo "== test-gate.sh =="
bash scripts/test-gate.sh

echo "ci-js: all JS gates passed"
