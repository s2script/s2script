# UI API hardening acceptance evidence

Status: final local evidence is recorded at source commit
`102ff64267a74b010a83171477edd7e374425ebe`. The three P2 findings and the P3 evidence-label
finding are fixed and independently approved in `final-rereview.md`. Tasks 0–6 and the local
Task 7 evidence gates are complete. The draft stack is navigable from the root draft [PR #186](https://github.com/s2script/s2script/pull/186), where code publication is tracked. Live staging and human acceptance remain pending.

## Evidence boundaries

The local JavaScript churn runs the real `games/cs2/js/ui.js` and `games/cs2/js/components.js` in
separate Node VM contexts. Engine calls and native ownership are deterministic host stubs. Internal
JavaScript queue and subscription counts are direct runtime observations; pool-owner, focus,
owned-surface, and capture counts in that test are host-stub observations. They are not native-host
evidence or client render acknowledgements.

The final native run is the integrated Linux gate at the exact source commit. Its ordinary 850-test
run reports three intentionally ignored policy-isolation tests; the native gate also runs each of
those three tests individually, and all three pass. The run includes the native 1,000-cycle churn proof;
the focused `surface_leases` fixture previously recorded 34/34. The native run proves host
ownership and cleanup, not client rendering, click delivery, or spectator privacy.

No new UI artifact has been deployed. SSH signing restoration, owned-server staging, rollback,
restart coordination, and the human protocol remain pending. The separate MultiAddonManager
startup/map crash remains an independent incident; an addon-loading failure blocks client
acceptance and does not count as an API pass or failure.

## Final source and gate evidence

All final gate logs are in the evidence ledger
`/Users/ghirakawa/projects/s2script/.superpowers/sdd/2026-09-05-ui-api-hardening`:
`task7-final-js.log`, `task7-final-native.log`, `task7-final-sniper.log`, and
`task7-release-symbols.log`.

| Check | Result at `102ff642` |
| --- | --- |
| Full JS gate | exit 0; 213 focused UI tests and 595 SDK tests passed; plugin/example typecheck and lint passed |
| Full Linux/native gate | exit 0; 850 core tests passed, 0 failed, 3 policy tests intentionally ignored; native churn passed |
| Sniper build | exit 0 in `s2script-final-linux-builder:local`, derived from `rust:bullseye` (Debian glibc 2.31); `s2script.so` needs GLIBC 2.17 and `libs2script_core.so` GLIBC 2.30 |
| Release symbols | both release/packaged shim symbol checks passed for 47 symbols; installed-game check skipped because no game install is present |
| Independent final rereview | approved; all three P2 findings and the P3 label correction fixed |

The final JS command was the repository's `bash scripts/ci-js.sh`; the final native command was
`bash scripts/ci-native.sh`. The native build context was `s2script-final-linux-builder:local`,
derived from the documented `rust:bullseye` image; the release artifact was packaged from that
build. Publication-head gate results are recorded in the final acceptance PR; documentation-only
changes do not require rebuilding the source-identical release artifact.

The authoritative artifact ledger is `final-artifact-summary.json`. The archive was built from
source commit `102ff642` and contains the runtime plus both UI fixtures. Its archive identity is:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `s2script-ui-acceptance-102ff64267a7.tar.gz` | 24,559,916 | `64fe3dd2f26812c3b5e3f1c67352b0121acf57b462d4ecac582fad4193cb525f` |
| `libs2script_core.so` | 76,011,312 | `ae96404d9e9646c97c202fd37dac84a483d4cc9ade0c5502a1be52c21eb9d8f7` |
| `s2script.so` | 3,364,960 | `d6c9a3653cc804b41b4955e336a2352189259a9ca88db1c7fbf8f1acda5d6cfe` |
| packaged `pawn.js` | 1,131,847 | `e871555439bd3147e8124170f30e186d610fef25df43622638071eec4c83baca` |
| `_demo_ui-multiplugin-a.s2sp` | 3,780 | `437578a7f06ab4049cf53af059b2a0a802c218e06da0441f2666b11dd4f6c978` |
| `_demo_ui-multiplugin-b.s2sp` | 2,395 | `32813730b14a32517be62fe3795e6215379ab0e841d5737365e0608d22a7cbbb` |

Root additionally verified that packaged `pawn.js` is byte-exact concatenation of the reviewed
source inputs. The artifact source-build SHA is `102ff642`; a later documentation-only commit
changes the repository HEAD and does not change this archive.

Reviewed source hashes:

| Source | Bytes | SHA-256 |
| --- | ---: | --- |
| `games/cs2/js/ui.js` | 49,184 | `c01cc61f15f58b7f36f2b1cc8268304079dd1b3392e1fdbaeef65b331c0f667e` |
| `games/cs2/js/components.js` | 122,461 | `153bf52c23e94f56c1d14cabe75d46dab99606dc28368eb366edb707ca85aaef` |
| `packages/cs2/ui.d.ts` | 30,073 | `825fa5c94c625e8ff181febf596a22640ad6fb11f50dc122e8b81bd0d8926283` |

## Reproducible local commands

Run from the repository root. These are the commands represented by the retained evidence:

```sh
npm ci
(cd packages/sdk && npm run build)
node packages/sdk/dist/cli.js build examples/ui-multiplugin-a --packages-dir packages
node packages/sdk/dist/cli.js build examples/ui-multiplugin-b --packages-dir packages
node --test --test-name-pattern='1000 VM reload cycles' games/cs2/js/hudkit-prelude.test.js
bash scripts/check-components-test.sh
node --test packages/sdk/test/cs2-ui.test.mjs
bash scripts/check-core-js-lint.sh
bash scripts/check-plugins-typecheck.sh
bash scripts/ci-js.sh
bash scripts/ci-native.sh
```

The final focused UI result was 213/213, the SDK total was 595/595 in the full JS ledger, and the
native result was 850 passed, 0 failed, 3 ignored. Both fixture builds exited 0. The existing
`examples/*/` wildcard includes both fixtures in plugin typecheck; no extra gate registration or
workspace-lock entry is required.

Task 5 benchmark evidence is historical and remains attributed to its exact Task 5 runtime and
source. It is not a Task 6 or Task 7 benchmark and was not rerun for this acceptance refresh. The
benchmark retains its fixed matrix, paired baseline/candidate results, output hashes, and cleanup
measurements in `docs/benchmarks/2026-09-ui-api/` and its ledger report. No FPS, network-byte, or
process-memory-bound claim is made from those VM/stub measurements.

## Automated 1,000-cycle churn

The real-prelude regression performs exactly 1,000 cycles. Each cycle creates a fresh plugin VM
after unloading the previous one and exercises modal acquisition, focus-aware open, synchronous
refresh, deferred invalidation/drain, queued invalidation cleanup, click delivery, explicit release,
owned banner disposal, disposable click subscription storage, same-slot generation replacement,
and plugin unload/reload. Stale handles cause zero provider evaluations, actions, or engine writes.

The JavaScript measurements return to baseline on every cycle:

| Measurement | Active/queued | After disposal/release | Fresh VM after reload |
| --- | ---: | ---: | ---: |
| Actual component dirty queue | 1 | 0 | 0 |
| Actual subscription route keys | 1 | 0 | 0 |
| Actual retained subscription entries | 1 | 0 | 0 |

The separate host-stub measurements are one while active and zero after cleanup: panel pool claims,
focus leases, owned-surface leases, capture routes, and capture holders. Totals are 4,000 provider
evaluations, 1,000 modal actions, and 1,000 subscription actions. The native churn proof likewise
returns owner, registry, and capture counts to zero. These are cleanup proofs within their stated
fixtures, not a client acknowledgement or a general memory bound.

## Actual fixture command runbook and pending human protocol

Fixture A commands are `sm_ui_a_open [slot]`, `sm_ui_a_reorder`, `sm_ui_a_refresh [slot]`,
`sm_ui_a_invalidate [slot]`, `sm_ui_a_banner_busy [slot]`, `sm_ui_a_close [slot]`,
`sm_ui_a_release`, and `sm_ui_a_status [slot]`. Fixture B commands are
`sm_ui_b_open [slot]`, `sm_ui_b_refresh [slot]`, `sm_ui_b_invalidate [slot]`,
`sm_ui_b_close [slot]`, `sm_ui_b_release`, and `sm_ui_b_status [slot]`. There is no synthetic
churn, metrics, reload, reconnect, or click command; normal operator and real-client workflows
are required.

The concrete owned target is `s2script-cs2-hardening` at `nebula.gkh.dev:27016`; production
`s2script-hudlab` is excluded. The connection command is `connect nebula.gkh.dev:27016`. Before
any restart, check whether a human is present and coordinate around that presence. Take the rollback
backup first, then stage only the owned target with the sniper artifact. Do not restart production.

For a human slot `S`, retain the existing runbook boundaries: reorder A and verify the visible
stable ID on click, repaint and invalidate A, open B and verify covered-A suppression, close B and
verify A restoration, require `Busy` for the second banner, release capture, reload each fixture,
reconnect the same account into the same slot with a new client generation, and finish by releasing
both fixtures while recording status, `meta list`, cursor/control state, and spectator observations.
Per-slot HUD state is not proven confidential against spectator viewing; raw observers and arbitrary
low-level layouts remain outside opt-in focus ownership.

## Remaining acceptance and publication

Owned `s2script-cs2-hardening` staging with rollback, SSH signing restoration, any coordinated
restart, and the human render/click/focus/cursor/reconnect/reload/pool-reclaim/spectator checks are
still pending. Draft-stack navigation and code publication are tracked from [root PR #186](https://github.com/s2script/s2script/pull/186); the artifact/source-build SHA remains `102ff642`, while this documentation refresh has its own commit SHA. Live staging and the human protocol are the remaining acceptance steps. No merge or deployment authorization is added here.
