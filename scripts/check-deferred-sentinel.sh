#!/usr/bin/env bash
# S2_DISPATCH_DEFERRED is the deferred-dispatch queue's ONE cross-language invariant: core returns
# it, the shim tests for it EXACTLY (`== S2_DISPATCH_DEFERRED`), and the two declarations are
# separate literals in two languages. Both files carry a comment asserting they are kept in sync —
# this is what makes that true.
#
# Without this gate, editing one of the two numbers leaves cargo test, the shim link,
# check-shim-symbols and `make ci` all green, and silently reverts the whole slice to the
# undiagnosable silent drop it exists to fix: every dispatch entry's deferral signal would read as
# an ordinary result, so nothing would ever be queued and nothing would ever say so.
#
# Fails closed: a missing or unparseable declaration on either side is a failure, so renaming or
# reshaping one of them cannot quietly disable the check.
set -euo pipefail
cd "$(dirname "$0")/.."

RS="core/src/ffi.rs"
H="shim/include/s2script_core.h"

# core:  pub const S2_DISPATCH_DEFERRED: c_int = -1000;
rs_val="$(sed -n 's/^[[:space:]]*pub const S2_DISPATCH_DEFERRED[[:space:]]*:[[:space:]]*c_int[[:space:]]*=[[:space:]]*(*\(-\{0,1\}[0-9_]\+\))*[[:space:]]*;.*/\1/p' "$RS" | tr -d '_')"
# shim:  #define S2_DISPATCH_DEFERRED (-1000)
h_val="$(sed -n 's/^[[:space:]]*#define[[:space:]]\+S2_DISPATCH_DEFERRED[[:space:]]\+(*\(-\{0,1\}[0-9]\+\))*[[:space:]]*$/\1/p' "$H")"

if [ -z "$rs_val" ]; then
  echo "check-deferred-sentinel: FAIL — no \`pub const S2_DISPATCH_DEFERRED: c_int = <int>;\` in $RS" >&2
  exit 1
fi
if [ -z "$h_val" ]; then
  echo "check-deferred-sentinel: FAIL — no \`#define S2_DISPATCH_DEFERRED (<int>)\` in $H" >&2
  exit 1
fi
if [ "$(printf '%s\n' "$rs_val" | wc -l)" -ne 1 ] || [ "$(printf '%s\n' "$h_val" | wc -l)" -ne 1 ]; then
  echo "check-deferred-sentinel: FAIL — more than one declaration (rs='$rs_val' h='$h_val')" >&2
  exit 1
fi

if [ "$rs_val" != "$h_val" ]; then
  echo "check-deferred-sentinel: FAIL — the deferred-dispatch sentinel DRIFTED" >&2
  echo "  $RS: $rs_val" >&2
  echo "  $H:  $h_val" >&2
  echo "  These MUST be byte-identical. A mismatch silently un-defers every re-entrant dispatch." >&2
  exit 1
fi

# The sentinel must also stay outside every value a dispatch entry can return: HookResult is 0..=3,
# the boolean entries are 0..=1, the catch_unwind fallbacks are 0 (and -99 for game_frame), and the
# header's "unavailable" idiom is -1. core/src/ffi.rs's
# deferred_sentinel_cannot_collide_with_any_dispatch_result pins the same ranges from the Rust side;
# this keeps the C side honest if the value is ever moved.
case "$rs_val" in
  0|1|2|3|-1|-2|-3|-99)
    echo "check-deferred-sentinel: FAIL — $rs_val collides with a real dispatch result" >&2
    exit 1 ;;
esac
if [ "$rs_val" -ge 0 ] 2>/dev/null; then
  echo "check-deferred-sentinel: FAIL — the sentinel must be NEGATIVE (a positive one fails closed" \
       "the wrong way for \`if (r >= 2) SUPERCEDE\` call sites)" >&2
  exit 1
fi

echo "check-deferred-sentinel: OK — S2_DISPATCH_DEFERRED = $rs_val in both core and the shim header"
