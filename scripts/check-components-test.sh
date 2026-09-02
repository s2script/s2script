#!/usr/bin/env bash
# Run the node:test suite for games/cs2/js/components.js (the ctx.ui.components() library).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test games/cs2/js/components.test.js games/cs2/js/hudinput.test.js games/cs2/js/menuhud.test.js games/cs2/js/voterail.test.js games/cs2/js/hudkit-prelude.test.js
echo "PASS: components.test.js hudinput.test.js menuhud.test.js voterail.test.js hudkit-prelude.test.js"
