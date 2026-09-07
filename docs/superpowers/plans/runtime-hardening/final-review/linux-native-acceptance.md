# Final Linux native validation

Source: `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1`. This is the independently reviewed production tree; subsequent commits record evidence and preserve the live test harness without changing production code.

The complete `scripts/ci-native.sh` exited zero in an isolated Linux/amd64 Docker container on the Mac. Rust 1.98.0, GCC 10.2.1, CMake 3.28.6; four CPU quota and 7 GiB memory. The builder derives from rust:bullseye and has image ID `sha256:16384f64da341d31dd0946a53ce7c109ee514f42ec8b867744de61a9713bc1cc`. Source and submodules were cloned separately; Linux target/cargo volumes did not overwrite the Mac build.

- Core: 797 passed, zero failed, three ignored, 11.40 seconds.
- All three isolated pressure processes pass.
- C++ unit and sanitizer gates, generated source, license freshness, boundary and ABI checks pass.
- The full shim compiles and links, including the extracted config translation unit; its tokenizer test passes.
- All 47 required `s2script_core_*` entry points are defined in the built core library.

The game-library symbol-resolution phase explicitly skipped because this isolated checkout has no CS2 installation. The [raw log](final-local-linux-ci-native.log) preserves that skip. The missing phase subsequently passed on Nebula against the actual installed CS2 libraries and shipping release artifact: all 47 core entry points were defined and the shim's game-library symbols resolved. The [remote symbol log](final-nebula-release-symbols.log) records that separate result.

Initial image setup failed because the Docker credential helper was absent from PATH, then timed out inside the helper. A task-local Docker configuration allowed anonymous retrieval of the public Rust image; the user's Docker configuration was unchanged. No sanitizer or test was disabled to obtain the passing run.

## Release package

The same compiler environment built the optimized core and release-linked shim, then packaged the addon. All 18 base-plugin builds passed; the release contains the 14 enabled base archives. The shim requires at most GLIBC 2.17 and the core at most 2.30, within the documented 2.31 server ceiling. Core SHA-256: `f1d3519dd9deafd24970d14aa5707664e6dc67382c4011179b1bca9610880331`; shim SHA-256: `5a6b58f96421abc9bfe98a91471911577d5ddc02e4b55bb399f6541501c1326d`.

The 24,473,784-byte compressed release archive has 45 files, each re-read and verified against the [release manifest](final-linux-release-manifest.json). It was installed on the isolated `s2script-cs2-hardening` container on Nebula on September 5. Mounted core/shim hashes match the manifest, Metamod loads s2script, and all 18 enabled plugins run (14 base plugins plus four acceptance fixtures). The [release build log](final-local-linux-release.log) records compilation and GLIBC checks.

The installer preserved configs, data, translations, custom gamedata and fixtures. The previous complete addon tree is retained at `/home/ghirakawa/s2script-hardening/.gate/final-ba6c7c1/previous-addon`. The separate production `s2script-hudlab` container was untouched; its start time remained `2026-09-04T23:39:08.651834244Z`. The installed source stays at the full SHA above throughout acceptance.

## Live soak acceptance

The loader-aware collector has 42 passing deterministic tests plus a bundled fixture protocol check. A five-minute compatibility pilot completed its three measured cycles and all cleanup proofs with no resource or loader errors. Its overall result was FAIL solely because no same-slot reuse occurred in that short run; this is not counted as an accepted soak.

The full 3,600-second run started at `2026-09-05T17:28:45Z` in
`.gate/mixed-soak/mixed-soak-20260905T172845Z` and passed all 57 measured cycles. It finished
with 18 running plugins, no collector errors, 58/58 reload acknowledgements and Active
transitions, 48 actual bot-churn attempts, and five actual same-slot reuses. The last ten cycles
reported no eligible bot and were skipped without receiving attempt or reuse proof credit; the
48-attempt total comes from the final status checkpoints.

The pressure phase settled all 256 requests: 64 succeeded, 192 received the expected named
capacity rejection, and zero had another outcome. Ten retained test handles remained at the final
cycle and were cleaned successfully. The fixture restored the original 94-byte JSONC config
byte-for-byte. Loader accounting returned from the 395-byte measured-config plateau to the
472-byte original-config plateau, with two paths and two baseline items in each state. The final
loader state was idle and all cleanup proofs passed. The [soak evidence index](live-soak-20260905/README.md)
links the retained report, all checkpoints, selected snapshots and the complete raw archive.

The reported `lastNs` percentiles summarize one sampled async-drain value from each of the 57
cycles; they are not engine frame-time percentiles. The RSS samples cover the whole container
process and are observations only, not an asserted memory bound. Human authenticated reconnect,
rendered HUD/chat/console behavior, and actual SetTransmit callbacks from a signed-on viewer remain
separate outstanding checks.

Startup validation reports a pre-existing EndTouch signature failure (35 of 36 descriptors available), also present in six earlier test-server startups. That descriptor fails closed and remains unavailable. The soak has no blanket engine-error allowlist. No live-pass claim includes the unavailable descriptor or unobserved human behavior.

## Additional human-setup limitation

After the accepted soak, enabling server-side MAM addon mounting prevented TypeScript plugin activation; an explicit map reload then crashed the isolated server. Restoring its exact original client-side-only MAM configuration restored all plugins. This [incident remains unresolved](mam-startup-incident.md); the passing soak does not establish server-side addon mounting or that map-transition path. Human checks proceed only on the recovered configuration.

## Separate inherited tooling advisory

Fresh npm installation reports GHSA-5p4m-2wfm-xmqj in js-yaml. Read-only dependency triage traces the affected 4.3.0 and 3.15.0 copies solely through the root Changesets development dependency; manifests and lockfile are unchanged from main. This is release-metadata YAML tooling, not a dependency packaged in the native addon or ordinary CLI build. A fresh `npm audit --omit=dev --json` reports zero vulnerabilities.

Scope ruling: retain this as a dependency-only follow-up, rather than modifying the reviewed runtime stack's dependency graph. A targeted lockfile refresh to compatible js-yaml 4.3.1 and 3.15.1, followed by npm installation/audits and the JS gate, is the minimal suggested remediation. Malicious checked-in YAML can still consume excessive CPU when a maintainer runs the affected Changesets tooling; the advisory is not dismissed as nonexistent.
