#!/usr/bin/env bash
# Applies patches/hl2sdk/*.patch, in order, to the vendored third_party/hl2sdk checkout.
#
# hl2sdk lags Valve (docs/ARCHITECTURE.md §2.0), so we carry fixes ahead of upstream as patches on
# the pinned submodule. shim/CMakeLists.txt runs this at configure time, so every build — local,
# CI and the sniper container — compiles against the patched headers without a manual step.
#
# Idempotent: a patch that is already applied (its reverse applies cleanly) is skipped, so
# re-running cmake never double-applies. A patch that neither applies nor is already applied
# fails loudly: the submodule moved under it and the patch must be regenerated or dropped.
#
# --verify applies nothing. It exits 0 only when third_party/hl2sdk's working tree is exactly the
# pinned commit plus these patches, so a build that requires a clean source tree can accept the
# carried patches without accepting any other edit to the submodule.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="$root/third_party/hl2sdk"
shopt -s nullglob
patches=("$root"/patches/hl2sdk/*.patch)
[ ${#patches[@]} -eq 0 ] && exit 0
[ -d "$sdk/public" ] || { echo "apply-hl2sdk-patches: $sdk is not checked out" >&2; exit 1; }

if [ "${1:-}" = --verify ]; then
    # Build the expected tree (HEAD + every patch) and the actual one (HEAD + the working tree) in
    # throwaway indexes, so neither touches the submodule's own index.
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    [ -z "$(git -C "$sdk" ls-files --others --exclude-standard)" ] || exit 1
    GIT_INDEX_FILE="$tmp/expected" git -C "$sdk" read-tree HEAD
    for p in "${patches[@]}"; do
        GIT_INDEX_FILE="$tmp/expected" git -C "$sdk" apply --cached "$p" 2>/dev/null || exit 1
    done
    GIT_INDEX_FILE="$tmp/actual" git -C "$sdk" read-tree HEAD
    GIT_INDEX_FILE="$tmp/actual" git -C "$sdk" add -u
    [ "$(GIT_INDEX_FILE="$tmp/expected" git -C "$sdk" write-tree)" = \
      "$(GIT_INDEX_FILE="$tmp/actual" git -C "$sdk" write-tree)" ]
    exit
fi

for p in "${patches[@]}"; do
    name="$(basename "$p")"
    if git -C "$sdk" apply --reverse --check "$p" 2>/dev/null; then
        echo "apply-hl2sdk-patches: $name already applied"
    elif git -C "$sdk" apply --check "$p" 2>/dev/null; then
        git -C "$sdk" apply "$p"
        echo "apply-hl2sdk-patches: $name applied"
    else
        echo "apply-hl2sdk-patches: FAIL — $name neither applies nor is already applied to" \
             "third_party/hl2sdk; regenerate it against the pinned commit or drop it" >&2
        exit 1
    fi
done
