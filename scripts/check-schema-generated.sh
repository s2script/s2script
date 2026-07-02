#!/usr/bin/env bash
# Fail if the committed schema codegen is out of date vs a fresh generation from the catalog.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
( cd packages/cli && node build.mjs >/dev/null )
node packages/cli/dist/cli.js gen-schema --check
echo "PASS: schema codegen is up to date"
