#!/usr/bin/env bash
# Fail if the committed ctx-hook-augmentation codegen (packages/cs2/hooks.generated.d.ts) is out of
# date vs a fresh generation from games/cs2/gamedata/game.cs2.jsonc's `hooks` section.
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"
( cd packages/sdk && node build.mjs >/dev/null )
node packages/sdk/dist/cli.js gen-hooks --check
echo "PASS: hook codegen is up to date"
