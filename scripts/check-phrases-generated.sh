#!/usr/bin/env bash
# Fail if a committed translations/*.phrases.json is out of date vs its in-code seed.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
node --experimental-strip-types --no-warnings scripts/gen-phrases.mjs --check
