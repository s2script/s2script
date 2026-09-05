# Native async admission and frame budgets

Native work has finite admission limits. Promise APIs reject with an Error named
`AsyncQueueFull`, `AsyncPayloadTooLarge`, or `AsyncCancelled`. `WebSocket.send`,
`TcpSocket.send`, and `UdpSocket.sendTo` return `false` if the entire send cannot be
accepted; callers can retry later or close. A successful send means queued, not delivered.
`delay`, `nextFrame`, and `threadSleep` reject on overload; `after` and `every` throw
`AsyncQueueFull`. Existing timer cancellation and socket graceful-close ordering remain.

Set `S2SCRIPT_ASYNC_LIMITS_JSON` before starting the process. It accepts a JSON object
with any subset of the fields below. Unknown fields, zero values, impossible relations,
or overflow invalidate the entire override and print a warning before using defaults.
The policy is read once per process, including across plugin unload and V8 reinitialization.
For example, `{"frame_items":64,"frame_bytes":1048576}` changes only those two limits.
Byte units below are binary KiB/MiB; the JSON uses integers in bytes.

| Fields | Defaults | Scope |
| --- | --- | --- |
| `jobs_global`, `jobs_per_owner` | 512, 64 | Admitted producers and retained completions; includes live sockets |
| `input_bytes`, `owner_input_bytes`, `input_item_bytes` | 32 MiB, 8 MiB, 8 MiB | Job input aggregate, owner aggregate, single input |
| `completion_bytes`, `failure_bytes` | 16 MiB, 4096 | Aggregate result reservations; reserved failure allowance per job |
| `sockets_global`, `sockets_per_owner` | 128, 16 | Actual socket lifetimes, including shutdown in progress |
| `sqlite_global`, `sqlite_per_owner` | 32, 4 | Actual SQLite actor lifetimes |
| `pools_global`, `pools_per_owner` | 16, 4 | Actual remote pool lifetimes; each pool has at most four backend connections |
| `socket_out_items`, `socket_out_bytes`, `outbound_bytes` | 256, 1 MiB, 16 MiB | Per-socket queued items/bytes (also per-send ceiling), global outbound bytes |
| `inbound_items`, `inbound_bytes` | 2048, 16 MiB | Aggregate socket data events retained by producers, queues, and dispatch |
| `timers_global`, `timers_per_owner` | 4096, 512 | Pending and selected timers |
| `worker_queue_items`, `sqlite_queue_items` | 256, 64 | Waiting jobs in the pool / each SQLite actor |
| `http_body_bytes` | 10 MiB | Final UTF-8 body plus retained response headers |
| `db_result_rows`, `db_result_bytes` | 10,000, 8 MiB | Rows and normalized columns/cells plus conservative row metadata |
| `cookie_versions`, `cookie_bytes`, `cookie_write_bytes` | 4096, 4 MiB, 64 KiB | Existing cookie outbox: retained versions, aggregate bytes, single write |
| `frame_items`, `frame_bytes`, `frame_poll_items`, `frame_soft_us` | 256, 2 MiB, 256, 2000 | Logical deliveries, delivered bytes, source polls, soft elapsed-time stop |

The plugin/config loader is configured through the nested `loader` object. Its `parse`
member is nested in turn; both levels accept partial overrides and reject unknown fields.
For example, `{"loader":{"request_items":64,"parse":{"zip_entries":128}}}` changes
only those two loader limits.

| Loader fields | Defaults | Scope |
| --- | --- | --- |
| `request_items`, `request_bytes` | 128, 32 MiB | Worker obligations, transient request bytes, main config-consumer table, and the independent persistent watch-control partition |
| `result_items`, `result_bytes` | 128, 64 MiB | Reserved worker result obligations and bytes |
| `prepared_items`, `prepared_bytes` | 32, 64 MiB | Main-thread retained prepared loads across active batches, READY, and WAITING |
| `scan_entries`, `scan_candidates` | 4096, 1024 | Examined directory entries and returned `.s2sp` candidates |
| `path_bytes`, `archive_bytes`, `config_bytes` | 256 KiB, 32 MiB, 1 MiB | Aggregate scanned candidate-path bytes, one archive, and one raw config read used by reservations |
| `config_baseline_items`, `config_baseline_bytes` | 128, 32 MiB | Union of watched config paths and simultaneous committed/proposed logical path+decoded-content retention |
| `parse.zip_entries`, `parse.member_name_bytes` | 256, 64 KiB | ZIP entry count and aggregate member-name bytes |
| `parse.manifest_bytes`, `parse.plugin_js_bytes`, `parse.gamedata_bytes` | 1 MiB, 16 MiB, 8 MiB | Parsed archive member limits |
| `drain_items`, `drain_bytes`, `drain_micros` | 8, 16 MiB, 1000 | Separate per-Post loader result/application soft budget |

Loader validation requires every field to be nonzero and safely sized, result items to
cover request obligations, and result bytes to hold the configured scan, decoded config,
or parsed-plugin payload limits.
Prepared bytes must hold one maximum parsed plugin plus the worst-case decoded config.
Config baseline bytes must hold one committed and one proposed maximum lossy-decoded
config before actual path charges. Any invalid nested relationship invalidates
the complete environment override, including otherwise valid top-level changes.

These are starting defaults, not measured production bandwidth targets. They retain
the baseline 10 MiB HTTP ceiling and four-connection SQL pools. The worker queues
allow bounded bursts over the four-thread Tokio runtime; socket and owner limits
allow several ordinary integrations while preventing one owner from occupying every
slot. The timer and cookie limits leave room for a typical 64-player server's repeated
work. Tune from rejection and retained-byte metrics during the deployment's soak.
The finite counts are independent of worker throughput or a particular frame rate.

Admission precedes native input copies, resolver registration, and ledger insertion.
The captured owner includes its generation; host work has a separate owner key.
Input/result leases follow the producer into its queue envelope, through staging and
V8 materialization. Removing a resolver does not release the producer's reservation.
Uncancelable SQLite execution and old actors remain globally charged after reload
until they actually exit; four cookie writer attempts are not a global DB-work cap.
Remote pool clones and asynchronous close retain their lifetime lease. Pool metadata
has a separate aggregate byte partition using `input_bytes` / `owner_input_bytes`;
it conservatively reserves four times the configuration's UTF-8 bytes plus 256.

A live socket conservatively retains one job/input/failure reservation until its
worker, registry, and queued/staged events have all released it. Its connect and
terminal controls reserve additional diagnostic space up front. Data saturation
cannot consume that control reservation. Socket readers can await data capacity off
the game thread, with shutdown able to interrupt the wait. HTTP and SQL result
builders grow reservations without waiting: if growth fails, they discard partial
output and publish a bounded failure. This avoids builders deadlocking while each
holds a partial result and waits for the other's bytes.

Polling and delivery use persistent rotating cursors. The first polling source rotates
independently of the number of polls completed and only advances when the pre-callback
phase actually polls. Thus neither complete polling rounds nor alternate callback-priority
frames can permanently deny a byte-blocked result its turn in an empty frame. Moving a socket event to a
staging queue consumes a poll, not a logical delivery. Promise/timer work and the
HOST-free cookie/socket callbacks share the same frame budget; when callbacks are
pending, they alternate priority with promise/timer work, including at `frame_items=1`.
Each socket keeps FIFO ordering, and connect settlement plus the checkpoint precedes
its data/terminal callbacks. One oversized delivery can run alone in a frame for
eventual progress. The elapsed-time check is soft and happens between units: one JS
callback, V8 materialization, or microtask checkpoint can exceed it. Timer selection
is a finite linear scan on this slice; the later indexed-timer slice replaces that
kernel with the same limited-selection contract. This is not a hard Post-frame
wall-clock guarantee.

Cookie persistence retains Task 5's single outbox, stable epoch, revisions,
`covers_from`, ACK rules, and per-account FIFO. Successful ACKs evict only the
acknowledged offline cache key when no newer same-key write remains. Active client
snapshots and pending newer values remain available. A slow sibling key cannot keep
an unbounded history of acknowledged account values in memory. Reads of evicted
offline values require the existing persistence-backed client cache load path.

Development instrumentation is `JSON.parse(__s2_async_stats())`; the native returns
a JSON **string**, without creating a job. Its schema is:

```text
jobs, completion, sockets, sqlite, pools, inbound, outbound, timers:
  { items: number, bytes: number, rejected: number }
queued: { worker, http, db, ws, net: number }
staged: { timers, ws, net, cookies, http, db: number }
cache: { accounts, entries, bytes: number }
frame: null | { items, bytes, polls: number }
timerExamined: number
lastNs: number
maxNs: number
loader:
  running: boolean
  worker:
    obligations: { items, requestBytes, resultBytes: number }
    queued, inFlight, results: number
    controls: { items, bytes, pending: number }
    config: { paths, bytes: number }
    baselines: { items, bytes: number }
    proposals: { items, bytes: number }
  main:
    pending: { items, bytes: number }
    active, ready, waiting, applying: number
    retained: { items, bytes: number }
  rejected:
    { requestItems, requestBytes, resultItems, resultBytes,
      controlItems, controlBytes, configPaths, configBytes,
      pendingItems, pendingBytes, retainedItems, retainedBytes: number }
  highWater:
    { obligations, requestBytes, resultBytes, queued, inFlight, results,
      controlItems, controlBytes, configPaths, configBytes,
      baselineItems, baselineBytes, proposalItems, proposalBytes,
      pendingItems, pendingBytes, retainedItems, retainedBytes: number }
  limits:
    { requestItems, requestBytes, resultItems, resultBytes,
      preparedItems, preparedBytes, scanEntries, scanCandidates,
      pathBytes, archiveBytes, configBytes, configBaselineItems,
      configBaselineBytes, drainItems, drainBytes, drainMicros: number,
      parse: { zipEntries, memberNameBytes, manifestBytes,
               pluginJsBytes, gamedataBytes: number } }
```

Partition gauges cover admitted producers plus queued/staged work, not just unread
channel contents. `queued` and `staged` report that distinction without releasing
the byte charge on dequeue. `rejected` counts failed partition acquisitions/growth;
early argument-validation errors are not partition rejections. Duration counters
cover the async drain through the HOST-free callback phase. Snapshot fields are
read separately and can change concurrently; they are diagnostic gauges, not one
transactional process snapshot. Counters persist across isolate reinitialization.

`loader.worker.obligations.items` is the single admitted worker-obligation count.
Its request and result byte reservations remain charged until main takes the result.
`queued` (pending requests plus coalesced reruns), `inFlight`, and `results` locate those
same obligations and are not additional ownership to sum. Each obligation has at most
one queued row, so `queued` cannot exceed `obligations.items`.

Watch controls are a separate persistent partition capped by loader request items/bytes.
Their `items` and `bytes` remain nonzero while config paths are watched; `pending` counts
controls currently carrying Ack, Discard, or Retire work. `worker.config.paths` is the
union of committed-baseline and proposal paths checked against `configBaselineItems`.
`worker.config.bytes` is the simultaneous logical charge `baselines.bytes +
proposals.bytes` checked against `configBaselineBytes`. The individual high-water
breakdowns are not additive across samples; `highWater.configPaths` and
`highWater.configBytes` record the shared partition peak.

`main.pending` is the independently bounded config-consumer table, with the effective
`requestItems`/`requestBytes` caps. `main.retained` is the exact prepared lease ownership
across `active`, `ready`, `waiting`, and a re-entrant lifecycle `applying` call; those
four counts only locate the leases. Rejection and high-water counters are monotonic
saturating values for the current joined loader lifecycle. Loader shutdown joins the
worker, drops main retained queues, and then resets its gauges and counters. A periodic
watcher can make an individual sample busy; bounded idle checks should sample until
obligations, queued/in-flight/results, pending controls, proposals, main pending/active/
ready/waiting/applying, and main retained gauges are zero. Persistent controls and
committed baselines remain charged until final unwatch.

Worker and main loader fields are sampled independently without holding a lock across
engine or V8 callbacks. They are diagnostic readings, not a transactional snapshot and
not a whole-process RSS measurement. Logical accounting excludes allocator/container/
thread overhead and the one bounded worker read currently inside a regular-file kernel
operation. Joined shutdown can wait for that regular-file operation to return.

These limits cover application-owned input/result/event retention, with conservative
metadata allowances. They do not bound whole-process RSS: allocator overhead,
SQLite page caches, sqlx/HTTP/WebSocket protocol decoding buffers and connection
state, TLS/kernel buffers, and V8 objects retained by JS or promise continuations are
outside these partitions. The WS parser separately caps messages/frames, but upstream
drivers can allocate a row or protocol header before the application sees it. UTF-8
replacement expansion, HTTP headers, SQL parameter strings and column names are
charged when materialized into application-owned values. SQL outer-row arrays explicitly
charge geometric spare capacity before growing; row/column/cell allowances remain
conservative. Lossy UTF-8 output uses its pre-counted exact capacity, so conversion
does not introduce an uncharged geometrically grown scratch string.

Run `cargo test -p s2script-core` and `bash scripts/test-async-pressure.sh`. The latter
starts a fresh test process with tiny limits, barrier-held producers, isolate reload,
and a one-item frame budget, followed by a separate six-poll process checking oversized
HTTP/DB progress beside a continuously due timer; `scripts/ci-native.sh` runs these gates. Do not change policy
in place while producers from an earlier configuration remain alive.
