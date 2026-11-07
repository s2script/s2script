# Random startup map implementation plan

**Goal:** Offer opt-in random startup selection from `nextmap`'s maplist and configurable round-boundary rotation.

**Spec:** `docs/superpowers/specs/2026-11-07-random-startup-map-design.md`

- [x] Add VM-hosted plugin tests for delayed startup, both ends of random selection, workshop commands, no-map waiting, empty/invalid lists, current-map selection, cancellation, subsequent rotation, and admin overrides. Run them against the unchanged plugin and observe the missing startup switch.
- [x] Extend `plugins/disabled/nextmap/src/plugin.ts` to pick once in the existing first-map poll and reuse the delayed change function with an explicit candidate. Keep all normal map-end behavior.
- [x] Register the tests in `scripts/ci-js.sh`, run the JS gate and build the plugin, and review the final diff. Record actual gate limitations in the progress log.
Delivery: commit only this slice, push its branch, and open one PR for review; preserve the unrelated original checkout.

Validation: all 11 new tests failed against the unchanged plugin, then passed with the change. `bash scripts/ci-js.sh` passed in full (including 962 SDK tests, plugin/example typechecks, the new tests, and Docker compose/tooling checks). `npm run build --workspace @s2script/sdk` and `node packages/sdk/dist/cli.js build plugins/disabled/nextmap` passed. The JS gate required unsandboxed execution because existing subprocess tests encounter `spawnSync git EPERM` under sandboxing. No live CS2 server restart/deployment was performed.

Independent read-only review: ready, no actionable findings. Live CS2 command/workshop acceptance remains unverified; preexisting rotation/parser limitations are unchanged, and randomization on manual plugin reload is intentional.

## Follow-up: round-boundary switching

- [x] Reproduce abrupt mid-round timelimit and immediate round-end RTV switches with VM-hosted regression tests.
- [x] Queue ready nextmap rotation for `round_prestart` with a final-match fallback, retain prompt startup selection, and recheck queued source maps.
- [x] Apply RTV winners at the same round/match boundaries, preserving workshop IDs; update nextmap announcement and config description.
- [x] Review found competing rotation/RTV commands on one boundary. Reproduced with eight failing two-plugin tests, then added an optional typed coordination hook so a voted target wins in either load order, including identical targets. Standalone rotation remains available.
- [x] Run the full JS gate, build both archives, and obtain a read-only review before updating the existing PR.

Follow-up validation: 32 focused tests pass, including the combined rotation/RTV regression in both load orders and at prestart/match end. Both plugin archives build with matching producer/consumer contract hashes. The final full `bash scripts/ci-js.sh` passes after the review fix (including 962 SDK tests and all plugin/example typechecks). Live CS2/client acceptance remains pending. The review's competing-command finding is fixed; its preexisting late-vote/intermission and same-name map-reload concerns are outside this slice.

## Review guidelines: operator settings

The [PR review](https://github.com/s2script/s2script/pull/340#issuecomment-6713564447) reverses the initial no-new-setting decision.

- [x] Add bool `nextmap_random_startup=false`; read it at the first live-map observation and consume the startup choice even when disabled.
- [x] Add bool `nextmap_wait_for_round=true`; false applies normal rotation immediately after the existing delay, with the `Changing To` announcement.
- [x] Keep the automatic-rotation veto independent of round waiting so RTV retains priority in either mode; describe both delay modes and update the spec.
- [x] Verify both flag states, default startup behavior, config changes before/after the first live map, both rotation triggers, and both plugin orders. Six behavioral tests failed before the fix; all 44 focused tests pass afterward.
- [x] Run the full JS gate, build both plugins, and review the changes against the comment before updating PR #340.

Review-guidelines validation: all 44 focused tests pass; the full JS gate passes, including 962 SDK tests and all plugin/example typechecks. Both archives build, and the nextmap archive contains bool defaults `nextmap_random_startup=false` and `nextmap_wait_for_round=true`. Independent review found no actionable issues. Live CS2 acceptance/deployment remains pending.
