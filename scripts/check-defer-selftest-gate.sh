#!/usr/bin/env bash
# The deferred-dispatch SELFTEST is armed by ONE environment variable read in TWO languages: core
# installs the `__s2_defer_selftest` native only when it is set, and the shim's op refuses to do
# anything unless it reads the same variable. Two literals, two files, no compiler between them.
#
# Both failure directions are silent and both are bad:
#   * rename core's -> the native never installs, the gate command reports "native ABSENT", and the
#     operator concludes the feature is broken when only the gate is.
#   * rename the shim's -> the native installs and every call is REFUSED, so a green-looking run
#     proves nothing at all. That is the exact "it looked fine" failure this whole slice exists to
#     end, reintroduced one layer up.
#
# This also pins the safety property the selftest rests on: the native is registered INSIDE the
# `if defer_selftest_armed()` guard. Registration-gating (not call-gating) is what makes the
# synthetic re-entrancy unreachable in a production process — the property is not on the global at
# all. core/src/ffi.rs's defer_selftest_native_exists_only_when_the_env_var_is_set pins the same
# property behaviourally; this keeps the intent legible at the call site.
#
# Fails closed: an unparseable or missing declaration on either side is a failure, so reshaping one
# of them cannot quietly disable the check.
set -euo pipefail
cd "$(dirname "$0")/.."

RS="core/src/v8host.rs"
CPP="shim/src/s2script_mm.cpp"

# core:  std::env::var_os("S2_DEFER_SELFTEST").is_some()
rs_var="$(sed -n 's/.*std::env::var_os(\"\([A-Z0-9_]\+\)\").*/\1/p' "$RS" | sort -u)"
# shim:  static const bool on = (getenv("S2_DEFER_SELFTEST") != nullptr);   (inside S2_DeferSelfTestArmed)
cpp_var="$(sed -n '/^static bool S2_DeferSelfTestArmed/,/^}/p' "$CPP" \
           | sed -n 's/.*getenv("\([A-Z0-9_]\+\)").*/\1/p')"

if [ -z "$rs_var" ]; then
  echo "check-defer-selftest-gate: FAIL — no \`std::env::var_os(\"<NAME>\")\` in $RS" >&2
  exit 1
fi
if [ "$(printf '%s\n' "$rs_var" | wc -l)" -ne 1 ]; then
  echo "check-defer-selftest-gate: FAIL — $RS reads more than one env var, so this gate can no" \
       "longer tell which one arms the selftest:" >&2
  printf '  %s\n' $rs_var >&2
  exit 1
fi
if [ -z "$cpp_var" ] || [ "$(printf '%s\n' "$cpp_var" | wc -l)" -ne 1 ]; then
  echo "check-defer-selftest-gate: FAIL — S2_DeferSelfTestArmed in $CPP does not read exactly one" \
       "getenv(\"<NAME>\") (found: '${cpp_var:-none}')" >&2
  exit 1
fi
if [ "$rs_var" != "$cpp_var" ]; then
  echo "check-defer-selftest-gate: FAIL — the selftest's arming variable DRIFTED" >&2
  echo "  $RS:  $rs_var" >&2
  echo "  $CPP: $cpp_var" >&2
  echo "  These MUST be identical, or the gate silently proves nothing." >&2
  exit 1
fi

# The registration must be guarded. Find the one set_native call and require the nearest preceding
# code line (comments and blanks skipped) to be the arming `if`.
reg_lines="$(grep -n 'set_native(.*"__s2_defer_selftest"' "$RS" | cut -d: -f1)"
if [ "$(printf '%s\n' "$reg_lines" | grep -c .)" -ne 1 ]; then
  echo "check-defer-selftest-gate: FAIL — expected exactly ONE set_native of __s2_defer_selftest" \
       "in $RS (found: $(printf '%s\n' "$reg_lines" | grep -c .))" >&2
  exit 1
fi
guard="$(sed -n "1,$((reg_lines - 1))p" "$RS" \
         | grep -vE '^[[:space:]]*(//.*)?$' | tail -1)"
case "$guard" in
  *"if defer_selftest_armed() {"*) ;;
  *)
    echo "check-defer-selftest-gate: FAIL — __s2_defer_selftest is not registered inside" \
         "\`if defer_selftest_armed() {\`; the line before it is:" >&2
    echo "  $guard" >&2
    echo "  Gating the CALL instead of the REGISTRATION would leave the native reachable in a" \
         "production process. Do not relax this." >&2
    exit 1 ;;
esac

echo "check-defer-selftest-gate: OK — armed by \$$rs_var in both core and the shim; the native is" \
     "registration-gated"
