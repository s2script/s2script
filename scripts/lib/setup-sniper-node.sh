#!/usr/bin/env bash
# Install build-only Node into a container-local directory; print only its bin path.
# The digest is from the official v22.14.0 SHASUMS256.txt at nodejs.org.
set -euo pipefail

install_root=${1:?usage: setup-sniper-node.sh ABSOLUTE_INSTALL_ROOT [ARCHIVE]}
if [[ "$install_root" != /* ]]; then
  echo 'error: Node install root must be absolute' >&2
  exit 1
fi

archive=${2:-}
downloaded=''
if [[ -z "$archive" ]]; then
  downloaded=$(mktemp)
  archive=$downloaded
  trap 'rm -f "$downloaded"' EXIT
  curl -fsSL --retry 3 -o "$archive" \
    'https://nodejs.org/download/release/v22.14.0/node-v22.14.0-linux-x64.tar.gz'
fi

expected='9d942932535988091034dc94cc5f42b6dc8784d6366df3a36c4c9ccb3996f0c2'
actual=$(sha256sum "$archive" | cut -d ' ' -f 1)
if [[ "$actual" != "$expected" ]]; then
  echo 'error: official Node v22.14.0 Linux x64 archive checksum mismatch' >&2
  exit 1
fi

mkdir -p "$install_root"
tar -xzf "$archive" -C "$install_root" --strip-components=1
if [[ ! -x "$install_root/bin/node" ]]; then
  echo 'error: verified Node archive has no executable bin/node' >&2
  exit 1
fi
printf '%s\n' "$install_root/bin"
