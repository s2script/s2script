#!/usr/bin/env bash
# Every shipped SDK capability module must have at least one consumer under
# examples/, plugins/, or tools/. Without this, the curated example set silently
# stops covering the API as new modules land — and the typecheck gate can only
# catch regressions in modules something actually imports.
#
# This gate proves MODULE granularity, not symbol granularity: it verifies each
# module is imported *somewhere* in the corpus, not that its full exported
# surface (every function/type/const it ships) is actually exercised by that
# import. A module with one narrow consumer passes even if most of its API is
# untouched.
set -euo pipefail
cd "$(dirname "$0")/.."

# Shipped capability modules = packages/sdk/<cap>.d.ts, minus globals.d.ts
# (ambient declarations, not importable as a module) and index.d.ts (the root
# barrel, which re-exports those same modules). This matches
# packages/sdk/package.json's `exports` map exactly (verified by hand when this
# gate was written) — that map is the authoritative list of what's importable,
# but reading it back out of package.json here would mean parsing JSON in bash
# for no gain, so the filename glob stands in for it.
# `read -r` loop rather than `mapfile`: mapfile is bash 4+, and macOS still ships bash 3.2,
# so this gate could not run locally at all. CI-only gates defeat the point of a gate.
modules=()
while IFS= read -r __line; do [ -n "$__line" ] && modules+=("$__line"); done < <(
  # `-exec basename` rather than `-printf`: -printf is a GNU extension that BSD/macOS find lacks.
  find packages/sdk -maxdepth 1 -name '*.d.ts' -exec basename {} \; \
    | sed 's/\.d\.ts$//' \
    | grep -vx -e globals -e index \
    | sort
)

# Every @s2script/sdk/<cap> and @s2script/<pkg> imported anywhere in the corpus.
# `|| true` on the grep: with `pipefail`, a corpus that imports nothing would
# make the pipeline's exit status grep's (1), aborting the script under `set -e`
# before it got a chance to report every module as UNCOVERED.
imported=$( (grep -rhoE 'from "@s2script/(sdk/)?[a-z0-9-]+"' \
               examples plugins tools --include='*.ts' 2>/dev/null || true) \
             | sed -E 's|from "@s2script/(sdk/)?||; s|"||' \
             | sort -u )

# A root barrel import (`from "@s2script/sdk"`, not a subpath) typechecks every
# `export *` in index.d.ts, so those modules count as covered. `unsafe` is not
# re-exported and still needs a subpath consumer.
# The closing quote must follow `sdk` immediately — `from "@s2script/sdk/bans"`
# is a subpath and must not trip this.
if grep -rqE "from \"@s2script/sdk\"" examples plugins tools --include='*.ts' 2>/dev/null; then
  barrel=$(grep -oE 'export \* from "\./[a-z0-9-]+"' packages/sdk/index.d.ts \
             | sed -E 's|export \* from "\./||; s|"||')
  imported=$(printf '%s\n%s\n' "$imported" "$barrel" | sort -u)
fi

fail=0
for m in "${modules[@]}"; do
  if ! grep -qx "$m" <<<"$imported"; then
    echo "UNCOVERED: @s2script/sdk/$m has no consumer in examples/, plugins/, or tools/"
    fail=1
  fi
done

if [ "$fail" = 0 ]; then
  echo "PASS: all ${#modules[@]} shipped SDK modules have a consumer"
else
  echo "FAIL: add a cookbook recipe (examples/cookbook/src/recipes/) for each module above"
  exit 1
fi
