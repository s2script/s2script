#!/usr/bin/env bash
# Fail if the committed event codegen is out of date vs a fresh generation from the catalog.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
( cd packages/sdk && node build.mjs >/dev/null )
node packages/sdk/dist/cli.js gen-events --check
echo "PASS: event codegen is up to date"
