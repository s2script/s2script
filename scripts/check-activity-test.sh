#!/usr/bin/env bash
# Run the node:test suite for core/js/activity.js (the pure show-activity decision logic).
set -euo pipefail
cd "$(dirname "$0")/.."
node --test core/js/activity.test.js
echo "PASS: activity.test.js"
