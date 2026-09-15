#!/usr/bin/env bash
# Fixture tests for scripts/cloud/install.sh Metamod refresh + verify-metamod-artifact.py.
# Never touches the live host docker/metamod/ or compose/RCON.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALL="$ROOT/scripts/cloud/install.sh"
VERIFY="$ROOT/scripts/verify-metamod-artifact.py"
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

assert_reason() {
  local log="$1" reason="$2" msg="$3"
  if grep -aE -q "(^|[^A-Za-z0-9_])${reason}([^A-Za-z0-9_]|$)" "$log"; then
    ok "$msg"
  else
    bad "$msg (missing named reason '$reason' in $(tr '\n' ' ' <"$log"))"
  fi
}

EXPECTED_MMS="7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33"
EXPECTED_KHOOK="1e200e4cc8e0badcb7cf941525268d6977f6a4e6"

tree_digest() {
  local d="$1"
  python3 - "$d" <<'PY'
import hashlib, os, sys
root = sys.argv[1]
h = hashlib.sha256()
for dirpath, dirnames, filenames in os.walk(root):
    dirnames.sort()
    for name in sorted(filenames):
        path = os.path.join(dirpath, name)
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        h.update(rel.encode("utf-8"))
        h.update(b"\0")
        with open(path, "rb") as fh:
            h.update(fh.read())
        h.update(b"\0")
print(h.hexdigest())
PY
}

# Synthetic ELF64 LE x86_64 ET_DYN. Not a Metamod binary; verifier tests use it as a
# format fixture. Transaction tests that inject verification must not treat stub
# files as proof of a real host binary.
write_elf() {
  python3 - "$1" "${2:-x64}" "${3:-}" <<'PY'
import pathlib, struct, sys
path = pathlib.Path(sys.argv[1])
kind = sys.argv[2]
extra = sys.argv[3].encode("utf-8") if len(sys.argv) > 3 else b""
path.parent.mkdir(parents=True, exist_ok=True)

def pack(machine=62, elfclass=2, data=1, etype=3, extra=b""):
    e_phoff = 64
    ident = bytes([0x7F, 0x45, 0x4C, 0x46, elfclass, data, 1, 0] + [0] * 8)
    hdr = ident + struct.pack(
        "<HHIQQQIHHHHHH",
        etype, machine, 1, 0, e_phoff, 0, 0, 64, 56, 1, 64, 0, 0,
    )
    ph = struct.pack("<IIQQQQQQ", 1, 5, 0, 0, 0, 0x1000, 0x1000, 0x1000)
    return hdr + ph + extra

if kind == "x64":
    blob = pack(extra=extra or b"elf-x64-fixture")
elif kind == "aarch64":
    blob = pack(machine=183, extra=b"elf-arm")
elif kind == "elf32":
    blob = pack(elfclass=1, machine=3, extra=b"elf32")
elif kind == "be":
    blob = pack(data=2, extra=b"elf-be")
elif kind == "exec":
    blob = pack(etype=2, extra=b"elf-exec")
elif kind == "truncated":
    blob = pack()[:16]
elif kind == "header-trunc":
    blob = pack()[:40]
elif kind == "glibc_new":
    blob = pack(extra=b"GLIBC_2.34\0")
else:
    raise SystemExit(f"unknown elf kind {kind}")
path.write_bytes(blob)
PY
}

write_manifest() {
  local tree="$1" out="$2"
  python3 - "$tree" "$out" "$EXPECTED_MMS" "$EXPECTED_KHOOK" <<'PY'
import hashlib, json, os, sys
tree, out, mms, khook = sys.argv[1:]
required = [
    "bin/linuxsteamrt64/metamod.2.cs2.so",
    "bin/linuxsteamrt64/libserver.so",
]
optional = ["metaplugins.ini", "README.txt"]
items = []
for rel in required + optional:
    path = os.path.join(tree, rel)
    if not os.path.isfile(path):
        if rel in required:
            continue
        continue
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        h.update(fh.read())
    items.append({"path": rel, "sha256": h.hexdigest()})
doc = {
    "schema": 2,
    "plapi": 18,
    "provenance": {"kind": "unmodified-source", "metamod_commit": mms, "khook_commit": khook},
    "target": "linux-x86_64",
    "glibc_max": "2.31",
    "artifacts": items,
}
os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
with open(out, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, indent=2)
    fh.write("\n")
PY
}

make_valid_tree() {
  local dest="$1"
  rm -rf "$dest"
  mkdir -p "$dest/bin/linuxsteamrt64"
  write_elf "$dest/bin/linuxsteamrt64/metamod.2.cs2.so" x64 "metamod-fixture"
  write_elf "$dest/bin/linuxsteamrt64/libserver.so" x64 "libserver-fixture"
  printf 'metaplugins\n' >"$dest/metaplugins.ini"
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
      printf 'stripped-metamod-bytes-no-symbols\n' >"$path"
      ;;
    zero)
      : >"$path"
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

write_curl_stub() {
  local path="$1"
  cat >"$path" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf 'curl-called %s\n' "$*" >>"${S2_CURL_LOG:?}"
echo "curl stub: network is disabled in transaction tests" >&2
exit 1
STUB
  chmod +x "$path"
}

run_ensure() {
  local dest="$1"
  S2_SCRIPT_REPO="$ROOT" \
  S2_METAMOD_DEST="$dest" \
  S2_METAMOD_CS2_RUNNING="${S2_METAMOD_CS2_RUNNING:-0}" \
  S2_METAMOD_CURL="$STUB_CURL" \
  S2_METAMOD_TREE="${S2_METAMOD_TREE:-}" \
  S2_METAMOD_BUILD_MANIFEST="${S2_METAMOD_BUILD_MANIFEST:-}" \
  S2_METAMOD_VERIFY="${S2_METAMOD_VERIFY:-}" \
  S2_METAMOD_INJECT_FAIL="${S2_METAMOD_INJECT_FAIL:-}" \
  S2_CURL_LOG="$CURL_LOG" \
    bash "$INSTALL" --metamod-only
}

run_verify() {
  python3 "$VERIFY" --tree "$1" --manifest "$2"
}

expect_verify_fail() {
  local tree="$1" manifest="$2" reason="$3" msg="$4"
  local out err rc
  out="$(mktemp)"
  err="$(mktemp)"
  set +e
  run_verify "$tree" "$manifest" >"$out" 2>"$err"
  rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    bad "$msg (verifier exited 0)"
  else
    assert_exit "$rc" "$rc" "$msg (nonzero exit $rc)"
    assert_reason "$err" "$reason" "$msg named reason $reason"
  fi
  rm -f "$out" "$err"
}

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/s2-t7-metamod.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT
STUB_CURL="$WORKDIR/curl-stub"
CURL_LOG="$WORKDIR/curl.log"
: >"$CURL_LOG"
write_curl_stub "$STUB_CURL"

PINNED="$WORKDIR/valid-pin"
make_valid_tree "$PINNED"
MANIFEST="$WORKDIR/valid-manifest.json"
write_manifest "$PINNED" "$MANIFEST"

# ---------------------------------------------------------------------------
# Verifier tests (real ELF fixtures). These do not go through install.sh.
# ---------------------------------------------------------------------------
echo "== verifier: stock source-build identity is accepted"
set +e
run_verify "$PINNED" "$MANIFEST" >"$WORKDIR/v-ok.out" 2>"$WORKDIR/v-ok.err"
v_rc=$?
set -e
assert_exit "$v_rc" 0 "valid tree + independent manifest exits 0"
if [ -s "$WORKDIR/v-ok.err" ] && grep -qiE 'error:' "$WORKDIR/v-ok.err"; then
  bad "valid verify wrote an error: $(cat "$WORKDIR/v-ok.err")"
else
  ok "valid verify has no error: line"
fi

echo "== verifier: missing / empty / malformed / stale / wrong manifest"
expect_verify_fail "$PINNED" "$WORKDIR/no-such-manifest.json" "missing_manifest" "missing manifest"
: >"$WORKDIR/empty.json"
expect_verify_fail "$PINNED" "$WORKDIR/empty.json" "empty_manifest" "empty manifest"
printf '{not json\n' >"$WORKDIR/bad.json"
expect_verify_fail "$PINNED" "$WORKDIR/bad.json" "malformed_manifest" "malformed manifest"
python3 - "$MANIFEST" "$WORKDIR/stale-patch.json" <<'PY'
import json, sys
from pathlib import Path
doc = json.loads(Path(sys.argv[1]).read_text())
doc["patchset_sha256"] = "0" * 64
Path(sys.argv[2]).write_text(json.dumps(doc) + "\n")
PY
expect_verify_fail "$PINNED" "$WORKDIR/stale-patch.json" "malformed_manifest" "obsolete patched manifest"
python3 - "$MANIFEST" "$WORKDIR/wrong-mms.json" <<'PY'
import json, sys
from pathlib import Path
doc = json.loads(Path(sys.argv[1]).read_text())
doc["provenance"]["metamod_commit"] = "deadbeef" + "0" * 32
Path(sys.argv[2]).write_text(json.dumps(doc) + "\n")
PY
expect_verify_fail "$PINNED" "$WORKDIR/wrong-mms.json" "unexpected_metamod_commit" "wrong Metamod commit"
python3 - "$MANIFEST" "$WORKDIR/wrong-khook.json" <<'PY'
import json, sys
from pathlib import Path
doc = json.loads(Path(sys.argv[1]).read_text())
doc["provenance"]["khook_commit"] = "cafebabe" + "0" * 32
Path(sys.argv[2]).write_text(json.dumps(doc) + "\n")
PY
expect_verify_fail "$PINNED" "$WORKDIR/wrong-khook.json" "unexpected_khook_commit" "wrong KHook commit"
python3 - "$MANIFEST" "$WORKDIR/wrong-plapi.json" <<'PY'
import json, sys
from pathlib import Path
doc = json.loads(Path(sys.argv[1]).read_text())
doc["plapi"] = 17
Path(sys.argv[2]).write_text(json.dumps(doc) + "\n")
PY
expect_verify_fail "$PINNED" "$WORKDIR/wrong-plapi.json" "unexpected_plapi" "wrong PLAPI"
python3 - "$MANIFEST" "$WORKDIR/modified-hash.json" <<'PY'
import json, sys
from pathlib import Path
doc = json.loads(Path(sys.argv[1]).read_text())
doc["artifacts"][0]["sha256"] = "ab" * 32
Path(sys.argv[2]).write_text(json.dumps(doc) + "\n")
PY
expect_verify_fail "$PINNED" "$WORKDIR/modified-hash.json" "hash_mismatch" "modified artifact hash"

echo "== verifier: zero-byte, GetDetourInterface text, truncated, wrong arch, incomplete"
ZERO="$WORKDIR/v-zero"
make_valid_tree "$ZERO"
: >"$ZERO/bin/linuxsteamrt64/metamod.2.cs2.so"
write_manifest "$ZERO" "$WORKDIR/v-zero.json"
expect_verify_fail "$ZERO" "$WORKDIR/v-zero.json" "empty_artifact" "zero-byte .so"

TEXT="$WORKDIR/v-text"
make_valid_tree "$TEXT"
printf 'GetDetourInterface\nKHook pin text\n' >"$TEXT/bin/linuxsteamrt64/metamod.2.cs2.so"
write_manifest "$TEXT" "$WORKDIR/v-text.json"
expect_verify_fail "$TEXT" "$WORKDIR/v-text.json" "not_elf" "text file containing GetDetourInterface"

TRUNC="$WORKDIR/v-trunc"
make_valid_tree "$TRUNC"
write_elf "$TRUNC/bin/linuxsteamrt64/metamod.2.cs2.so" truncated
write_manifest "$TRUNC" "$WORKDIR/v-trunc.json"
expect_verify_fail "$TRUNC" "$WORKDIR/v-trunc.json" "truncated_elf" "truncated ELF"

ARM="$WORKDIR/v-arm"
make_valid_tree "$ARM"
write_elf "$ARM/bin/linuxsteamrt64/metamod.2.cs2.so" aarch64
write_manifest "$ARM" "$WORKDIR/v-arm.json"
expect_verify_fail "$ARM" "$WORKDIR/v-arm.json" "wrong_architecture" "wrong architecture ELF"

INCOMPLETE="$WORKDIR/v-incomplete"
make_valid_tree "$INCOMPLETE"
rm -f "$INCOMPLETE/bin/linuxsteamrt64/libserver.so"
write_manifest "$INCOMPLETE" "$WORKDIR/v-incomplete.json"
expect_verify_fail "$INCOMPLETE" "$WORKDIR/v-incomplete.json" "required_loader_missing" "incomplete loader tree"

ESCAPE="$WORKDIR/v-escape.json"
python3 - "$MANIFEST" "$ESCAPE" <<'PY'
import json, sys
from pathlib import Path
doc = json.loads(Path(sys.argv[1]).read_text())
doc["artifacts"].append({"path": "../outside.so", "sha256": "aa" * 32})
Path(sys.argv[2]).write_text(json.dumps(doc) + "\n")
PY
expect_verify_fail "$PINNED" "$ESCAPE" "path_escape" "artifact path escapes the tree"

WRONG_SRC="$WORKDIR/v-wrong-src"
make_valid_tree "$WRONG_SRC"
# Real host-linked ELF (not the pinned Metamod). Independent manifest still names
# the expected pin but hashes a different binary.
printf 'void foo(void){puts("x");}\n' >"$WORKDIR/tiny.c"
gcc -shared -fPIC -o "$WRONG_SRC/bin/linuxsteamrt64/metamod.2.cs2.so" -x c "$WORKDIR/tiny.c" -include stdio.h
write_manifest "$PINNED" "$WORKDIR/v-wrong-src.json"
# Force the manifest hashes to the valid-pin files while the tree is gcc's ELF.
expect_verify_fail "$WRONG_SRC" "$WORKDIR/v-wrong-src.json" "hash_mismatch" "real wrong-source ELF"

GLIBC="$WORKDIR/v-glibc"
make_valid_tree "$GLIBC"
write_elf "$GLIBC/bin/linuxsteamrt64/metamod.2.cs2.so" glibc_new
write_manifest "$GLIBC" "$WORKDIR/v-glibc.json"
expect_verify_fail "$GLIBC" "$WORKDIR/v-glibc.json" "glibc_too_new" "GLIBC requirement above 2.31"

echo "== verifier: ordinary verification cannot invent or modify evidence"
verify_tree_before="$(tree_digest "$PINNED")"
verify_manifest_before="$(sha256sum "$MANIFEST" | awk '{print $1}')"
if run_verify "$PINNED" "$MANIFEST"; then
  ok "ordinary verification accepts the independent manifest"
else
  bad "ordinary verification rejected the independent manifest"
fi
assert_eq "$(tree_digest "$PINNED")" "$verify_tree_before" "ordinary verification leaves candidate tree unchanged"
assert_eq "$(sha256sum "$MANIFEST" | awk '{print $1}')" "$verify_manifest_before" "ordinary verification leaves input manifest unchanged"
missing_verify_manifest="$WORKDIR/never-created-by-verify.json"
if run_verify "$PINNED" "$missing_verify_manifest" >"$WORKDIR/readonly.out" 2>"$WORKDIR/readonly.err"; then
  bad "ordinary verification must require an existing independent manifest"
else
  assert_reason "$WORKDIR/readonly.err" "missing_manifest" "ordinary verification refuses missing identity"
fi
assert_absent "$missing_verify_manifest" "ordinary verification never creates a missing manifest"
assert_eq "$(tree_digest "$PINNED")" "$verify_tree_before" "missing-manifest refusal leaves candidate bytes unchanged"

# ---------------------------------------------------------------------------
# Installer transaction tests. Stub trees use explicit injected verification
# and must not be claimed as binary validation.
# ---------------------------------------------------------------------------
echo "== transaction: F3 zero-byte pin is rejected; dest unchanged"
DEST="$WORKDIR/case-zero/metamod"
make_tree "$DEST" pre18
echo "must-keep-zero" >"$DEST/KEEP_ME"
before="$(tree_digest "$DEST")"
ZERO_PIN="$WORKDIR/zero-pin"
rm -rf "$ZERO_PIN"
mkdir -p "$ZERO_PIN/bin/linuxsteamrt64"
: >"$ZERO_PIN/bin/linuxsteamrt64/metamod.2.cs2.so"
write_elf "$ZERO_PIN/bin/linuxsteamrt64/libserver.so" x64 "libserver-zero-tree"
write_manifest "$ZERO_PIN" "$WORKDIR/zero-pin.json"
: >"$CURL_LOG"
export S2_METAMOD_TREE="$ZERO_PIN"
export S2_METAMOD_BUILD_MANIFEST="$WORKDIR/zero-pin.json"
unset S2_METAMOD_VERIFY || true
unset S2_METAMOD_INJECT_FAIL || true
set +e
run_ensure "$DEST" >"$WORKDIR/zero-install.out" 2>"$WORKDIR/zero-install.err"
zero_rc=$?
set -e
assert_exit "$zero_rc" 1 "zero-byte candidate exits 1"
assert_eq "$(tree_digest "$DEST")" "$before" "zero-byte rejection leaves dest byte-for-byte"
assert_contains "$DEST/KEEP_ME" "must-keep-zero" "zero-byte rejection kept KEEP_ME"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "SourceHook version" "zero-byte rejection left the previous .so"
assert_absent "$DEST/.s2script-metamod-identity" "zero-byte rejection did not write a verified identity"
assert_reason "$WORKDIR/zero-install.err" "empty_artifact" "zero-byte install reports empty_artifact"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "0" "zero-byte path does not select mmsdrop"

echo "== transaction: GetDetourInterface text is rejected; dest unchanged"
DEST="$WORKDIR/case-text/metamod"
make_tree "$DEST" pre18
echo "text-keep" >"$DEST/KEEP_ME"
before="$(tree_digest "$DEST")"
TEXT_PIN="$WORKDIR/text-pin"
make_valid_tree "$TEXT_PIN"
printf 'GetDetourInterface\n' >"$TEXT_PIN/bin/linuxsteamrt64/metamod.2.cs2.so"
write_manifest "$TEXT_PIN" "$WORKDIR/text-pin.json"
export S2_METAMOD_TREE="$TEXT_PIN"
export S2_METAMOD_BUILD_MANIFEST="$WORKDIR/text-pin.json"
set +e
run_ensure "$DEST" >"$WORKDIR/text-install.out" 2>"$WORKDIR/text-install.err"
text_rc=$?
set -e
assert_exit "$text_rc" 1 "GetDetourInterface text candidate exits 1"
assert_eq "$(tree_digest "$DEST")" "$before" "text candidate rejection leaves dest byte-for-byte"
assert_contains "$DEST/KEEP_ME" "text-keep" "text rejection kept KEEP_ME"

echo "== transaction: pin without independent manifest is refused"
DEST="$WORKDIR/case-nomanifest/metamod"
make_tree "$DEST" pre18
echo "nomanifest-keep" >"$DEST/KEEP_ME"
before="$(tree_digest "$DEST")"
export S2_METAMOD_TREE="$PINNED"
unset S2_METAMOD_BUILD_MANIFEST || true
set +e
run_ensure "$DEST" >"$WORKDIR/noman.out" 2>"$WORKDIR/noman.err"
nm_rc=$?
set -e
assert_exit "$nm_rc" 1 "pinned tree without manifest exits 1"
assert_eq "$(tree_digest "$DEST")" "$before" "missing-manifest rejection leaves dest byte-for-byte"
assert_reason "$WORKDIR/noman.err" "missing_manifest" "missing independent manifest is named"

echo "== transaction: verified ELF install, identity vs build-manifest distinction, repeat skip"
DEST="$WORKDIR/case-install/metamod"
make_tree "$DEST" pre18
echo "old-keep" >"$DEST/KEEP_ME"
export S2_METAMOD_TREE="$PINNED"
export S2_METAMOD_BUILD_MANIFEST="$MANIFEST"
unset S2_METAMOD_VERIFY || true
: >"$CURL_LOG"
if run_ensure "$DEST" >"$WORKDIR/install.out" 2>"$WORKDIR/install.err"; then
  assert_exit 0 0 "verified ELF install exits 0"
else
  assert_exit "$?" 0 "verified ELF install exits 0"
fi
assert_file "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "install wrote metamod.2.cs2.so"
assert_file "$DEST/bin/linuxsteamrt64/libserver.so" "install wrote required libserver.so"
assert_file "$DEST/.s2script-metamod-identity" "install wrote installation receipt"
assert_file "$DEST/.s2script-metamod-build.json" "install copied the independent build manifest"
assert_file "$DEST/s2script.vdf" "s2script.vdf present after swap"
assert_file "$DEST.prev/KEEP_ME" "previous tree preserved as dest.prev"
assert_contains "$DEST.prev/KEEP_ME" "old-keep" "preserved previous operator marker"
# Receipt is separate from the source/release artifact manifest.
if python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$DEST/.s2script-metamod-identity" 2>/dev/null; then
  bad "installation receipt must not be the JSON build manifest"
else
  ok "installation receipt is not the JSON build manifest"
fi
python3 - "$DEST/.s2script-metamod-build.json" "$MANIFEST" <<'PY'
import json, sys
from pathlib import Path
a = json.loads(Path(sys.argv[1]).read_text())
b = json.loads(Path(sys.argv[2]).read_text())
raise SystemExit(0 if a == b else 1)
PY
assert_exit "$?" 0 "copied build manifest matches the independent input"
assert_contains "$DEST/.s2script-metamod-identity" "plapi=18" "receipt records plapi=18"
assert_contains "$DEST/.s2script-metamod-identity" "provenance=unmodified-source" "receipt records unmodified-source provenance"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "0" "verified install does not select mmsdrop"

echo "== transaction: repeat install skips and leaves dest unchanged"
before="$(tree_digest "$DEST")"
: >"$CURL_LOG"
if run_ensure "$DEST" >"$WORKDIR/skip.out" 2>"$WORKDIR/skip.err"; then
  assert_exit 0 0 "repeat verified install exits 0"
else
  assert_exit "$?" 0 "repeat verified install exits 0"
fi
assert_eq "$(tree_digest "$DEST")" "$before" "skip path leaves dest byte-for-byte"
assert_absent "$DEST.prev.prev" "skip does not stack another prev generation beyond dest.prev from first swap"
if grep -qi 'skip' "$WORKDIR/skip.out" "$WORKDIR/skip.err"; then
  ok "skip path reports a skip"
else
  bad "skip path did not report skip"
fi
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "0" "skip path does not call curl"
# Skip rechecks artifacts, not only the sidecar: corrupt .so must not skip.
echo "== transaction: skip rechecks artifact bytes, not only sidecar"
cp -a "$DEST/.s2script-metamod-identity" "$WORKDIR/saved-identity"
printf '\x00' >>"$DEST/bin/linuxsteamrt64/metamod.2.cs2.so"
set +e
run_ensure "$DEST" >"$WORKDIR/skip-corrupt.out" 2>"$WORKDIR/skip-corrupt.err"
# Source is still the good PINNED tree, so this should refresh rather than skip,
# OR fail closed if CS2... CS2 is not running. Should replace from good source.
corrupt_rc=$?
set -e
assert_exit "$corrupt_rc" 0 "corrupt dest is refreshed from the independent source"
python3 - "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "$PINNED/bin/linuxsteamrt64/metamod.2.cs2.so" <<'PY'
import hashlib, sys
from pathlib import Path
def h(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()
raise SystemExit(0 if h(sys.argv[1]) == h(sys.argv[2]) else 1)
PY
assert_exit "$?" 0 "refreshed dest .so matches the independent source bytes"

echo "== transaction: injected verification (stub files; not a binary proof)"
STUB_PIN="$WORKDIR/stub-pin"
make_tree "$STUB_PIN" stripped18
mkdir -p "$STUB_PIN/bin/linuxsteamrt64"
printf 'stub-loader\n' >"$STUB_PIN/bin/linuxsteamrt64/libserver.so"
write_manifest "$STUB_PIN" "$WORKDIR/stub-pin.json"
DEST="$WORKDIR/case-inject-pass/metamod"
make_tree "$DEST" pre18
echo "inject-old" >"$DEST/KEEP_ME"
export S2_METAMOD_TREE="$STUB_PIN"
export S2_METAMOD_BUILD_MANIFEST="$WORKDIR/stub-pin.json"
export S2_METAMOD_VERIFY="inject-pass"
if run_ensure "$DEST"; then
  assert_exit 0 0 "inject-pass transaction exits 0 (not a binary proof)"
else
  assert_exit "$?" 0 "inject-pass transaction exits 0 (not a binary proof)"
fi
assert_file "$DEST/.s2script-metamod-identity" "inject-pass wrote receipt after success"
assert_file "$DEST/.s2script-metamod-build.json" "inject-pass copied independent manifest after success"
assert_file "$DEST.prev/KEEP_ME" "inject-pass preserved previous tree"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "stripped-metamod-bytes-no-symbols" "inject-pass installed stub bytes (transaction only)"

echo "== transaction: inject-fail verification preserves dest"
DEST="$WORKDIR/case-inject-fail/metamod"
make_tree "$DEST" pre18
echo "inj-fail-keep" >"$DEST/KEEP_ME"
before="$(tree_digest "$DEST")"
export S2_METAMOD_VERIFY="inject-fail"
set +e
run_ensure "$DEST" >"$WORKDIR/inj-fail.out" 2>"$WORKDIR/inj-fail.err"
inj_rc=$?
set -e
assert_exit "$inj_rc" 1 "inject-fail verification exits 1"
assert_eq "$(tree_digest "$DEST")" "$before" "inject-fail leaves dest byte-for-byte"
assert_contains "$DEST/KEEP_ME" "inj-fail-keep" "inject-fail kept KEEP_ME"
unset S2_METAMOD_VERIFY || true

echo "== transaction: injected rename/receipt/VDF failures (stub + inject-pass)"
export S2_METAMOD_VERIFY="inject-pass"
for point in receipt-write vdf-write before-rename-dest after-rename-dest before-rename-stage after-rename-stage; do
  DEST="$WORKDIR/case-inj-$point/metamod"
  make_tree "$DEST" pre18
  echo "keep-$point" >"$DEST/KEEP_ME"
  before="$(tree_digest "$DEST")"
  export S2_METAMOD_INJECT_FAIL="$point"
  set +e
  run_ensure "$DEST" >"$WORKDIR/inj-$point.out" 2>"$WORKDIR/inj-$point.err"
  rc=$?
  set -e
  assert_exit "$rc" 1 "inject $point exits 1"
  assert_eq "$(tree_digest "$DEST")" "$before" "inject $point leaves dest byte-for-byte"
  assert_contains "$DEST/KEEP_ME" "keep-$point" "inject $point kept previous KEEP_ME"
  assert_file "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "inject $point left a usable previous .so"
  assert_absent "$DEST.staging" "inject $point did not leave a staging tree as dest"
done
unset S2_METAMOD_INJECT_FAIL || true
unset S2_METAMOD_VERIFY || true

echo "== transaction: CS2 running refuses replace and preserves dest"
DEST="$WORKDIR/case-running/metamod"
make_tree "$DEST" pre18
echo "live-tree" >"$DEST/KEEP_ME"
before="$(tree_digest "$DEST")"
export S2_METAMOD_TREE="$PINNED"
export S2_METAMOD_BUILD_MANIFEST="$MANIFEST"
unset S2_METAMOD_VERIFY || true
S2_METAMOD_CS2_RUNNING=1
set +e
run_ensure "$DEST"
run_rc=$?
set -e
unset S2_METAMOD_CS2_RUNNING
assert_exit "$run_rc" 1 "running CS2 refresh exits 1"
assert_eq "$(tree_digest "$DEST")" "$before" "running CS2 left dest byte-for-byte"
assert_contains "$DEST/KEEP_ME" "live-tree" "running CS2 left dest in place"
assert_contains "$DEST/bin/linuxsteamrt64/metamod.2.cs2.so" "SourceHook version" "running CS2 did not swap the .so"

echo "== transaction: pinned official download failure preserves previous"
DEST="$WORKDIR/case-nosource/metamod"
make_tree "$DEST" pre18
echo "still-here" >"$DEST/KEEP_ME"
before="$(tree_digest "$DEST")"
: >"$CURL_LOG"
unset S2_METAMOD_TREE || true
unset S2_METAMOD_BUILD_MANIFEST || true
unset S2_METAMOD_VERIFY || true
set +e
run_ensure "$DEST" >"$WORKDIR/nosrc.out" 2>"$WORKDIR/nosrc.err"
stale_rc=$?
set -e
assert_exit "$stale_rc" 1 "failed default official download exits 1"
assert_eq "$(tree_digest "$DEST")" "$before" "no-source path preserved dest byte-for-byte"
assert_contains "$DEST/KEEP_ME" "still-here" "no-source preserved dest"
curl_calls="$(grep -c . "$CURL_LOG" || true)"
assert_eq "$curl_calls" "1" "fresh setup selects the immutable official default"
assert_contains "$CURL_LOG" "https://github.com/alliedmodders/metamod-source/releases/download/2.0.0.1467/mmsource-2.0.0-git1467-linux.tar.gz" "default uses exact official asset, never moving latest"

echo "== transaction: stock official archive needs no private build manifest"
RELEASE_ROOT="$WORKDIR/release-root"
mkdir -p "$RELEASE_ROOT/addons"
cp -a "$PINNED" "$RELEASE_ROOT/addons/metamod"
RELEASE_ARCHIVE="$WORKDIR/official-fixture-linux.tar.gz"
tar -czf "$RELEASE_ARCHIVE" -C "$RELEASE_ROOT" addons
export S2_METAMOD_RELEASE_ARCHIVE="$RELEASE_ARCHIVE"
export S2_METAMOD_RELEASE_URL="https://github.com/alliedmodders/metamod-source/releases/download/test/mmsource-fixture-linux.tar.gz"
export S2_METAMOD_RELEASE_SHA256="$(sha256sum "$RELEASE_ARCHIVE" | awk '{print $1}')"
export S2_METAMOD_RELEASE_PLAPI=18
DEST="$WORKDIR/case-official/metamod"
make_tree "$DEST" pre18
echo "stock-previous" >"$DEST/KEEP_ME"
if run_ensure "$DEST" >"$WORKDIR/official.out" 2>"$WORKDIR/official.err"; then
  ok "verified stock archive installs without a private source build"
else
  bad "stock archive install failed: $(cat "$WORKDIR/official.err")"
fi
assert_file "$DEST.prev/KEEP_ME" "official stock install preserves prior tree"
assert_contains "$DEST/.s2script-metamod-identity" "provenance=official-release" "receipt records official provenance"
before="$(tree_digest "$DEST")"
if run_ensure "$DEST" >"$WORKDIR/official-repeat.out" 2>"$WORKDIR/official-repeat.err"; then
  ok "repeat stock archive selection verifies and skips"
else
  bad "repeat stock archive selection failed"
fi
assert_eq "$(tree_digest "$DEST")" "$before" "repeat official install leaves bytes unchanged"
export S2_METAMOD_RELEASE_SHA256="$(printf '%064d' 0)"
if run_ensure "$DEST" >"$WORKDIR/official-bad.out" 2>"$WORKDIR/official-bad.err"; then
  bad "changed expected archive hash must invalidate the previous selection"
else
  assert_reason "$WORKDIR/official-bad.err" "archive_hash_mismatch" "wrong stock archive checksum rejected"
fi
assert_eq "$(tree_digest "$DEST")" "$before" "wrong archive checksum preserves installed stock host"
unset S2_METAMOD_RELEASE_PLAPI
if run_ensure "$DEST" >"$WORKDIR/official-api.out" 2>"$WORKDIR/official-api.err"; then
  bad "unconfirmed stock release PLAPI must not install"
else
  assert_reason "$WORKDIR/official-api.err" "missing_release_identity" "official PLAPI is never inferred from archive checksum"
fi
assert_eq "$(tree_digest "$DEST")" "$before" "missing PLAPI confirmation preserves stock host"
unset S2_METAMOD_RELEASE_ARCHIVE S2_METAMOD_RELEASE_URL S2_METAMOD_RELEASE_SHA256

echo "== contract: live_gate propagates s2_ensure_metamod failure"
if grep -E 's2_ensure_metamod .+ \|\| return 1' "$INSTALL" >/dev/null; then
  ok "live_gate uses s2_ensure_metamod … || return 1"
else
  bad "live_gate does not propagate s2_ensure_metamod failure"
fi
if grep -q 'GetDetourInterface' "$INSTALL"; then
  bad "installer still contains GetDetourInterface heuristic"
else
  ok "installer has no GetDetourInterface heuristic"
fi
if grep -q 's2_metamod_stage_is_pin_ok' "$INSTALL"; then
  bad "s2_metamod_stage_is_pin_ok heuristic still present"
else
  ok "s2_metamod_stage_is_pin_ok heuristic removed"
fi
if awk '/^s2_ensure_metamod\(/,/^}/ {print}' "$INSTALL" | grep -q 's2_stage_mmsdrop'; then
  bad "s2_ensure_metamod still selects mmsdrop"
else
  ok "s2_ensure_metamod does not select mmsdrop"
fi

# Source the installer (does not run main) and lock verify/reject helpers.
# shellcheck disable=SC1090
source "$INSTALL"
ZERO_UNIT="$WORKDIR/pin-zero-unit"
rm -rf "$ZERO_UNIT"
mkdir -p "$ZERO_UNIT/bin/linuxsteamrt64"
: >"$ZERO_UNIT/bin/linuxsteamrt64/metamod.2.cs2.so"
write_elf "$ZERO_UNIT/bin/linuxsteamrt64/libserver.so" x64 "z"
write_manifest "$ZERO_UNIT" "$WORKDIR/pin-zero-unit.json"
if s2_metamod_verify "$ZERO_UNIT" "$WORKDIR/pin-zero-unit.json"; then
  bad "s2_metamod_verify must reject zero-byte tree"
else
  ok "s2_metamod_verify rejects zero-byte tree"
fi
if s2_metamod_verify "$PINNED" "$MANIFEST"; then
  ok "s2_metamod_verify accepts the independent valid fixture"
else
  bad "s2_metamod_verify rejected the independent valid fixture"
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "test-install-metamod: FAILED ($fail/$ran)" >&2
  exit 1
fi
echo "test-install-metamod: all passed ($ran)"
