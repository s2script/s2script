#!/usr/bin/env bash
# S2EngineOps is generated from one list (core/engine-ops.jsonc). Field ORDER is the ABI: the shim
# fills a C struct; core reinterprets the same bytes as a #[repr(C)] Rust struct. This gate first
# checks the generated copies are fresh, then diffs names + arity so a generator bug cannot ship
# a silent reorder.
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

# Freshness first: the three generated copies must match the list. A stale
# generated file is how the two-language lockstep used to drift.
python3 scripts/gen-engine-ops.py --check

RS="core/src/engine_ops.generated.rs"
H="shim/include/s2script_engine_ops.generated.h"

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

# ---------------------------------------------------------------------------
# ARITY. Names and order agreeing proves the fields LINE UP; it says nothing about whether each one
# is the same FUNCTION. `fn(a, b)` on one side and `fn(a, b, c)` on the other passes every check
# above and then reads a garbage third argument off the register the caller never set.
#
# This is the same disease as the arg-width bug: one ABI, written down twice by hand, with a gate
# that only checked the two copies against each other in the dimensions it happened to look at.
#
# Field `schema_offset` -> Rust alias `SchemaOffsetFn` -> C typedef `s2_schema_offset_fn`.
# ---------------------------------------------------------------------------
python3 - "$RS" "$H" <<'PYEOF'
import re, sys
rs_src, h_src = open(sys.argv[1]).read(), open(sys.argv[2]).read()

def balanced(rest):
    """Take chars up to the paren that closes the one just consumed."""
    depth, out = 1, ""
    for ch in rest:
        if ch == "(": depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0: return out
        out += ch
    return out

def split_top(params):
    """Split a C/Rust parameter list on TOP-LEVEL commas — a function-pointer parameter has its own."""
    out, depth, cur = [], 0, ""
    for ch in params:
        if ch in "(<": depth += 1
        elif ch in ")>": depth -= 1
        if ch == "," and depth == 0:
            out.append(cur); cur = ""
        else:
            cur += ch
    if cur.strip(): out.append(cur)
    return [p for p in (x.strip() for x in out) if p and p != "void"]

# Rust: pub type FooFn = extern "C" fn(<params>) [-> ret];
rs_arity = {}
# NOT line-anchored and NOT non-greedy: a parameter list may span lines (EngineCallInvokeFn does),
# and a function-pointer parameter carries its own parens. Match the opening paren, then balance.
for m in re.finditer(r'^(?:pub )?type (\w+)\s*=\s*(?:unsafe\s+)?extern "C" fn\(', rs_src, re.M):
    # Balance the parens ourselves instead of a non-greedy `(.*?)\)`: a function-pointer PARAMETER
    # has its own parens, and a lazy match stops at the inner one — truncating the list and
    # reporting an arity that is wrong in BOTH directions.
    rs_arity[m.group(1)] = len(split_top(balanced(rs_src[m.end():])))

# Rust struct field -> alias
fields = []
mstruct = re.search(r'pub struct S2EngineOps \{(.*?)\n\}', rs_src, re.S)
for m in re.finditer(r'^\s*pub (\w+)\s*:\s*Option<(\w+)>', mstruct.group(1), re.M):
    fields.append((m.group(1), m.group(2)))

# C: typedef <ret> (*s2_foo_fn)(<params>);
h_arity = {}
for m in re.finditer(r'typedef\s+[\w\s*]+\(\s*\*\s*(s2_\w+_fn)\s*\)\s*\(', h_src):
    h_arity[m.group(1)] = len(split_top(balanced(h_src[m.end():])))

bad, checked, unresolved = [], 0, []
for field, alias in fields:
    ctype = "s2_" + field + "_fn"
    if alias not in rs_arity or ctype not in h_arity:
        missing = "core alias " + alias if alias not in rs_arity else "shim typedef " + ctype
        unresolved.append(f"  {field}: cannot resolve {missing}")
        continue
    checked += 1
    if rs_arity[alias] != h_arity[ctype]:
        bad.append(f"  {field}: core {alias} takes {rs_arity[alias]} arg(s), shim {ctype} takes {h_arity[ctype]}")

if bad:
    print("check-engine-ops-order: FAIL — S2EngineOps field ARITY diverged between core and the shim:",
          file=sys.stderr)
    print("\n".join(bad), file=sys.stderr)
    print("\n  Same field, same position, DIFFERENT signature: the callee reads an argument the caller\n"
          "  never set. Names and order agreeing does not make two declarations the same function.",
          file=sys.stderr)
    sys.exit(1)
# A field the gate cannot see is a field it is not checking. Reporting OK while any go unexamined is
# how this same gate reported "OK — 70 field signature(s) agree on arity" with a live 1-vs-3 ABI
# divergence sitting in one of the 44 it silently skipped.
if unresolved:
    print(f"check-engine-ops-order: FAIL — {len(unresolved)} S2EngineOps field(s) could not be "
          f"resolved, so their arity is UNCHECKED:", file=sys.stderr)
    print("\n".join(unresolved), file=sys.stderr)
    print("\n  Either the alias/typedef regex is stale or a declaration changed shape. An unchecked\n"
          "  field is not a passing field — fix the parse or the declaration, do not lower the bar.",
          file=sys.stderr)
    sys.exit(1)
print(f"check-engine-ops-order: OK — all {checked} S2EngineOps field signature(s) agree on arity")
PYEOF

