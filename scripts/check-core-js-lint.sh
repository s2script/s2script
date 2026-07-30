#!/usr/bin/env bash
# Lint core/js/ — the engine-generic prelude that core bakes in with include_str!.
#
# The gate that earns its keep here is `no-undef` against globals DERIVED from core's own source
# (see core/js/eslint.config.mjs): the prelude runs in a bare V8 context where the only globals are
# the ones core installs, so a native referenced from JS that Rust does not register is a silent
# runtime failure — `undefined is not a function` on whichever code path happens to hit it, which
# may be none of the ones CI exercises. This catches it at build time, in BOTH directions: a typo in
# the JS, and a native renamed in Rust whose JS caller was not updated.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "  linting core/js/ (globals derived from core/src/v8host.rs)"
# ESLint resolves its flat config from the working directory, and the config reads ../src/v8host.rs
# relative to itself — so run it from core/js/ rather than passing -c, which would leave the config's
# own relative reads resolving against the repo root.
(cd core/js && npx --no-install eslint .)

echo "PASS: core/js lints clean"
