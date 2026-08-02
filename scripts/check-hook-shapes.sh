#!/usr/bin/env bash
# The inbound-hook SHAPE vocabulary is declared twice — `kShapes` in shim/src/hook_dispatch.cpp and
# `SHAPES` in core/src/gamedata_hooks.rs — and the (name, id) pairing is an ABI. This diffs them.
#
# WHY THIS GATE EXISTS. A hook descriptor names a shape as a STRING; core turns it into an id and
# hands that id to S2_HookInstall, which selects the compiled thunk — i.e. the C++ SIGNATURE the
# engine will call. If the two tables ever disagree about which id a name means, a hook installs a
# thunk with the WRONG ABI on a real engine function: arguments read out of the wrong registers, and
# a corrupted stack on the first call. Nothing else catches it. The name resolves, the id is in
# range, the install succeeds, and `cargo test` + `make ci` are green — the shim's own
# "unknown hook shape id" reason only fires for an id NEITHER side has.
#
# Core cannot simply ask the shim: S2Hook_ShapeFromName is hidden (-fvisibility=hidden) and reaching
# it would mean a new engine-op field per lookup. Mirroring plus a gate is the same answer this repo
# already uses for the other cross-language constants (check-deferred-sentinel.sh,
# check-engine-ops-order.sh).
#
# Fails closed: an empty or unparseable table on either side is a failure, so reshaping one of the
# declarations cannot quietly disable the check.
set -euo pipefail
cd "$(dirname "$0")/.."

CPP="shim/src/hook_dispatch.cpp"
H="shim/src/hook_dispatch.h"
RS="core/src/gamedata_hooks.rs"

# Shim: `{ S2_HOOK_SHAPE_THIS_VOID, "this_void" },` — the enumerator's VALUE comes from the header,
# so resolve the name through it rather than assuming positional order.
shim_pairs="$(
  awk '/^const ShapeEntry kShapes\[\] = \{/,/^\};/' "$CPP" |
  sed -n 's/^[[:space:]]*{[[:space:]]*\(S2_HOOK_SHAPE_[A-Z0-9_]*\)[[:space:]]*,[[:space:]]*"\([a-z0-9_]*\)".*/\1 \2/p' |
  while read -r enum name; do
    val="$(sed -n "s/^[[:space:]]*${enum}[[:space:]]*=[[:space:]]*\([0-9]\+\).*/\1/p" "$H" | head -1)"
    [ -n "$val" ] || { echo "UNRESOLVED-ENUM $enum" ; continue; }
    echo "$name $val"
  done | sort
)"

# Core: `&[("this_void", 0), ("this_f32_i32_i32_i32", 1)];`
core_pairs="$(
  awk '/^pub\(crate\) const SHAPES/,/;/' "$RS" |
  grep -oE '\("[a-z0-9_]+", *[0-9]+\)' |
  sed -E 's/\("([a-z0-9_]+)", *([0-9]+)\)/\1 \2/' | sort
)"

shim_n="$(printf '%s' "$shim_pairs" | grep -c . || true)"
core_n="$(printf '%s' "$core_pairs" | grep -c . || true)"

if [ "$shim_n" -lt 1 ]; then
  echo "check-hook-shapes: FAIL — parsed no shapes from $CPP's kShapes table." >&2
  exit 1
fi
if [ "$core_n" -lt 1 ]; then
  echo "check-hook-shapes: FAIL — parsed no shapes from $RS's SHAPES table." >&2
  exit 1
fi
if printf '%s\n' "$shim_pairs" | grep -q UNRESOLVED-ENUM; then
  echo "check-hook-shapes: FAIL — a kShapes enumerator has no value in $H:" >&2
  printf '%s\n' "$shim_pairs" | grep UNRESOLVED-ENUM | sed 's/^/    /' >&2
  exit 1
fi

if ! diff -u <(printf '%s\n' "$shim_pairs") <(printf '%s\n' "$core_pairs") \
       --label "$CPP + $H (shim)" --label "$RS (core)"; then
  echo >&2
  echo "check-hook-shapes: FAIL — the shape vocabularies DIVERGED (shim $shim_n, core $core_n)." >&2
  echo "  A shape id selects the compiled THUNK, i.e. the C++ signature the engine calls. A" >&2
  echo "  mismatch installs a detour with the wrong ABI — no link error, no test failure, a" >&2
  echo "  corrupted stack on the first engine call. Fix both tables in this commit." >&2
  exit 1
fi

echo "check-hook-shapes: OK — $shim_n hook shape(s), same names and ids in core and the shim"
