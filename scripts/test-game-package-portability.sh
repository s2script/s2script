#!/usr/bin/env bash
# Prove one frozen core test executable rejects absent package artifacts and accepts
# the explicitly packaged synthetic Source 2 fixture.
set -euo pipefail
cd "$(dirname "$0")/.."

test_name='game_packages::tests::synthetic_package_selects_and_uses_an_ordinary_function'
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
artifact_root="$tmp/artifacts"
mkdir "$artifact_root"

command -v node >/dev/null || { echo 'FAIL: node is required' >&2; exit 1; }
node scripts/lib/check-game-package-deps.mjs
command -v cargo >/dev/null || { echo 'FAIL: cargo is required' >&2; exit 1; }
command -v python3 >/dev/null || { echo 'FAIL: python3 is required' >&2; exit 1; }

echo '== locate core test executable from Cargo artifact metadata =='
cargo metadata --locked --no-deps --format-version 1 > "$tmp/metadata.json"
cargo test --locked -p s2script-core --lib --no-run --message-format=json > "$tmp/artifacts.jsonl"
python3 - "$tmp/metadata.json" "$tmp/artifacts.jsonl" "$tmp/executable" <<'PY'
import json
import os
import sys

metadata, events, output = sys.argv[1:]
with open(metadata, encoding="utf-8") as file:
    packages = json.load(file)["packages"]
matches = [package for package in packages if package["name"] == "s2script-core"]
if len(matches) != 1:
    raise SystemExit(f"FAIL: expected one s2script-core package, found {len(matches)}")
package = matches[0]
libs = [target for target in package["targets"] if target["name"] == "s2script_core" and target["test"]]
if len(libs) != 1:
    raise SystemExit(f"FAIL: expected one s2script_core library target, found {len(libs)}")
executables = []
with open(events, encoding="utf-8") as file:
    for line in file:
        event = json.loads(line)
        if (event.get("reason") == "compiler-artifact"
                and event.get("package_id") == package["id"]
                and event.get("target", {}).get("name") == libs[0]["name"]
                and event.get("target", {}).get("kind") == libs[0]["kind"]
                and event.get("profile", {}).get("test") is True
                and event.get("executable")):
            executables.append(event["executable"])
if len(executables) != 1:
    raise SystemExit(f"FAIL: expected one core test executable artifact, found {len(executables)}")
executable = executables[0]
if not os.path.isfile(executable) or not os.access(executable, os.X_OK) or "\n" in executable:
    raise SystemExit(f"FAIL: missing or unusable core test executable: {executable!r}")
with open(output, "w", encoding="utf-8") as file:
    file.write(executable + "\n")
PY
IFS= read -r executable < "$tmp/executable"
digest() {
  python3 - "$executable" <<'PY'
import hashlib
import sys

with open(sys.argv[1], "rb") as file:
    digest = hashlib.sha256()
    for block in iter(lambda: file.read(1024 * 1024), b""):
        digest.update(block)
    print(digest.hexdigest())
PY
}
before=$(digest)
echo "frozen executable: $executable"
echo "SHA-256: $before"

echo '== absent runtime package artifacts: expected missing-package failure =='
if S2_TEST_GAME_PACKAGE_ROOT="$artifact_root" "$executable" "$test_name" --exact --nocapture > "$tmp/absent.log" 2>&1; then
  cat "$tmp/absent.log"
  echo 'FAIL: absent package artifacts unexpectedly passed' >&2
  exit 1
fi
cat "$tmp/absent.log"
if ! grep -Fq 'called `Result::unwrap()` on an `Err` value: "missing: []"' "$tmp/absent.log" \
    || ! grep -Fq "test $test_name ... FAILED" "$tmp/absent.log" \
    || ! grep -Eq '^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' "$tmp/absent.log"; then
  echo 'FAIL: absent artifacts did not produce the one-test missing-package failure' >&2
  exit 1
fi

echo '== package explicit synthetic fixture =='
node scripts/build-game-packages.mjs --out "$artifact_root" --test-source games/fixture-source2

echo '== present runtime package artifacts: expected one-test pass =='
if ! S2_TEST_GAME_PACKAGE_ROOT="$artifact_root" "$executable" "$test_name" --exact --nocapture > "$tmp/present.log" 2>&1; then
  cat "$tmp/present.log"
  echo 'FAIL: packaged synthetic fixture test failed' >&2
  exit 1
fi
cat "$tmp/present.log"
if ! grep -Fq "test $test_name ... ok" "$tmp/present.log" \
    || ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored;' "$tmp/present.log"; then
  echo 'FAIL: packaged fixture did not produce exactly one passing test' >&2
  exit 1
fi
after=$(digest)
if [[ "$before" != "$after" ]]; then
  echo "FAIL: test executable changed: $before -> $after" >&2
  exit 1
fi
echo "PASS: absent/present package portability, same executable SHA-256 $after"
