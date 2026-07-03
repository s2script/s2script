#!/usr/bin/env bash
# Fail if the committed nav codegen is out of date vs a fresh generation from nav-targets.json + the catalog.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
( cd packages/cli && node build.mjs >/dev/null )
node packages/cli/dist/cli.js gen-nav --check
echo "PASS: nav codegen is up to date"
