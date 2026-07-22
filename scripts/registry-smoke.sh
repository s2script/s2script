#!/usr/bin/env bash
# Registry smoke checklist (manual / local).
# This exercises the CLIENT side (`s2script deploy` / `add`), which lives here. The registry
# SERVICE it talks to lives in a separate repo.
# Prerequisites: check out that repo, bring its Postgres up (`npm run db:start`), set env,
# `npm run db:push`, `npm run dev`, and sign up a Better Auth user.
#
# 1) Create org @demo + deploy token in the UI (/account/orgs, /account/tokens)
# 2) export S2SCRIPT_REGISTRY_URL=http://localhost:5173
# 3) export S2SCRIPT_TOKEN=s2s_…
# 4) From a fixture plugin with publishes + api.d.ts:
#      npx s2script deploy ./path/to/plugin
# 5) From a consumer plugin dir:
#      npx s2script add @demo/your-plugin@^1
#      npx s2script build .
# 6) Deploy without types while publishes is set → must fail CLI + API
set -euo pipefail
echo "See comments in scripts/registry-smoke.sh for the interactive smoke flow."
echo "Automated unit coverage: packages/cli publish-gate tests + website tokens/semver vitest."
