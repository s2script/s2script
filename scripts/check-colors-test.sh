#!/usr/bin/env bash
# Run the node:test suite for core/js/colors.js (the pure colour-tag expander).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test core/js/colors.test.js
echo "PASS: colors.test.js"
