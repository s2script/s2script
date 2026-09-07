# Final consolidated fix-wave re-review

Reviewed final frozen HEAD: `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1`.
Comparison: original whole-stack reviewed `043861085902c43c6f53b63b3212e8a563e3382d`.
Checkout: `/Users/ghirakawa/projects/s2script`; verified clean at this HEAD.

## Verdict

**Scoped spec and quality verdict: PASS. F1, F2, and F3 are addressed. No residual actionable finding, Critical/P1 issue, or introduced regression was found in the consolidated fixes and their immediate interactions.** This closes the three P2 findings in `final-stack-review.md`; it does not claim completion of the still-pending full Linux/native/shim and live acceptance gates.

This is the single requested scoped independent re-review. I read the final integrated source diff, original findings/reproductions, Stage A/B/C report, focused RED/GREEN evidence, and integrated raw logs. I inspected final source and tests, including the extraction conflict resolutions. No production edits, rebase, SSH, subagent, or duplicate test-suite run was performed. The only review artifact written is this report.

## F1 — Initial config reconciliation: closed

Final locations: `core/src/loader.rs:1089–1106`; `core/src/v8host/lifecycle.rs:314–360`.

The first accepted result now reconciles **each plugin's** current applied values with the worker snapshot before the watch proposal is acknowledged. This runs for a first `Seed`, `Changed`, or shared-path `Unchanged` result, so an already-seeded shared worker path cannot hide a joining plugin's initial mismatch. Existing path/watch-generation validation precedes the reconciliation; pressure does not mark the plugin seeded. Subsequent changed-result callback behavior is preserved.

The helper normalizes both current and proposed values through V8 JSON serialization, checks serialized UTF-8 length before copying current contents into Rust, and uses structural JSON equality. This handles V8 `1` versus materialized Rust `1.0`, property order, escaped newlines, and Unicode without spurious initial callbacks. A mismatch takes the established application and owner/generation-checked callback path. HOST and PLUGINS borrows are released at the required boundaries. No recurring synchronous config read or retained native baseline map was added.

Evidence reviewed: the original actual-worker/V8 lost-edit RED, the intermediate numeric-default RED, `final-f1-canonical-green.log`, and integrated tests `delayed_coalesced_first_watch_and_joining_watcher_apply_unseen_edit` and `delayed_first_watch_and_joining_watcher_suppress_unchanged_defaults`. The delayed/coalesced first watchers and later joining watcher each observe B once in the edited case; unchanged generated-default JSONC produces zero callbacks. The following normal poll does not duplicate delivery.

The byte-length guard bounds native comparison copies relative to the proposed serialized config; it is not a bound on arbitrary plugin V8 heap or execution. JSON serialization can create V8 temporaries and invoke getters/toJSON. The implementation/report disclose that limitation; it is not a remaining instance of F1 or an added persistent ownership leak.

## F2 — Historical revision ownership: closed

Final locations: `core/src/loader.rs:665`, `818–823`, `957–978`.

The historical `PATH_REVISIONS` map is gone. One checked, never-reset monotonic scalar assigns revisions, while current expected path revisions remain only in `ACTIVE_BATCH.pending`. Cancellation removes pending work; later admission of the same or another path receives a distinct token. Existing lifecycle-epoch filtering remains intact. This removes ownership proportional to historical discovered paths without allowing an old result to match a replacement.

Evidence reviewed: original malformed-archive churn RED, strengthened uniqueness RED, `final-f2-green.log`, and integrated tests `removed_plugin_paths_do_not_accumulate_revision_tombstones` and `cancelled_and_deleted_recreated_paths_reject_late_worker_results`. The churn test inspects internal current-path stores and active/ready/waiting rows in addition to idle payload gauges after 64 distinct one-at-a-time paths. The second test replays a valid old prepared result after cancellation and after delete/recreate; the replacement token remains pending and only version 2 becomes ready. No replacement tombstone table was introduced.

## F3 — Header capacity accounting and growth: closed

Final locations: `core/src/jobs.rs:382–415`, `core/src/http.rs:242–245`, `core/src/v8host.rs:963–970`.

Both request builders admit/copy the key and value before calling the shared append helper. Its exact reserve target uses the already-admitted 64 bytes per header to fund actual tuple slots: `floor((len + 1) * 64 / size_of::<(String, String)>())`. At a growth boundary, the target stays within all admitted header metadata, and on the supported 64-bit layout it permits roughly 4/3 amortized growth. Existing string charges are not doubled, and the old unconstrained Vec growth is removed from both HTTP and WS request paths.

Evidence reviewed: original 513-header native RED; the rejected one-slot growth RED; `final-f3-amortized-green.log`; and integrated ordinary/fresh-policy tests. Actual retained native capacity falls from HTTP 51,104 / WS 51,101 to HTTP 33,248 / WS 33,245 bytes under the unchanged 34,813-byte charge. The helper invariant is checked after every append through 16,385 headers, with 31 capacity changes. The fresh 40,000-byte policy test also verifies that 601 headers produce named `AsyncPayloadTooLarge` for both natives before submission and leave no retained job/resolver/bytes after cleanup.

These measurements concern application-owned Vec/String capacity. They do not establish bounds on protocol buffers, allocator metadata, V8 memory, RSS, or whole-process memory.

## Extraction and integrated evidence

I inspected the integrated production changes against the original reviewed tree and the move witness against frozen slice 9 (`009ab2dec7b9df97bf8191ced71d90f91164fb15`). The F1 production block was preserved by extraction, the new F3 native and pressure tests remain in `v8host::frame_tests`, and the remaining production fix files/pressure script match frozen slice 9. The full moved frame-test content is equivalent excluding blank-only lines. Store ownership and shutdown remain unchanged.

I independently resolved the apparent native-name count discrepancy by enumerating literal `set_native` names from Git objects at original `0438610`, frozen slice 9, and final HEAD: all three have the same **240 non-test-path names**, plus the existing test-only `__test_socket_owned`. The recursive witness's 241 count includes that helper. There are no added or removed production registration names.

Raw integrated evidence reviewed:

- `final-integrated-core.log`: **797 passed, 0 failed, 3 ignored**, 9.61s; relevant new regression tests pass.
- `final-integrated-pressure.log`: all **three** exact fresh-process tests pass, one selected test per process. This includes the isolated 40,000-byte request-header policy.
- `final-integrated-static.log`: core boundary, negative name-leak probes, invoke ABI (`lifetime=10 flags=4`), and recursive JS/native-name lint pass. Stage C records successful C++ config syntax and both diff checks, which are silent on success.
- `final-integrated-linux-static.log`: Linux/amd64 Docker static generation/order/arity checks pass for 126 ops; deferred sentinel and selftest gates pass.
- `final-integrated-move-witness.log`: source/test move equivalence, shared-store ownership, shutdown, and recursive native-name equivalence.

The integrated source is frozen and these logs are the implementer/root's executions, independently inspected here. I did not rerun broad suites because no new unresolved execution uncertainty justified colliding with the shared target. The original full-JS/584-SDK/Docker evidence predates this fix wave; these fixes contain no JS/SDK source change, but this report does not relabel that evidence as a newly executed final full-JS gate.

## Remaining nonblocking observations and acceptance limits

The private child-module `use super::*` observation remains a separately scoped maintainability opportunity, with no runtime ownership or API finding. The original documentation-only trailing blank line has been removed. Neither warrants widening this fix wave.

Final full Linux compilation/link/symbol/native-shim validation, installed-engine execution, and the 60-minute integrated soak remain unproven because 1Password SSH authentication is unavailable. The last full Linux native/shim gate was slice 8. Passing static or C++ syntax checks does not substitute for those gates, and bot slot-reuse tests do not prove human CheckTransmit viewer traffic.

Benchmark raw/model scope remains root-owned and documented separately. This review makes no production speedup or RSS plateau claim from the focused header-growth witness, asymptotic structure, extracted test modules, modeled benchmarks, or logical gauges.
