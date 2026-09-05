# Final native benchmark reruns

Source: `043861085902c43c6f53b63b3212e8a563e3382d`, Darwin arm64, rustc 1.98.1, optimized Rust. Five alternating baseline/candidate pairs use the previously reviewed workload sizes, setup boundaries and percentile definition. Every cell below is the median of the five per-run values; it is not a pooled percentile. Values are nanoseconds.

Timer results use the exact final TimerKind/TimerQueue and its production examined counter. Pool/V8/JS/engine work is excluded. The hook results repeat the reviewed indexed native model, not production dispatch; its additional entity/viewer rows have no matching baseline and are retained as model scaling data only.

The clock produces roughly 41-42 ns increments. A zero or one-tick result is resolution-limited, not zero work or a precise percentage improvement. The largest cancellation p95/p99 use only eight samples per run and are coarse.

## Timers: exact source

| Workload:size | Samples/run | p50 old -> new | p95 old -> new | p99 old -> new | Max old -> new |
| --- | ---: | ---: | ---: | ---: | ---: |
| timer_idle:0 | 2000 | 0 -> 41 | 42 -> 42 | 42 -> 42 | 42 -> 84 |
| timer_due:0 | 2000 | 0 -> 41 | 42 -> 42 | 42 -> 42 | 83 -> 84 |
| timer_cancelheavy:0 | 2000 | 0 -> 0 | 42 -> 42 | 42 -> 42 | 42 -> 42 |
| timer_idle:10 | 2000 | 0 -> 41 | 42 -> 42 | 42 -> 42 | 83 -> 42 |
| timer_due:10 | 2000 | 83 -> 84 | 125 -> 125 | 125 -> 167 | 1,250 -> 917 |
| timer_cancelheavy:10 | 1000 | 42 -> 500 | 42 -> 625 | 42 -> 750 | 167 -> 1,250 |
| timer_idle:1000 | 2000 | 458 -> 41 | 459 -> 42 | 459 -> 42 | 625 -> 83 |
| timer_due:1000 | 500 | 1,000 -> 2,875 | 1,042 -> 3,000 | 1,167 -> 3,334 | 2,917 -> 4,583 |
| timer_cancelheavy:1000 | 100 | 181,625 -> 131,917 | 190,500 -> 143,708 | 206,000 -> 160,709 | 206,000 -> 160,709 |
| timer_idle:10000 | 2000 | 4,417 -> 41 | 4,709 -> 42 | 5,833 -> 42 | 21,625 -> 42 |
| timer_due:10000 | 200 | 8,334 -> 25,208 | 11,208 -> 25,583 | 13,583 -> 29,375 | 17,834 -> 46,417 |
| timer_cancelheavy:10000 | 8 | 19,053,250 -> 1,909,083 | 19,322,208 -> 1,945,125 | 19,322,208 -> 1,945,125 | 19,322,208 -> 1,945,125 |

## Hooks: native model

| Workload:size | Samples/run | p50 old -> new | p95 old -> new | p99 old -> new | Max old -> new |
| --- | ---: | ---: | ---: | ---: | ---: |
| sdkhook_snapshot:1 | 10000 | 41 -> 41 | 42 -> 42 | 42 -> 42 | 875 -> 1,500 |
| sdkhook_snapshot:100 | 10000 | 42 -> 41 | 83 -> 42 | 84 -> 42 | 250 -> 167 |
| sdkhook_snapshot:1000 | 10000 | 292 -> 41 | 292 -> 42 | 375 -> 42 | 6,958 -> 208 |

## Interpretation

- At 10,000 timers, idle polling falls from about 4.42 microseconds to a one-tick measurement; cancellation-heavy work falls from 19.05 to 1.91 milliseconds.
- The indexed timer structure costs more for draining an entirely due queue: 10,000 due timers increase from 8.33 to 25.21 microseconds, and 1,000 due timers from 1.00 to 2.88 microseconds. Tiny cancellation also regresses (42 to 500 ns at size ten). These tradeoffs were investigated during Task 8 review and remain visible in the final rerun.
- In the hook model, one addressed subscriber at 1,000 total hooks falls from 292 to 41 ns median; sizes one and 100 are approximately clock-resolution equivalent. This is not live CheckTransmit evidence.
- No end-to-end engine throughput, viewer-callback parity, V8 heap or whole-RSS claim follows from these measurements.

Raw results, metadata and source snapshots are in `final-timer-bench-20260905T113817Z/` and `final-hook-model-bench-20260905T113835Z/` beside this report. `final-benchmark-summary.json` retains all five per-run rows. Copied timer harness comments still describe the original baseline; candidate metadata and candidate/async_rt.rs identify the exact measured replacement. The earlier timer extraction attempt at 113729Z failed compilation because it omitted the production examined-counter helper; no measurements from that attempt are used.

Final integrated source `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1` retains byte-identical `core/src/async_rt.rs` and `core/src/sdkhooks.rs` relative to the measured revision. The final header/config/revision corrections do not alter these kernels; no redundant rerun was used to select more favorable timings.
