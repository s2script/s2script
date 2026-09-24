#!/usr/bin/env bash
# Exercise the exact pinned Node archive installer without a Rust/C++ or Docker build.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ ! -f scripts/lib/setup-sniper-node.sh ]; then
  echo 'FAIL: sniper Node setup helper is missing' >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# macOS has shasum rather than sha256sum; CI and the Bullseye container use sha256sum.
if ! command -v sha256sum >/dev/null 2>&1; then
  mkdir -p "$tmp/bin"
  printf '#!/usr/bin/env bash\nexec shasum -a 256 "$@"\n' > "$tmp/bin/sha256sum"
  chmod +x "$tmp/bin/sha256sum"
  export PATH="$tmp/bin:$PATH"
fi

printf 'corrupt archive\n' > "$tmp/corrupt.tar.gz"
if bash scripts/lib/setup-sniper-node.sh "$tmp/rejected" "$tmp/corrupt.tar.gz" > "$tmp/rejected.stdout" 2> "$tmp/rejected.stderr"; then
  echo 'FAIL: corrupt Node archive was accepted' >&2
  exit 1
fi
if [ -e "$tmp/rejected/bin/node" ] || [ -s "$tmp/rejected.stdout" ]; then
  echo 'FAIL: corrupt Node archive was extracted or exposed on PATH' >&2
  exit 1
fi
grep -q 'checksum' "$tmp/rejected.stderr"

archive="$tmp/node-v22.14.0-linux-x64.tar.gz"
curl -fsSL --retry 3 -o "$archive" \
  'https://nodejs.org/download/release/v22.14.0/node-v22.14.0-linux-x64.tar.gz'
node_bin=$(bash scripts/lib/setup-sniper-node.sh "$tmp/verified" "$archive")
test "$node_bin" = "$tmp/verified/bin"
test -x "$node_bin/node"

# Reuse the verified official archive to exercise the builder's download branch without
# fetching the same 54 MB a second time in the host gate.
mkdir -p "$tmp/fakebin"
cat > "$tmp/fakebin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
dest=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    -o) dest=$2; shift 2 ;;
    https://nodejs.org/download/release/v22.14.0/node-v22.14.0-linux-x64.tar.gz) shift ;;
    -fsSL|--retry) if [[ "$1" == --retry ]]; then shift 2; else shift; fi ;;
    *) echo "unexpected Node download argument: $1" >&2; exit 1 ;;
  esac
done
test -n "$dest"
cp "$S2_TEST_NODE_ARCHIVE" "$dest"
SH
chmod +x "$tmp/fakebin/curl"
download_bin=$(S2_TEST_NODE_ARCHIVE="$archive" PATH="$tmp/fakebin:$PATH" \
  bash scripts/lib/setup-sniper-node.sh "$tmp/downloaded")
test "$download_bin" = "$tmp/downloaded/bin"
test -x "$download_bin/node"

# The real builder must use the verified helper before handing control to package-addon.
python3 - <<'PY'
from pathlib import Path
s = Path('scripts/build-sniper.sh').read_text()
helper = s.find('scripts/lib/setup-sniper-node.sh')
package = s.find('./scripts/package-addon.sh')
assert 0 <= helper < package, 'sniper builder must install verified Node before packaging'
assert 'export PATH="$NODE_BIN:$PATH"' in s, 'sniper builder must expose only local Node on PATH'
PY

echo 'test-sniper-node: corrupt archive rejected before extraction; official archive verified through supplied and download paths; builder ordering OK'
