#!/usr/bin/env bash
# Every shipped SDK capability module must have at least one consumer under
# examples/, plugins/, or tools/. Without this, the curated example set silently
# stops covering the API as new modules land — and the typecheck gate can only
# catch regressions in modules something actually imports.
set -euo pipefail
cd "$(dirname "$0")/.."

# Shipped capability modules = packages/sdk/<cap>.d.ts, minus globals.d.ts
# (ambient declarations, not importable as a module). This matches
# packages/sdk/package.json's `exports` map exactly (verified by hand when this
# gate was written) — that map is the authoritative list of what's importable,
# but reading it back out of package.json here would mean parsing JSON in bash
# for no gain, so the filename glob stands in for it.
mapfile -t modules < <(
  find packages/sdk -maxdepth 1 -name '*.d.ts' -printf '%f\n' \
    | sed 's/\.d\.ts$//' \
    | grep -vx -e globals \
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
