#!/usr/bin/env bash
# Fail if a plugin's generated phrase-key declaration is out of date with the phrase files it loads.
#
# The declaration is derived (types only, no phrase text) and gitignored — this is the gate that
# stops a stale one silently accepting a key the plugin can no longer resolve, or rejecting one it
# now can.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
node --experimental-strip-types --no-warnings scripts/sync-phrase-types.mjs --check
