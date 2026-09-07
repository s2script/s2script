# Coalesced component invalidation benchmark

This benchmark compares the Task 4 source at
`e63ac07e70d6cb2e1ac94f5efd4f89c2c3719faf` with the Task 5 implementation. It loads the actual
`games/cs2/js/ui.js` and each revision of `components.js` in isolated Node VM contexts. Only the
engine boundary is simulated. Candidate generation, paging, component painting, UI diff caches,
and the dirty-record scheduler all run production game-package code.

Run the complete matrix from the repository root:

```bash
node docs/benchmarks/2026-09-ui-api/run.mjs \
  --output=docs/benchmarks/2026-09-ui-api/results.json
```

Use `--quick` for a two-scenario smoke test. The checked-in raw result used Node v24.19.0 on
darwin-arm64. It contains 54 scenarios: 1/8/32 players × 1/2 visible modals × 10/100/1000 rows ×
1/10/100 update intents. Each scenario has five pairs; pair order alternates
baseline→candidate, candidate→baseline. Setup and initial open are outside the timed region.
Cleanup is also outside the timed region and outside the provider/operation counters.

Before timing, the harness advances one deterministic data revision. Baseline applies the same
number of synchronous owner `refresh()` calls as update intents. Candidate applies owner
`invalidate()` calls and then runs one pre-frame drain. Provider output depends only on the fixed
scenario, revision, modal, player, and row; provider-call counts never affect rendered data. Every
pair asserts an equal final rendered-state hash before its samples are accepted.

The measurements have these precise meanings:

- **Provider calls:** invocations of the real modal row provider during the measured update window.
- **Drive attempts:** calls from real `components.js` into real `ui.js` `_drive` methods, including
  calls that `ui.js` suppresses through its diff cache, during that same window.
- **Engine attempts:** calls during that window that pass diffing and reach a resolved engine/native
  stub.
- **Submitted writes:** successful mutating calls accepted by that stub. No failures are injected,
  so this equals engine attempts in this benchmark. These counters are captured after the candidate
  drain and before cleanup closes the modals.
- **Queue depth:** the actual private dirty-record queue length after all intents and before the
  candidate frame. After-drain depth is read after that frame, and cleanup depth is read from the
  same queue after closing every modal.

## Results

Across the 18 shape combinations at each intent count, summed scenario medians and p95 values were:

| Intents | Provider calls baseline→candidate | Drive attempts baseline→candidate | Median elapsed change | p95 elapsed change |
|---:|---:|---:|---:|---:|
| 1 | 369→369 | 35,793→35,793 | 0.63% slower | 1.30% slower |
| 10 | 3,690→369 | 357,930→35,793 | 89.25% faster | 88.77% faster |
| 100 | 36,900→369 | 3,579,300→35,793 | 98.73% faster | 98.69% faster |

The single-intent case performs the same repaint work plus queue/drain bookkeeping. Twelve of its
18 scenario medians regressed; individual median changes ranged from 10.53% slower to 1.81% faster.
Fourteen p95 values regressed, with changes ranging from 20.61% slower to 16.98% faster. This spread
also shows the noise expected from five samples. No 10- or 100-intent scenario regressed by median
or p95.

The largest cell (32 players, two modals, 1,000 rows, 100 intents) reduced provider calls from 6,400
to 64 and drive attempts from 620,800 to 6,208. Median elapsed time fell from 10,238.99 ms to
124.13 ms; p95 fell from 10,286.89 ms to 124.97 ms.

Engine attempts and submitted writes were equal for baseline and candidate in every scenario. The
first update submits the changed revision; later synchronous baseline refreshes still evaluate and
drive the component tree, but the real `ui.js` diff cache suppresses identical engine writes. This
means coalescing saves provider/candidate/drive work under this workload without claiming additional
write savings.

Candidate peak queue depth equaled the number of live modal/player pairs, from 1 through 64. Every
scenario reached zero immediately after drain and remained zero after close. All 270 paired final
rendered-state comparisons matched.

Source identity recorded in `results.json`:

- baseline Git SHA: `e63ac07e70d6cb2e1ac94f5efd4f89c2c3719faf`
- baseline `components.js` SHA-256: `dbc0b84efce57ce309207e50458af60cc86a481c1fdbcb6aae788de55a14eb0a`
- candidate `components.js` SHA-256: `13f24617b22a4e206dce5a53e18669ba854b5cb1efb5c1002de259fd04075f61`
- shared `ui.js` SHA-256: `5e07d3a512997accc0871cc4db28938f300d4d5b457326fb7a302b97e14b93b1`

This is a Node microbenchmark with five samples per variant/scenario and a simulated accepting
engine boundary. It does not measure CS2 frame time, FPS, network bytes, client rendering, or live
server contention. The raw samples and per-scenario medians/p95 values are in
[`results.json`](results.json).
