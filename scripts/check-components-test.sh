#!/usr/bin/env bash
# Run the node:test suite for games/cs2/js/components.js (the ctx.ui.components() library).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test games/cs2/js/components.test.js
echo "PASS: components.test.js"
