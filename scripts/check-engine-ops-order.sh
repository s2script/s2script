#!/usr/bin/env bash
# S2EngineOps is declared TWICE, in two languages, and its FIELD ORDER is the ABI. The shim fills a
# C struct; core reinterprets the same bytes as a #[repr(C)] Rust struct. Both files already carry a
# comment saying they must stay index-for-index identical and change in the same commit — this is
# what makes that true.
#
# WHY THIS GATE EXISTS. Nothing else catches a divergence. There is no shared header, no bindgen, no
# link-time check: core takes a `*const S2EngineOps` and does `unsafe { *ops }`, so ANY field list
# compiles, links, passes `cargo test` and passes `make ci`. Get the order wrong by one and core's
# `ops.hook_arm_bypass(id)` calls the shim's `S2_HookInstall(id, <stale rsi>, <stale rdx>, ...)` —
# a detour attempt at a garbage address, with no error anywhere. A field appended on only ONE side is
# the milder case (core reads its own shorter prefix, in bounds) and is still caught here, because
# "inert today" is not a property anyone should have to re-derive.
#
# Compares NAMES, positionally. Types deliberately are NOT compared: they are spelled in two
# different languages and a wrapper would have to encode the mapping, which is a second thing to get
# wrong. A name mismatch is what actually catches reorder, insert, drop and rename.
#
# Fails closed: an empty or unparseable field list on either side is a failure, so reshaping one of
# the declarations cannot quietly disable the check.
set -euo pipefail
cd "$(dirname "$0")/.."

RS="core/src/v8host.rs"
H="shim/include/s2script_core.h"

rs_fields="$(
  awk '/^pub struct S2EngineOps \{/,/^\}/' "$RS" |
  sed -n 's/^[[:space:]]*pub[[:space:]]\+\([a-z_][a-z0-9_]*\)[[:space:]]*:.*/\1/p'
)"

# The header's struct is anonymous (`typedef struct { ... } S2EngineOps;`), so accumulate from the
# most recent `typedef struct {` and flush at the terminator — that is the one whose fields count.
h_fields="$(
  awk '/^typedef struct \{/ { buf = "" ; next }
       { buf = buf $0 "\n" }
       /^\} S2EngineOps;/ { printf "%s", buf; exit }' "$H" |
  sed -n 's/^[[:space:]]*s2_[a-z0-9_]*_fn[[:space:]]\+\([a-z_][a-z0-9_]*\)[[:space:]]*;.*/\1/p'
)"

rs_n="$(printf '%s' "$rs_fields" | grep -c . || true)"
h_n="$(printf '%s' "$h_fields" | grep -c . || true)"

if [ "$rs_n" -lt 10 ]; then
  echo "check-engine-ops-order: FAIL — parsed only $rs_n fields from $RS's S2EngineOps." >&2
  echo "  Expected a \`pub struct S2EngineOps { pub <name>: Option<...>, ... }\` block." >&2
  exit 1
fi
if [ "$h_n" -lt 10 ]; then
  echo "check-engine-ops-order: FAIL — parsed only $h_n fields from $H's S2EngineOps." >&2
  echo "  Expected a \`typedef struct { s2_<x>_fn <name>; ... } S2EngineOps;\` block." >&2
  exit 1
fi

if ! diff -u <(printf '%s\n' "$h_fields") <(printf '%s\n' "$rs_fields") \
       --label "$H (shim)" --label "$RS (core)" > /tmp/s2-engine-ops-diff.$$ 2>&1; then
  echo "check-engine-ops-order: FAIL — the S2EngineOps field lists DIVERGED (shim $h_n, core $rs_n)." >&2
  sed 's/^/  /' /tmp/s2-engine-ops-diff.$$ >&2
  rm -f /tmp/s2-engine-ops-diff.$$
  echo >&2
  echo "  Field ORDER is the ABI. Core does \`unsafe { *ops }\` over the shim's struct, so a" >&2
  echo "  mismatch calls the WRONG function pointer with the previous call's registers — no link" >&2
  echo "  error, no test failure. Fix both declarations in this commit." >&2
  exit 1
fi
rm -f /tmp/s2-engine-ops-diff.$$

echo "check-engine-ops-order: OK — $h_n S2EngineOps fields, same names in the same order in core and the shim"
