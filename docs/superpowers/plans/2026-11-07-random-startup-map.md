# Random startup map implementation plan

**Goal:** Switch the default startup map to a random usable map from `nextmap`'s maplist once per plugin load.

**Spec:** `docs/superpowers/specs/2026-11-07-random-startup-map-design.md`

- [x] Add VM-hosted plugin tests for delayed startup, both ends of random selection, workshop commands, no-map waiting, empty/invalid lists, current-map selection, cancellation, subsequent rotation, and admin overrides. Run them against the unchanged plugin and observe the missing startup switch.
- [x] Extend `plugins/disabled/nextmap/src/plugin.ts` to pick once in the existing first-map poll and reuse the delayed change function with an explicit candidate. Keep all normal map-end behavior.
- [x] Register the tests in `scripts/ci-js.sh`, run the JS gate and build the plugin, and review the final diff. Record actual gate limitations in the progress log.
Delivery: commit only this slice, push its branch, and open one PR for review; preserve the unrelated original checkout.

Validation: all 11 new tests failed against the unchanged plugin, then passed with the change. `bash scripts/ci-js.sh` passed in full (including 962 SDK tests, plugin/example typechecks, the new tests, and Docker compose/tooling checks). `npm run build --workspace @s2script/sdk` and `node packages/sdk/dist/cli.js build plugins/disabled/nextmap` passed. The JS gate required unsandboxed execution because existing subprocess tests encounter `spawnSync git EPERM` under sandboxing. No live CS2 server restart/deployment was performed.

Independent read-only review: ready, no actionable findings. Live CS2 command/workshop acceptance remains unverified; preexisting rotation/parser limitations are unchanged, and randomization on manual plugin reload is intentional.
