#!/usr/bin/env bash
# Run from the repository root. No Node dependency: native builds use this too.
set -euo pipefail
version="${1:-${VERSION:-}}"
if [ -z "$version" ] && [ "${GITHUB_REF_TYPE:-}" = tag ]; then
    version="${GITHUB_REF_NAME:-}"
fi
if [ -z "$version" ]; then
    tag=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
    sha=$(git rev-parse --short=12 HEAD 2>/dev/null || true)
    if [ -z "$sha" ]; then
        echo 'ERROR: source without Git history requires VERSION (for example VERSION=1.2.3)' >&2
        exit 1
    fi
    if [ -n "$tag" ]; then
        count=$(git rev-list --count "$tag"..HEAD)
        version="${tag#v}"
    else
        count=$(git rev-list --count HEAD)
        version=0.0.0
    fi
    if [ -z "$tag" ] || [ "$count" != 0 ]; then
        # Build metadata is replaced by commit provenance in development builds.
        version="${version%%+*}"
        case "$version" in *-*) version="$version.dev.$count.g$sha" ;; *) version="$version-dev.$count.g$sha" ;; esac
    fi
fi
version="${version#v}"
if ! [[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?(\+[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$ ]]; then
    echo "ERROR: invalid framework version: $version" >&2
    exit 1
fi
# Numeric prerelease identifiers must not contain leading zeroes.
core="${version%%+*}"
if [[ "$core" == *-* ]]; then
    IFS=. read -r -a identifiers <<< "${core#*-}"
    for identifier in "${identifiers[@]}"; do
        if [[ "$identifier" =~ ^0[0-9]+$ ]]; then
            echo "ERROR: invalid numeric prerelease identifier: $identifier" >&2
            exit 1
        fi
    done
fi
printf '%s\n' "$version"
