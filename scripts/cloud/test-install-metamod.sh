#!/usr/bin/env bash
# Fixture tests for scripts/cloud/install.sh Metamod refresh (T7 Steps 1–2).
# Never touches the live host docker/metamod/ or compose/RCON.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALL="$ROOT/scripts/cloud/install.sh"
fail=0
ran=0
ok()   { echo "  ok   $1"; ran=$((ran + 1)); }
bad()  { echo "  FAIL $1" >&2; fail=$((fail + 1)); ran=$((ran + 1)); }

assert_file() {
  local path="$1" msg="$2"
  if [ -f "$path" ]; then ok "$msg"; else bad "$msg (missing $path)"; fi
}

assert_absent() {
  local path="$1" msg="$2"
  if [ ! -e "$path" ]; then ok "$msg"; else bad "$msg (still present $path)"; fi
}

assert_contains() {
  local path="$1" needle="$2" msg="$3"
  if grep -aF -q "$needle" "$path"; then ok "$msg"; else bad "$msg"; fi
}

assert_eq() {
  local got="$1" want="$2" msg="$3"
  if [ "$got" = "$want" ]; then ok "$msg"; else bad "$msg (got '$got' want '$want')"; fi
}

assert_exit() {
  local got="$1" want="$2" msg="$3"
  if [ "$got" = "$want" ]; then ok "$msg"; else bad "$msg (exit $got want $want)"; fi
}

make_so() {
  local path="$1" kind="$2"
  mkdir -p "$(dirname "$path")"
  case "$kind" in
    pre18)
      printf 'Metamod:Source 2.0.0-dev+1411\n    SourceHook version: %%d:%%d\n' >"$path"
      ;;
    plapi18)
      printf 'Metamod:Source pin 7e24ce9e7a03 PLAPI 18\nGetDetourInterface\nKHook\n' >"$path"
      ;;
    stripped18)
      # No symbol names — identity sidecar is the skip gate.
      printf 'stripped-metamod-bytes-no-symbols\n' >"$path"
      ;;
    *)
      echo "unknown so kind $kind" >&2
      return 1
      ;;
  esac
}

make_tree() {
  local dest="$1" kind="$2"
  rm -rf "$dest"
  mkdir -p "$dest/bin/linuxsteamrt64"
  make_so "$dest/bin/linuxsteamrt64/metamod.2.cs2.so" "$kind"
  printf '"Metamod Plugin"\n{\n\t"alias"\t"s2script"\n}\n' >"$dest/s2script.vdf"
  echo "operator-marker" >"$dest/KEEP_ME"
}

write_identity() {
  local dest="$1" source="$2" artifact="$3"
  local so="$dest/bin/linuxsteamrt64/metamod.2.cs2.so"
  local sha
  sha="$(sha256sum "$so" | awk '{print $1}')"
  cat >"$dest/.s2script-metamod-identity" <<EOF
pin=7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33
plapi=18
source=${source}
artifact=${artifact}
sha256=${sha}
EOF
}

make_tarball() {
  local tarpath="$1" kind="$2"
  local work
  work="$(mktemp -d)"
  mkdir -p "$work/addons/metamod/bin/linuxsteamrt64"
  make_so "$work/addons/metamod/bin/linuxsteamrt64/metamod.2.cs2.so" "$kind"
  echo "drop-$kind" >"$work/addons/metamod/README.txt"
  tar -C "$work" -czf "$tarpath" addons
  rm -rf "$work"
}

# curl stub: last http(s) arg is the URL; honours -o FILE.
# S2_FAKE_LATEST / S2_FAKE_TARBALL / S2_CURL_FAIL control behaviour.
write_curl_stub() {
  local path="$1"
  cat >"$path" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [ "${S2_CURL_FAIL:-}" = "1" ]; then
  echo "curl stub: forced failure" >&2
  exit 1
fi
url=""
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$arg"
  fi
  case "$arg" in
    http://*|https://*) url="$arg" ;;
  esac
  prev="$arg"
done
if [ -z "$url" ]; then
  echo "curl stub: no URL" >&2
  exit 1
fi
if [[ "$url" == *mmsource-latest-linux ]]; then
  if [ "${S2_CURL_FAIL_LATEST:-}" = "1" ]; then
    exit 1
  fi
  printf '%s\n' "${S2_FAKE_LATEST:?S2_FAKE_LATEST unset}"
  echo curl-latest >>"${S2_CURL_LOG:?}"
  exit 0
fi
echo curl-tarball >>"${S2_CURL_LOG:?}"
if [ "${S2_CURL_FAIL_TARBALL:-}" = "1" ]; then
  exit 1
fi
if [ -z "$out" ]; then
  echo "curl stub: tarball fetch missing -o" >&2
  exit 1
fi
cp "${S2_FAKE_TARBALL:?S2_FAKE_TARBALL unset}" "$out"
STUB
  chmod +x "$path"
}

run_ensure() {
  local dest="$1"
  S2_SCRIPT_REPO="$ROOT" \
  S2_METAMOD_DEST="$dest" \
  S2_METAMOD_CS2_RUNNING="${S2_METAMOD_CS2_RUNNING:-0}" \
  S2_METAMOD_CURL="$STUB_CURL" \
  S2_METAMOD_PINNED_TREE="${S2_METAMOD_PINNED_TREE:-}" \
  S2_CURL_LOG="$CURL_LOG" \
  S2_FAKE_LATEST="${S2_FAKE_LATEST:-}" \
  S2_FAKE_TARBALL="${S2_FAKE_TARBALL:-}" \
  S2_CURL_FAIL="${S2_CURL_FAIL:-}" \
  S2_CURL_FAIL_LATEST="${S2_CURL_FAIL_LATEST:-}" \
  S2_CURL_FAIL_TARBALL="${S2_CURL_FAIL_TARBALL:-}" \
    bash "$INSTALL" --metamod-only
}

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/s2-t7-metamod.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT
STUB_CURL="$WORKDIR/curl-stub"
CURL_LOG="$WORKDIR/curl.log"
: >"$CURL_LOG"
write_curl_stub "$STUB_CURL"

PLAPI18_TAR="$WORKDIR/mmsource-2.0.0-git7e24ce9-linux.tar.gz"
PRE18_TAR="$WORKDIR/mmsource-2.0.0-git26c03fa-linux.tar.gz"
make_tarball "$PLAPI18_TAR" plapi18
make_tarball "$PRE18_TAR" pre18

PIN_TREE="$WORKDIR/pin-tree"
make_tree "$PIN_TREE" plapi18
# Pin tree is a verified PLAPI 18 artifact (heuristic pass); identity is written after swap.

echo "== fixture: pre-18 tree is replaced by a verified PLAPI 18 drop"
DEST="$WORKDIR/case-pre18/metamod"
make_tree "$DEST" pre18
echo "pre18-original" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git7e24ce9-linux.tar.gz"
S2_FAKE_TARBALL="$PLAPI18_TAR"
unset S2_METAMOD_PINNED_TREE || true
if run_ensure "$DEST"; then
  assert_exit 0 0 "pre-18 refresh exits 0"
else
  assert_exit "$?" 0 "pre-18 refresh exits 0"
fi
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "GetDetourInterface" "pre-18 dest now has GetDetourInterface"
assert_file "$DEST/.s2script-metamod-identity" "pre-18 dest wrote identity sidecar"
assert_contains "$DEST/.s2script-metamod-identity" "plapi=18" "identity records plapi=18"
assert_contains "$DEST/.s2script-metamod-identity" "pin=7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33" "identity records tested pin"
assert_contains "$DEST/.s2script-metamod-identity" "source=drop" "identity source=drop"
assert_file "$DEST/s2script.vdf" "s2script.vdf kept after drop swap"
assert_file "$DEST.prev/KEEP_ME" "pre-18 previous tree preserved as dest.prev"
assert_contains "$DEST.prev/bin/linuxsteamrt64/metamod.2.cs2.so" "SourceHook version" "preserved tree is the pre-18 binary"
assert_absent "$DEST/KEEP_ME" "operator marker from pre-18 tree is not in the new dest (lives in .prev)"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "2" "pre-18 path fetched latest pointer + one tarball (no loop)"

echo "== fixture: current/verified tree is left alone (no download)"
DEST="$WORKDIR/case-current/metamod"
make_tree "$DEST" stripped18
write_identity "$DEST" pin "pin:7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33"
echo "verified-marker" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="should-not-be-fetched.tar.gz"
S2_FAKE_TARBALL="$PLAPI18_TAR"
before_sha="$(sha256sum "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" | awk '{print $1}')"
if run_ensure "$DEST"; then
  assert_exit 0 0 "verified skip exits 0"
else
  assert_exit "$?" 0 "verified skip exits 0"
fi
after_sha="$(sha256sum "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" | awk '{print $1}')"
assert_eq "$after_sha" "$before_sha" "verified .so bytes unchanged"
assert_contains "$DEST/KEEP_ME" "verified-marker" "verified tree not replaced"
assert_absent "$DEST.prev" "verified skip does not snapshot a .prev"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "0" "verified skip does not call curl"
assert_file "$DEST/s2script.vdf" "verified skip keeps s2script.vdf"

echo "== fixture: absent tree is installed from a verified drop"
DEST="$WORKDIR/case-absent/metamod"
rm -rf "$DEST" "$DEST.prev"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git7e24ce9-linux.tar.gz"
S2_FAKE_TARBALL="$PLAPI18_TAR"
if run_ensure "$DEST"; then
  assert_exit 0 0 "absent install exits 0"
else
  assert_exit "$?" 0 "absent install exits 0"
fi
assert_file "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "absent dest received metamod.2.cs2.so"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "GetDetourInterface" "absent dest is PLAPI 18 heuristic-pass"
assert_file "$DEST/.s2script-metamod-identity" "absent dest wrote identity"
assert_file "$DEST/s2script.vdf" "absent dest restored docker/s2script.vdf from the repo"
assert_contains "$DEST/s2script.vdf" "s2script" "restored VDF names s2script"

echo "== fixture: failed download preserves the previous installation"
DEST="$WORKDIR/case-fail/metamod"
make_tree "$DEST" pre18
echo "must-survive" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git7e24ce9-linux.tar.gz"
S2_FAKE_TARBALL="$PLAPI18_TAR"
S2_CURL_FAIL=1
unset S2_METAMOD_PINNED_TREE || true
set +e
run_ensure "$DEST"
fail_rc=$?
set -e
unset S2_CURL_FAIL
assert_exit "$fail_rc" 1 "failed download exits 1"
assert_contains "$DEST/KEEP_ME" "must-survive" "failed download left KEEP_ME in place"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "SourceHook version" "failed download left the pre-18 .so"
assert_absent "$DEST/.s2script-metamod-identity" "failed download did not write a verified identity"
assert_file "$DEST/s2script.vdf" "failed download kept s2script.vdf"

echo "== fixture: stale (pre-18) drop is not retried; pin fallback succeeds"
DEST="$WORKDIR/case-stale/metamod"
make_tree "$DEST" pre18
echo "old-drop" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git26c03fa-linux.tar.gz"
S2_FAKE_TARBALL="$PRE18_TAR"
export S2_METAMOD_PINNED_TREE="$PIN_TREE"
if run_ensure "$DEST"; then
  assert_exit 0 0 "stale drop + pin fallback exits 0"
else
  assert_exit "$?" 0 "stale drop + pin fallback exits 0"
fi
unset S2_METAMOD_PINNED_TREE
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "GetDetourInterface" "stale drop replaced via pin tree"
assert_contains "$DEST/.s2script-metamod-identity" "source=pin" "identity source=pin after fallback"
assert_file "$DEST.prev/KEEP_ME" "stale-drop path preserved previous tree"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "2" "stale drop fetched latest+tarball once (no loop)"

echo "== fixture: stale drop with no pin preserves previous"
DEST="$WORKDIR/case-stale-nopin/metamod"
make_tree "$DEST" pre18
echo "still-here" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git26c03fa-linux.tar.gz"
S2_FAKE_TARBALL="$PRE18_TAR"
unset S2_METAMOD_PINNED_TREE || true
set +e
run_ensure "$DEST"
stale_rc=$?
set -e
assert_exit "$stale_rc" 1 "stale drop without pin exits 1"
assert_contains "$DEST/KEEP_ME" "still-here" "stale drop without pin preserved dest"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "SourceHook version" "stale drop without pin left pre-18 .so"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "2" "stale drop without pin did not re-download"

echo "== fixture: stripped pin tree, no sidecar, stale drop → swap source=pin"
STRIPPED_PIN="$WORKDIR/pin-stripped"
make_tree "$STRIPPED_PIN" stripped18
# Operator pin: stripped .so, no identity sidecar. Heuristic is unknown, not fail.
assert_absent "$STRIPPED_PIN/.s2script-metamod-identity" "stripped pin has no sidecar"
DEST="$WORKDIR/case-stripped-pin/metamod"
make_tree "$DEST" pre18
echo "stale-dest" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git26c03fa-linux.tar.gz"
S2_FAKE_TARBALL="$PRE18_TAR"
export S2_METAMOD_PINNED_TREE="$STRIPPED_PIN"
if run_ensure "$DEST"; then
  assert_exit 0 0 "stripped pin fallback exits 0"
else
  assert_exit "$?" 0 "stripped pin fallback exits 0"
fi
unset S2_METAMOD_PINNED_TREE
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "stripped-metamod-bytes-no-symbols" "stripped pin .so installed"
assert_contains "$DEST/.s2script-metamod-identity" "source=pin" "stripped pin identity source=pin"
assert_contains "$DEST/.s2script-metamod-identity" "plapi=18" "stripped pin identity records plapi=18"
assert_file "$DEST.prev/KEEP_ME" "stripped pin path preserved previous tree"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "2" "stripped pin path fetched drop once (no loop)"

echo "== fixture: CS2 running refuses replace and preserves dest"
DEST="$WORKDIR/case-running/metamod"
make_tree "$DEST" pre18
echo "live-tree" >"$DEST/KEEP_ME"
: >"$CURL_LOG"
S2_FAKE_LATEST="mmsource-2.0.0-git7e24ce9-linux.tar.gz"
S2_FAKE_TARBALL="$PLAPI18_TAR"
S2_METAMOD_CS2_RUNNING=1
set +e
run_ensure "$DEST"
run_rc=$?
set -e
unset S2_METAMOD_CS2_RUNNING
assert_exit "$run_rc" 1 "running CS2 refresh exits 1"
assert_contains "$DEST/KEEP_ME" "live-tree" "running CS2 left dest in place"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "SourceHook version" "running CS2 did not swap the .so"

echo "== contract: live_gate propagates s2_ensure_metamod failure"
if grep -E 's2_ensure_metamod .+ \|\| return 1' "$INSTALL" >/dev/null; then
  ok "live_gate uses s2_ensure_metamod … || return 1"
else
  bad "live_gate does not propagate s2_ensure_metamod failure"
fi

# Source the installer (does not run main) and lock pin-origin accept/reject.
# shellcheck disable=SC1090
source "$INSTALL"
PIN_OK_DIR="$WORKDIR/pin-ok-unit"
make_tree "$PIN_OK_DIR" stripped18
if s2_metamod_stage_is_pin_ok "$PIN_OK_DIR"; then
  ok "s2_metamod_stage_is_pin_ok accepts stripped pin (heuristic unknown)"
else
  bad "s2_metamod_stage_is_pin_ok rejected stripped pin"
fi
if s2_metamod_stage_is_plapi18 "$PIN_OK_DIR"; then
  bad "drop verifier must still reject stripped tree without sidecar"
else
  ok "drop verifier still rejects stripped tree without sidecar"
fi
FAIL_PIN="$WORKDIR/pin-fail-unit"
make_tree "$FAIL_PIN" pre18
if s2_metamod_stage_is_pin_ok "$FAIL_PIN"; then
  bad "s2_metamod_stage_is_pin_ok must reject SourceHook/pre-18 pin"
else
  ok "s2_metamod_stage_is_pin_ok rejects SourceHook/pre-18 pin"
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "test-install-metamod: FAILED ($fail/$ran)" >&2
  exit 1
fi
echo "test-install-metamod: all passed ($ran)"
