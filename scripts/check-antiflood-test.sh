#!/usr/bin/env bash
# Run the node:test suite for the antiflood pure token-decay flood model (plugins/antiflood/src/flood.ts).
set -euo pipefail
cd "$(dirname "$0")/.."
node --experimental-strip-types --no-warnings --test plugins/antiflood/src/flood.test.mjs
echo "PASS: antiflood flood.test.mjs"
