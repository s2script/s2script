#!/usr/bin/env bash
# Run the node:test conformance suites for the CS2 engine-function adapters
# (games/cs2/js/adapters: legacy.acquire.v1, legacy.hud-click.v1 and legacy.damage.v1).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test games/cs2/js/adapters/acquire.test.js games/cs2/js/adapters/hud-click.test.js games/cs2/js/adapters/damage.test.js
echo "PASS: adapters/acquire.test.js adapters/hud-click.test.js adapters/damage.test.js"
