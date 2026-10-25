#!/usr/bin/env bash
# Run the node:test conformance suites for the CS2 engine-function adapters
# (games/cs2/js/adapters: legacy.acquire.v1, legacy.hud-click.v1, legacy.damage.v1, cs2.chat-visibility.v1 and the cs2.weapon-* hooks).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test games/cs2/js/adapters/acquire.test.js games/cs2/js/adapters/hud-click.test.js games/cs2/js/adapters/damage.test.js games/cs2/js/adapters/chat-visibility.test.js games/cs2/js/adapters/weapon-hooks.test.js
echo "PASS: adapters/acquire.test.js adapters/hud-click.test.js adapters/damage.test.js adapters/chat-visibility.test.js adapters/weapon-hooks.test.js"
