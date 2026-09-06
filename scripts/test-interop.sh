#!/usr/bin/env bash
# Integrated acceptance fixture gate. Live commands are intentionally operator-driven.
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-js}" in
  js)
    node --experimental-strip-types --no-warnings --test tools/interop-acceptance/test/*.test.mjs
    bash scripts/check-workspace-build.sh tools/interop-acceptance
    ;;
  native)
    cargo test -p s2script-core owned_interop_watch_churn_1000
    cargo test -p s2script-core protocol2_archive_requires_new_host_and_complete_metadata
    cargo test -p s2script-core api_version_compatible_accepts_matching_major
    ;;
  *) echo "usage: $0 [js|native]" >&2; exit 2 ;;
esac
