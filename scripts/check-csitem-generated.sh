#!/usr/bin/env bash
# Fail if the committed CsItem codegen is out of date vs a fresh generation from a pinned
# CounterStrikeSharp checkout (mirrors check-events-generated.sh).
set -euo pipefail
cd "$(cd "$(dirname "$0")/.." && pwd)"

REPO_URL="https://github.com/roflmuffin/CounterStrikeSharp"
REF="v1.0.363"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "cloning ${REPO_URL} @ ${REF} ..."
git clone --quiet --depth 1 --branch "$REF" "$REPO_URL" "$TMPDIR/CounterStrikeSharp"

node scripts/extract-csitem.mjs "$TMPDIR/CounterStrikeSharp"

git diff --exit-code -- games/cs2/js/csitem.generated.js packages/cs2/csitem.generated.d.ts

echo "PASS: CsItem codegen is up to date"
