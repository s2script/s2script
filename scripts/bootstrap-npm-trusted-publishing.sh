#!/usr/bin/env bash
# One-time bootstrap for npm trusted publishing across all @s2script/* packages.
#
# npm requires each package to EXIST before a Trusted Publisher can be attached.
# This script:
#   1. Publishes any missing packages (classic publish — chicken-and-egg)
#   2. Runs `npm trust github` for EVERY public workspace package in a loop
#      (no clicking through 29 package settings pages)
#
# Usage:
#   scripts/bootstrap-npm-trusted-publishing.sh           # dry-run
#   scripts/bootstrap-npm-trusted-publishing.sh --apply   # do it (needs npm login + 2FA)
#
# Prerequisites for --apply:
#   - npm login (web 2FA / interactive — granular tokens that bypass 2FA won't work
#     for `npm trust`)
#   - npm >= 11.15.0  (npm install -g npm@latest)
#   - On the first `npm trust` 2FA prompt in the browser, enable
#     "skip 2FA for the next 5 minutes" so the rest of the loop is unattended
#
# Trusted publisher target (same for every package):
#   GabeHirakawa / s2script / changesets.yml / allow npm publish
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APPLY=0
if [[ "${1:-}" == "--apply" ]]; then
  APPLY=1
fi

OWNER_REPO="GabeHirakawa/s2script"
WORKFLOW="changesets.yml"

echo "Trusted Publisher target (applied to every @s2script/* package):"
echo "  Provider:   GitHub Actions"
echo "  Repo:       $OWNER_REPO"
echo "  Workflow:   $WORKFLOW"
echo "  Allowed:    npm publish"
echo

# --- collect workspace packages ---
packages=()
for dir in packages/*/; do
  pkg_json="${dir}package.json"
  [[ -f "$pkg_json" ]] || continue
  name=$(node -pe 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).name' "$pkg_json")
  private=$(node -pe 'String(!!JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).private)' "$pkg_json")
  if [[ "$private" == "true" ]]; then
    echo "skip private $name"
    continue
  fi
  packages+=("$dir|$name")
done

missing=()
existing=()
for entry in "${packages[@]}"; do
  dir="${entry%%|*}"
  name="${entry##*|}"
  if npm view "$name" version >/dev/null 2>&1; then
    ver=$(npm view "$name" version 2>/dev/null || true)
    existing+=("$name@$ver")
  else
    missing+=("$entry")
  fi
done

echo "Already on npm (${#existing[@]}):"
for e in "${existing[@]+"${existing[@]}"}"; do
  echo "  $e"
done
echo

if [[ ${#missing[@]} -gt 0 ]]; then
  echo "Missing on npm (${#missing[@]}) — need a one-time classic publish first:"
  for entry in "${missing[@]}"; do
    echo "  ${entry##*|}  (${entry%%|*})"
  done
  echo
fi

trust_cmd() {
  local name="$1"
  echo "npm trust github \"$name\" --repo \"$OWNER_REPO\" --file \"$WORKFLOW\" --allow-publish --yes"
}

if [[ "$APPLY" -eq 0 ]]; then
  echo "Dry-run only. Plan:"
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "  1. Publish missing packages:"
    for entry in "${missing[@]}"; do
      echo "       ( cd ${entry%%|*} && npm publish --access public )"
    done
  else
    echo "  1. (no missing packages to publish)"
  fi
  echo "  2. Configure Trusted Publisher on all ${#packages[@]} packages via CLI:"
  for entry in "${packages[@]}"; do
    echo "       $(trust_cmd "${entry##*|}")"
  done
  echo
  echo "Re-run with --apply after:"
  echo "  npm install -g npm@latest   # need >= 11.15 for \`npm trust\`"
  echo "  npm login                   # interactive 2FA (not a bypass-2FA token)"
  exit 0
fi

# --- apply ---
if ! command -v npm >/dev/null; then
  echo "error: npm not found" >&2
  exit 1
fi

npm_major=$(npm -v | cut -d. -f1)
npm_minor=$(npm -v | cut -d. -f2)
if [[ "$npm_major" -lt 11 ]] || { [[ "$npm_major" -eq 11 ]] && [[ "$npm_minor" -lt 15 ]]; }; then
  echo "error: npm $(npm -v) is too old for \`npm trust\` (need >= 11.15.0)" >&2
  echo "  npm install -g npm@latest" >&2
  exit 1
fi

if ! npm whoami >/dev/null 2>&1; then
  echo "error: not logged in to npm — run \`npm login\` first" >&2
  exit 1
fi

if ! npm trust --help >/dev/null 2>&1; then
  echo "error: this npm build has no \`npm trust\` command" >&2
  echo "  npm install -g npm@latest" >&2
  exit 1
fi

if [[ ${#missing[@]} -gt 0 ]]; then
  if printf '%s\n' "${missing[@]}" | grep -q '|@s2script/sdk$'; then
    if [[ ! -d node_modules ]]; then
      npm install --no-fund --no-audit
    fi
    ( cd packages/sdk && npm run build )
  fi
  for entry in "${missing[@]}"; do
    dir="${entry%%|*}"
    name="${entry##*|}"
    echo "=== publishing $name (bootstrap, no provenance) ==="
    ( cd "$dir" && npm publish --access public )
  done
  echo
fi

echo "=== configuring Trusted Publisher on ${#packages[@]} packages ==="
echo "Tip: on the FIRST 2FA browser prompt, enable 'skip 2FA for the next 5 minutes'."
echo

failed=0
for entry in "${packages[@]}"; do
  name="${entry##*|}"
  echo "--- $name ---"
  # If trust already exists, list+revoke then recreate (idempotent --force style)
  if npm trust list "$name" --json 2>/dev/null | node -e '
    let d=""; process.stdin.on("data",c=>d+=c); process.stdin.on("end",()=>{
      try {
        const j=JSON.parse(d||"[]");
        const arr=Array.isArray(j)?j:(j&&j.trusts)||(j&&j.configurations)||[];
        if (arr.length) process.exit(0);
      } catch {}
      process.exit(1);
    });
  '; then
    echo "  existing trust found — leaving in place (re-run with FORCE=1 to replace)"
    if [[ "${FORCE:-}" == "1" ]]; then
      ids=$(npm trust list "$name" --json 2>/dev/null | node -e '
        let d=""; process.stdin.on("data",c=>d+=c); process.stdin.on("end",()=>{
          try {
            const j=JSON.parse(d||"[]");
            const arr=Array.isArray(j)?j:(j.trusts||j.configurations||[]);
            for (const t of arr) {
              const id=t.id||t._id||t.trustId;
              if (id) console.log(id);
            }
          } catch {}
        });
      ')
      for id in $ids; do
        echo "  revoking $id"
        npm trust revoke "$name" --id="$id" --yes || true
      done
      npm trust github "$name" --repo "$OWNER_REPO" --file "$WORKFLOW" --allow-publish --yes \
        || { echo "  FAILED $name" >&2; failed=1; }
    fi
  else
    npm trust github "$name" --repo "$OWNER_REPO" --file "$WORKFLOW" --allow-publish --yes \
      || { echo "  FAILED $name" >&2; failed=1; }
  fi
  sleep 2
done

echo
if [[ "$failed" -ne 0 ]]; then
  echo "done with errors — re-run --apply (and FORCE=1 if a package already had a different trust)." >&2
  exit 1
fi
echo "done. All packages trusted for $OWNER_REPO / $WORKFLOW."
echo "Optional: on each package's Publishing access, disallow tokens (OIDC still works)."
