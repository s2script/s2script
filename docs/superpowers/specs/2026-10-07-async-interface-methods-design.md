# Async Interface Methods (Protocol 2)

**Date:** 2026-10-07

**Status:** Proposed. Not approved; no implementation exists.

**Implementation plan:** to be written after approval.

**Stack:** extends [plugin interop (protocol 2)](2026-09-06-plugin-interop-design.md). Forwards are unchanged.

## Purpose

Protocol 2 lets a contract method return only a synchronous wire value. That is right for forwards
(a Hook or Transform listener is a decision the engine needs now), but it is wrong for methods
whose honest answer depends on I/O. Today such a service must either fake a synchronous answer from
an in-memory mirror or invent a request-id + completion-notification protocol per plugin.

Evidence from the first real ports onto SDK 0.27:

- **Gangs (`@gangs/api` 1.0.0).** To satisfy sync-only methods it now loads every gang, rank,
  member and stat at start, answers from that cache, and writes through a background queue. The
  consequences are visible in its contract: an `isReady()`/`OnReady` startup window in which reads
  return empty and writes fail; wallet writes for a player who is neither online nor a gang member
  return failure; `tryPurchase` returns before the database acknowledges anything; writes queued at
  unload are lost; and anything that needs a real query (a leaderboard position, requested by the
  Jailbreak gang-perk port) cannot be offered at all.
- **TTT (s2s-ttt ADR 0001).** Settled on "async storage operations return IDs with
  status/completion notifications" — the per-plugin workaround this spec replaces.
- **Legacy contracts.** The cookbook's async `workshop` contract stays on protocol 1
  (`examples/legacy-contracts`) only because protocol 2 cannot express it. Under protocol 1 a
  returned Promise is `JSON.stringify`'d to `{}` (`core/src/v8host.rs` protocol-1 result path).

When every author rebuilds the same correlation/timeout/lifetime machinery, the framework should own
it — with the same liveness guarantees the host already gives timers, `fetch` and the database.

## Decision

Allow a protocol-2 contract **method** to return `Promise<T>`, where `T` is any existing protocol-2
result schema (including `void`). The host owns every pending call: it is admitted against a budget,
ledgered to both participants, generation-checked at settlement, delivered on a game frame into the
consumer's context, and rejected (never silently dropped) when the provider goes away.

Forwards do not change: `Notification`, `Hook` and `Transform` listeners stay synchronous, and a
thenable returned by a listener is still observed, logged and treated as Continue.

## Goals and non-goals

Goals:

- One declared, type-checked way to expose I/O-backed methods across plugins.
- No new lifetime hazards: a pending call can never deliver into a dead context, resurrect a stale
  provider generation, or leak past either plugin's unload.
- Mixed contracts: a service keeps sync methods for hot paths (permission checks inside a Transform)
  and adds async methods for authoritative operations.
- Archives that do not use the feature stay byte-for-byte compatible with existing API-3 hosts.

Non-goals (v1):

- Async Hook/Transform/Notification listeners, or async `watchOptional` attachment callbacks.
- Cancellation or progress (no `AbortSignal`; a consumer that stops caring just ignores the result).
- Streaming results, async iterators, or multiple settlements per call.
- Async methods under protocol 1.

## Contract and authoring

```ts
// api.d.ts (producer)
import type { Notification } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: {
    getBalance(steamId: string): number;                       // sync, unchanged
    grantOffline(steamId: string, amount: number): Promise<number>;
    leaderboard(limit: number): Promise<{ gangId: number; score: number }[]>;
    flush(): Promise<void>;
  };
  forwards: { OnBalanceChanged: Notification<{ steamId: string; balance: number }> };
}
export declare function grantOffline(steamId: string, amount: number): Promise<number>;
```

Rules (enforced by `packages/sdk/src/interop.ts`):

- A method result of exactly `Promise<T>` is async; `T` goes through the existing `schema()` with
  `voidAllowed = true`. Nested promises, `Promise<T> | T`, and `PromiseLike`/custom thenables are
  rejected ("async methods must return exactly Promise<T>").
- Argument rules are unchanged (including the optional-union rejection).
- The method name `then` remains reserved.
- **Producer agreement** (`checkInteropCalls`): an async contract method must be implemented by a
  function whose return type is `Promise<U>` with `U` assignable to `T`; a sync contract method
  must still be implemented synchronously (an `async` implementation of a sync method keeps failing
  exactly as today).
- Method value exports (`export declare function …`) must agree in sync/async kind.

### Metadata

`ContractMetadata.methods[name]` gains `kind: "async"` for async methods only. Sync methods omit the
field, so a contract with no async methods produces identical metadata, digest and
`typesSha256` to today. A contract that has at least one async method is emitted with metadata
`version: 2`; otherwise `version: 1`.

This is the compatibility gate: `core/src/interop.rs` currently requires `version == 1` and parses
`Method`/`Metadata` with `deny_unknown_fields`, so an existing API-3 host refuses a version-2
contract with "InterfaceContractError: unsupported metadata" at load — before any call. New hosts
accept versions 1 and 2. No host API major bump is needed; the per-contract metadata version is
the feature gate, which keeps non-async archives loadable on older API-3 hosts. (Open question 1
covers whether the loader should additionally surface a clearer "requires async interop" message.)

The canonical digest (RFC 8785 JCS) covers `kind`, so changing a method between sync and async is a
contract change that consumers must rebuild against — the same rule that already applies to forward
kinds.

### Consumer types

No new consumer API. `TypedInterfaceHandle<C>` already exposes `ContractMethods<C>`, so
`use("@x/y").grantOffline(id, 5)` is typed `Promise<number>`, and the CLI's derived direct-import
declarations follow the contract signature. `await` and `.then` work as with `fetch`.

## Runtime behavior

### Call

`s2_iface_call` (`core/src/v8host.rs`) keeps its existing preamble for async methods:
`IFACES.call_target`, `checked_contract`, `InteropGuard::enter` (recursion depth), consumer
generation via `live_interop_context`, `InterfaceUnknownMethod`, strict argument validation.

Then, for `kind: "async"`:

1. **Admission.** Reserve a lease in a new `interop` budget domain (`core/src/async_limits.rs`,
   alongside `jobs`/`timers`), keyed by the **consumer** owner `(plugin, generation)`. Exhaustion
   rejects with `AsyncQueueFull` before the provider runs. Defaults to be set in
   `docs/ASYNC_LIMITS.md` (proposal: 256 global / 32 per owner).
2. **Consumer promise.** Create a `PromiseResolver` **in the consumer's context** and register it in
   `jobs::RESOLVERS` with the consumer's owner tag (not `resolver_owner_tag()` of whatever context is
   current at settlement). This fixes the ownership caveat of provider-created promises, which are
   tagged to the provider and would be dropped by `settle_if_live` on provider unload instead of
   being rejected to the consumer.
3. **Ledger.** Record a pending call `{ id, consumer:(id,gen), provider:(id,gen), interface, method,
   resolverId, lease }` in a host table, and ledger a new `Resource::InteropCall(id)` against **both**
   plugins (`core/src/plugin.rs` `Resource`).
4. **Invoke** the provider method synchronously in the provider context, exactly as sync methods do.
   The `InteropGuard` depth covers only this synchronous portion.
5. **Inspect the return.**
   - A synchronous throw → settle the call as rejected (`InterfaceCallError "N.m: msg"`).
   - A native Promise or thenable → attach host-native fulfil/reject reactions in the provider
     context that capture only the call id.
   - A non-thenable value → treat as immediate fulfilment with that value (still delivered
     asynchronously; see Delivery). The SDK checker prevents this statically; the host tolerates it.
6. Return the consumer promise. The consumer never observes provider objects.

### Settlement

When the provider promise settles, the reaction runs in the provider context and:

- **Fulfilled:** validates the value with `strict_value`/`strict_json` + `schema.result.accepts`
  (or `undefined` for `void`) and copies it into an owned host value. Failure settles as rejected
  with `InterfaceValueNotSerializable "N.m return"`.
- **Rejected:** copies only `name` (if a string) and `message` (stringified, length-capped) into an
  `InterfaceCallError "N.m: <message>"`. No provider object crosses.
- Enqueues `{ callId, outcome }` on an `INTEROP_COMPLETIONS` queue and requests a drain.

Settlement never runs consumer code. A provider promise settling twice is impossible (V8 semantics);
a thenable that calls both callbacks is handled by first-settlement-wins on the call id.

### Delivery

`frame_async_drain` gains an `interop` source in its rotation (fairness and `frame_items`/
`frame_bytes` accounting as for the other sources). For each completion:

1. Remove the pending call; release its lease; drop both `Resource::InteropCall` ledger rows.
2. If the consumer generation is no longer live → discard silently (the same rule
   `settle_if_live` applies to timers and jobs).
3. If the provider generation is no longer live, or `IFACES.producer_of(name)` no longer equals the
   recorded `(provider, gen)` → reject with `InterfaceUnavailable` even if the provider fulfilled.
   This mirrors the existing sync return-boundary rule (`v8host.rs`, the post-call generation
   re-check) and prevents a reloaded provider's predecessor from answering for it.
4. Otherwise enter the consumer context and resolve/reject. EntityRef-capable result schemas are
   revived through the existing SDK reviver path; others are materialized from the checked owned
   value, as for sync results.

Completions are delivered no earlier than the next drain, in settlement order subject to the
rotation. A consumer therefore never sees its promise settle re-entrantly inside the provider's
stack.

### Provider and consumer lifetime

- **Provider unload or reload** (`teardown_ledger_and_dispose`, `Resource::InteropCall`): every
  pending call whose provider is the retiring generation is settled as rejected with
  `InterfaceUnavailable` and delivered on the next drain. Provider-side reactions that fire later
  find no pending call and do nothing. This is the key guarantee the current sync-only design gets
  for free and async must restore: a consumer is never left with a promise that never settles.
- **Consumer unload:** pending calls owned by the consumer are removed; their leases are released.
  The provider's work is not cancelled; its eventual reaction finds no pending call. Providers must
  not assume a caller still exists.
- **Optional providers (`watchOptional`):** calling an async method on a retained service after its
  attachment ended throws `InterfaceUnavailable` synchronously, as sync methods do today. Calls made
  during the attachment and still pending when it ends are rejected with `InterfaceUnavailable`.
- **Timeout:** optional host-wide `interop_call_timeout_ms` (default off in v1; open question 2).
  When set, an unsettled call is rejected with `InterfaceTimeout`; the provider is not notified.

### Errors (names set on the rejected `Error`)

| Name | When |
| --- | --- |
| `AsyncQueueFull` | interop budget exhausted at call time (thrown before the provider runs) |
| `InterfaceUnavailable` | provider absent at call time, retired before delivery, or attachment ended |
| `InterfaceCallError` | provider threw synchronously or its promise rejected |
| `InterfaceValueNotSerializable` | args invalid (sync throw) or fulfilled value fails the result schema |
| `InterfaceRecursionLimit` | the synchronous portion exceeded the 32-call depth |
| `InterfaceTimeout` | optional timeout elapsed |

### Identity guidance (documentation)

Nothing new at the host level, but `docs/PLUGIN_INTEROP.md` must say it plainly: a slot is only
meaningful now. Async methods should take and return SteamIDs (decimal strings), user ids or
`EntityRef`s, and consumers re-resolve after `await` (`Player.fromUserId`, `ref.isValid()`).

## Implementation map

| Area | Change |
| --- | --- |
| `packages/sdk/src/interop.ts` | detect `Promise<T>` results; `kind: "async"`; metadata `version` 2 when any async method; producer agreement for async; value-export kind agreement; new diagnostics |
| `packages/sdk/src/build.ts`, `typecheck/typecheck.ts` | no structural change; fixtures and hash-drift tests for version-2 metadata |
| `core/src/interop.rs` | `Method.kind: Option<Kind>`; accept metadata version 1 or 2; `kind` only allowed in version 2 |
| `core/src/v8host.rs` | async branch in `s2_iface_call`; `interop` source in `frame_async_drain` |
| `core/src/v8host/interop_wire.rs` | settlement reactions; owned-value copy of results and rejection messages |
| `core/src/jobs.rs` | resolver registration with an explicit owner (consumer), not the current context |
| `core/src/async_limits.rs`, `docs/ASYNC_LIMITS.md` | `interop` budget domain + defaults; `obligations()` counts pending calls |
| `core/src/plugin.rs`, `core/src/v8host/lifecycle.rs` | `Resource::InteropCall`; teardown rejects (provider) or drops (consumer) |
| `core/src/v8host/interop_lifetime.rs` | pending calls tied to an attachment are rejected when it ends |
| `docs/PLUGIN_INTEROP.md`, SDK README | authoring section, errors table, identity guidance, migration note |
| `examples/legacy-contracts` | migrate the async `workshop` contract to protocol 2 as the worked example |

## Testing and acceptance

SDK (`packages/sdk/test/interop.test.mjs` and fixtures):

- `Promise<number>`, `Promise<void>`, `Promise<Record[]>` accepted with `kind: "async"` and metadata
  version 2; contracts without async methods produce unchanged metadata and digests (golden test).
- Rejected: `Promise<Promise<T>>`, `Promise<T> | T`, `PromiseLike<T>`, `Promise<unknown>`,
  `Promise<bigint>`; async implementation of a sync method; sync implementation of an async method;
  value-export kind mismatch.
- Host/SDK shared canonical-metadata vectors updated for `kind`.

Core (`core/src/v8host/tests*`, single-threaded as today):

- Fulfil, reject, sync throw, invalid result → the documented names; result delivered only on a
  drain, never inside the provider stack.
- Provider unload with N pending calls → all N reject `InterfaceUnavailable`; late provider
  reactions are no-ops; leases and ledger rows return to baseline.
- Provider reload (new generation) between call and settlement → `InterfaceUnavailable`, even when
  the old generation fulfilled.
- Consumer unload → nothing delivered, budgets return to baseline, provider unaffected.
- Admission: per-owner and global exhaustion → `AsyncQueueFull` before the provider runs.
- `watchOptional` attachment end with pending calls → rejected; post-detach call throws.
- Old-host refusal: a version-2 contract fixture is rejected by the version-1-only validator with
  "unsupported metadata" (regression guard for the compatibility gate).
- 1,000-cycle provider reload soak with calls in flight (extends `tools/interop-acceptance`), no
  growth in resolver, ledger or budget baselines.

Live CS2 acceptance (separate gate, as for protocol 2): a DB-backed provider answering an async
method under a real map change and a provider hot-reload.

## Migration

- Additive for authors: existing contracts are untouched. A producer adds async methods by declaring
  `Promise<T>` results; this is a contract change (digest) for that interface, so producer and
  consumers rebuild together, and it is a semver **minor** for the producer if no existing method
  changed kind, **major** if one did.
- Archives using async methods require a host with this feature; older API-3 hosts refuse them at
  load with a clear error rather than misbehaving at runtime.
- Downstream follow-ups once shipped: `@gangs/api` adds async `grantPlayerOffline`, `createGang`
  (resolves after the row is written), `purchasePerk` with persisted acknowledgement, and
  `leaderboard`, while keeping its synchronous cache reads for Transform hot paths.

## Open questions

1. Is the per-contract metadata version a sufficient gate, or should the loader also refuse with a
   feature-specific message (e.g. a manifest `features: ["async-methods"]` checked before contract
   validation) so operators see "this plugin needs a newer s2script" instead of "unsupported
   metadata"?
2. Default timeout: off (consumer decides with `Promise.race`) or a host default such as 30s?
3. Should a provider be able to observe consumer departure (cooperative cancellation) in a later
   version, or is "results to departed consumers are discarded" the permanent contract?
4. Budget defaults for the `interop` domain, and whether pending interop calls should count toward
   the consumer's `jobs` budget instead of a new domain.
