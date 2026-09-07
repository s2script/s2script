# UI continuation baseline — September 5, 2026

The UI continuation starts above the existing PR #195. Original PRs #186–#195 were rebased, preserving their numbers and dependencies, onto upstream `9a36af4834a19e94d0485cdaa6ece543d6fbc8ab`. The new integrated top is `8ca3978b188d956026b52aa0d195f28bbd448887`.

Independent integration review confirmed registration before prelude evaluation, extracted native/owner-store/retry wiring, shared panel/cursor ownership, entity-bound caches, and client-generation fencing survive together. Fixture corrections belong to the original client-identity slice, not a later UI feature slice.

Validation:

- `node --test` focused UI group: 149 passed at the exact baseline.
- `bash scripts/ci-js.sh`: all JS gates passed, including SDK build/tests, typechecks, component/markup checks and Docker gate helpers. This ran at `97467cf`; its entire JS/build-script tree matches the final baseline. The only subsequent tree change corrected Rust test fixture connection tokens.
- `bash scripts/ci-native.sh`: full Linux Docker gate passed at the exact baseline: 816 core tests passed, 3 ignored, fresh-process pressure cases, C++ sanitizer/ABI checks, shim build, and 47 core entry-point resolutions.
- Installed CS2 game-symbol resolution was skipped locally because that container has no game install. No live render, click, focus, or spectator acceptance is claimed by this baseline.

The first native attempt found two shared-switch tests using retired slot-only event delivery or disconnecting without an established client lifetime. Tests were updated to use actual connection tokens and to prove a stale disconnect cannot clear replacement capture. The production fail-closed guard was preserved.

Historical benchmark and hour-soak evidence in the original hardening records remains evidence for that pre-rebase runtime. It is not evidence for this new baseline or the new UI implementation. Human acceptance and the separate MultiAddonManager startup/map incident remain open.

All original branches were published atomically using exact expected old-SHA force-with-lease checks. A full pre-restack Git bundle was verified and retained privately. The root PR is now mergeable; no PR has been merged by this work.
