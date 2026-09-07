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
```

Partition gauges cover admitted producers plus queued/staged work, not just unread
channel contents. `queued` and `staged` report that distinction without releasing
the byte charge on dequeue. `rejected` counts failed partition acquisitions/growth;
early argument-validation errors are not partition rejections. Duration counters
cover the async drain through the HOST-free callback phase. Snapshot fields are
read separately and can change concurrently; they are diagnostic gauges, not one
transactional process snapshot. Counters persist across isolate reinitialization.

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
