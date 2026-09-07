# Typed plugin forwards (protocol 2)

Set `s2script.interfaceProtocol` to `2` in both producer and consumer packages. A producer publishes one declaration entry, selected by its package `types` field. Consumers declare `s2script.pluginDependencies` or `optionalPluginDependencies` and obtain that entry through `s2s add`. Plugin acquisition downloads registry metadata and the types artifact; it does not download the producer binary. Workspace consumers instead use the sibling producer's declaration directly, even when an older downloaded copy exists.

```json
{
  "name": "@demo/counter",
  "version": "1.0.0",
  "types": "api.d.ts",
  "main": "src/plugin.ts",
  "s2script": { "interfaceProtocol": 2, "publishes": "self" }
}
```

```ts
// api.d.ts
import type { Notification } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: { getCount(): number; setCount(count: number): void };
  forwards: { OnCountChanged: Notification<{ count: number }> };
}
```

```ts
// Producer: registrations belong to the normal plugin load window.
import { publish } from "@s2script/sdk/plugin";
let count = 0;
const counter = publish("@demo/counter", {
  getCount: () => count,
  setCount(next: number) {
    count = next;
    counter.emit("OnCountChanged", { count });
  },
});
```

```ts
// Consumer: package s2script.pluginDependencies includes "@demo/counter": "^1.0.0".
import { use } from "@s2script/sdk/plugin";
const counter = use("@demo/counter");
counter.on("OnCountChanged", event => console.log(event.count));
// Direct method imports are also derived from Contract.methods:
import { getCount } from "@demo/counter";
// Direct on imports use the same inferred payloads and disposable Subscription:
import { on } from "@demo/counter";
on("OnCountChanged", event => console.log(event.count));
```

The CLI generates `.s2script/interfaces.d.ts` for name-based inference. New scaffolds include this file in `tsconfig.json`; add it to existing projects' `include` list. Refresh it with `s2s build` after changing declarations. Explicit caller-selected generics and authored `InterfaceContracts` augmentations do not authorize protocol 2 builds. Producer method agreement is checked even without an explicit implementation annotation. Each implementation must accept every declared input and return the declared synchronous result; async methods, value-returning implementations of void methods, and narrowed input types are rejected. SDK call authority follows the resolved signature through imports, aliases, and destructuring. If the entry also declares method exports, those declarations must agree with `Contract.methods`.

## Wire values

The supported algebra is null, boolean, finite number, string, literal enums, arrays, finite object records with optional fields, discriminated object unions, and the SDK's existing `EntityRef`. Method results may also be void. All domain types must be declared in the entry file. Only SDK `Notification`, `Hook`, `Transform`, and `EntityRef` imports are supported; external domain imports, re-exports, reference directives, unbounded dictionaries, recursive types, arbitrary classes, functions, BigInt, Promise, any, and unknown are rejected. This validates the documented wire algebra, not arbitrary TypeScript.

Values cross by copy. Every field is checked before any producer method or notification listener receives the value; method results are checked before returning to the consumer. Unknown fields, present-but-undefined fields, nonfinite numbers, sparse arrays, symbols, proxies, accessors, and `toJSON` coercions do not pass the strict copy path. Omit an optional field or argument instead of passing undefined. `__s2ref` is reserved for the existing entity-reference wire envelope, which retains the host's reference/liveness checks. Carry persistent player identity as copied identifiers and a map generation; a bare retained client slot is not a stable identity.

Metadata hashes use RFC 8785 JSON Canonicalization Scheme (JCS): ECMAScript finite-number serialization and string escaping, with object keys sorted by UTF-16 code units. Lone-surrogate strings are rejected. The SDK and host share regression vectors covering decimal/exponent transitions and BMP/non-BMP key ordering. Authored names, including `__proto__`, are preserved as own dictionary entries.

Entity references must use the original SDK prototype and contain exactly its two own data fields, `index` and `id`, with numeric primitives in range. Extra own properties, symbols, accessors, and coercible values are rejected before delivery. Revival still constructs the receiving SDK reference, whose methods check host liveness.

Notification identity is the interface name plus forward name. Subscription verifies dependency declaration, the existing version-range policy, declaration hash, canonical metadata digest, and forward membership. Emission verifies the owning provider and generation. The direct native entry points perform the same checks. Exactly one provider may own an interface.

Listeners run in registration order using a snapshot. Removed or stale listeners are skipped; listeners added during a dispatch start on the next dispatch. Every listener gets a fresh copy. Exceptions are logged with provider, consumer, and forward and do not prevent later listeners or later notifications. Thenable returns are observed and logged as synchronous-contract errors. Nested method/notification crossings share a limit of 32 active calls; call 33 throws `InterfaceRecursionLimit`, and the counter unwinds after an error.

## Synchronous hooks and transforms

A hook asks listeners for a decision. A transform lets them return a shallow patch to declared writable fields. Both reuse `HookResult` from `@s2script/sdk/events` (`Continue = 0`, `Changed = 1`, `Handled = 2`, `Stop = 3`).

```ts
import type { Hook, Transform } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: {};
  forwards: {
    OnRequest: Hook<{ identity: string }>;
    OnFormat: Transform<{ identity: string; text: string }, "text">;
  };
}
```

```ts
// Consumer, during the load window:
import { use } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
const service = use("@demo/formatter");
service.on("OnRequest", event =>
  event.identity === "blocked" ? HookResult.Handled : HookResult.Continue);
service.on("OnFormat", event => ({
  result: HookResult.Changed,
  patch: { text: event.text.trim() },
}));

// Producer, using its publish handle:
const decision = formatter.dispatch("OnRequest", { identity: "guest" });
const formatted = formatter.dispatch("OnFormat", { identity: "guest", text: " hello " });
// formatted: { result: HookResultValue; payload: { identity: string; text: string } }
```

`emit` accepts notification names. `dispatch` accepts hook and transform names. Hooks return the highest listener result, defaulting to Continue. Changed alone does not mutate anything. Handled still runs later listeners; Stop ends the snapshot immediately.

Transforms return `{ result, patch? }`. A patch is allowed only with Changed and may contain only the contract's writable keys. The host validates the entire response and all patch fields before applying anything. Optional fields can be omitted; `undefined` does not remove a field. Patches replace whole field values, with no deep merge. Each listener receives a fresh copy of the updated payload, and the producer receives a fresh `{ result, payload }` copy. An empty listener set returns Continue and a copy of the original payload.

Handlers must finish synchronously. A throwing listener, invalid result, invalid patch, or thenable contributes Continue and no patch, with a log naming the provider, consumer, and forward. Promise rejections are observed. A response from a consumer whose generation expired during its callback is discarded. Later listeners still run. This is a fail-open policy: security-sensitive producers must account for it explicitly. Earlier listeners' effects are not rolled back.

Ordering is monotonic registration order within one registration history, not a priority promise across server restarts. Dispatch snapshots IDs, skips removed or stale entries, and does not hold registry borrows while invoking listeners. Nested calls share the 32-call interop bound. Provider removal during a listener stops delivery and throws `InterfaceUnavailable` at the return boundary; already-run effects remain. `on` returns a consumer-owned `Subscription`; ignoring the return remains valid. Registrations belong to the normal load window or the synchronous optional-provider attachment described below.

Forward kind and, for transforms, sorted writable keys are part of canonical metadata and its compatibility digest. Changing either requires compatible producer and consumer contracts even if their payload fields stay identical.

## Ownership and optional providers

Every `on` returns an idempotent `Subscription` with `dispose(): void`. Disposal removes that exact registration, even when the same handler is subscribed twice. Disposing before the buffered registration arms cancels it. Disposing during dispatch makes the remaining snapshot skip the removed entry. The host also ledgers subscriptions to the consumer and removes them automatically when either participant unloads.

Declare an optional provider under `s2script.optionalPluginDependencies`, acquire its contract using the existing types-only `s2s add` workflow, and register a watch during the normal load window:

```ts
import { watchOptional } from "@s2script/sdk/plugin";
const watch = watchOptional("@demo/counter", (counter, scope) => {
  scope.own(counter.on("OnCountChanged", event => console.log(event.count)));
  // Any local disposable can belong to this attachment.
  scope.own({ dispose() { console.log("counter attachment ended"); } });
});
// Calling watch.dispose() later removes the watch and its current attachment.
```

The host calls `attach` at a lifecycle boundary after both plugins become Active, once per compatible provider identity and generation. Publication never recursively invokes an attachment. A watch remains pending while the provider is absent. Version, declaration hash, and canonical wire metadata must all agree. An incompatible provider logs a diagnostic and does not attach; a later compatible generation can attach normally. `tryUse` remains a one-time optional lookup and does not track availability.

The supplied service permits `on` registration only during this synchronous attachment callback. The host checks its attachment token, consumer origin, provider generation, and crossing depth. Saved handles and scopes cannot reopen registration after the callback, during a nested interop callback, or during a later attachment. Normal load APIs stay closed. Methods on a retained service throw `InterfaceUnavailable` once its attachment ends, including after the provider reloads.

`scope.own<T extends { dispose(): void }>(resource: T): T` returns its input and retains its disposer for this attachment. Forward subscriptions created through the supplied service are automatically attachment-owned, including when `scope.own` is omitted. If attachment throws or returns a thenable, all partial subscriptions and owned disposables are retired; other watches continue. The SDK rejects statically visible async/thenable attachment callbacks, and the host observes thenables without extending registration authorization. Failed attempts retry only for a new compatible provider generation.

Provider removal disposes the attachment locally; host teardown never calls provider methods to unsubscribe. Consumer removal disposes its watches and attachments before dropping the context. Watch disposal also releases its availability callback and registry row. Teardown invalidates the attachment and subscriptions before running custom disposers in reverse ownership order; a throwing or reentrant disposer cannot prevent the remaining cleanup. Keep disposers synchronous and local. Hard dependencies preserve the existing reverse-dependency unload order.

## Archive and migration contract

SDK builds now stamp host API `3.x`. Protocol 2 archives carry `interfaceProtocol: 2`, a `contract` containing metadata format version 1 and its SHA-256 digest in each `publishes` entry, plus dependency-keyed `interfaceContracts`. `typesSha256` and `compiledAgainst` hash the exact resolved declaration bytes. Canonical metadata sorts object keys recursively and preserves array order. Downloaded copies carrying a receipt are rechecked against that receipt before building.

The API 3 host explicitly accepts API 2 protocol 1 archives. Their existing generic types and permissive wire behavior are retained. Protocol 2 requires API 3 and complete valid metadata; an archive cannot evade the requirement by declaring API 2. Old API 2 hosts reject newly built API 3 archives. Library acquisition and `.s2lib` bundling retain their existing workflow.

Protocol 1 interfaces remain available while a producer and its callers migrate together. Rebuild and deploy both ends with protocol 2; a protocol 1 consumer cannot silently bind to a protocol 2 provider without verified metadata. Interface version matching retains the repository's existing major-based range policy in this slice; exact declaration and metadata hashes provide the additional compatibility checks. Named binding helpers are a separate slice.

## Zones migration

`@s2script/zones` version `1.0.0` is the first base plugin using protocol 2. Its
seven methods and existing `enter`, `leave`, `stay`, `created`, and `deleted`
notification names and payload fields remain the same. The major contract bump
requires consumers to migrate atomically: opt into protocol 2, refresh the
verified declaration, use `^1.0.0`, and remove caller-selected generics. The CLI
derives named method exports and a typed `on` export returning `Subscription`.

Getters describe current state. `created` reports creates/replacements, operator
imports/editor saves, and map DB loads after publication; `deleted` reports
explicit deletions and cleared map zones. Initial startup loads precede
publication and are never replayed. The cookbook uses `watchOptional`, owns its
five subscriptions, and calls `getZones()` after subscribing on every attachment.
This also discovers a replacement provider's current layout after reload.

`enter` and `leave` describe engine boundary crossings. `stay` fires every eighth
game frame for the same connection that entered. Disconnect/map cleanup does not
invent leave events. Resolve `Player.fromUserId(event.userId)` when acting on an
event; a copied slot is not connection identity. Occupancy is cleared on map
change, and delayed previous-map DB loads cannot create zones in the new map.

Protocol selection applies to the whole plugin. The cookbook's existing optional
econ/workshop recipes therefore remain protocol 1 in the
[legacy-contracts companion](../examples/legacy-contracts/README.md); their
asynchronous community contracts and command behavior are preserved.

Compiled contract/build tests and plugin VM operation/lifecycle tests cover this
migration. Real CS2 enter/leave and provider reload acceptance remain a separate
live gate; offline tests do not establish that acceptance.

## BaseComm service

`@s2script/basecomm` version `1.0.0` publishes mute and gag policy through protocol 2.
Its methods are `isMuted`, `isGagged`, `setMuted`, and `setGagged`; its notifications
are `OnClientMuteChanged` and `OnClientGagChanged`, each carrying a copied
`{ steamId: string; state: boolean }` payload.

SteamIDs at the service boundary must be canonical, nonzero decimal unsigned 64-bit
values. Queries return false for invalid identities. A setter returns false for an invalid
identity or nonboolean state. Otherwise its return describes whether current policy equals
the requested state when the call returns, including an unchanged request. Reentrant
notification listeners may make an outer setter return false by selecting a different final
state. Notifications fire only for actual policy transitions, after policy and current live
engine state have changed, so listener queries see the emitted state.

Policy is SteamID-keyed and can be set while a player is offline. Reconnect reapplies both
the real `Client.voiceMuted` flag and the scoreboard communication-abuse flag. The mute
result describes BaseComm policy; if the host voice descriptor is degraded, it does not claim
that audio delivery changed. Command and menu paths share the same operations while retaining
their admin permissions, immunity filters, target handling, and translated replies. Direct
service callers are trusted plugins and bypass those command-layer checks.

The [interop observer example](../examples/interop-observer/README.md) declares BaseComm as
optional, owns its subscriptions inside the synchronous attachment, and queries current state
after subscribing on each provider generation. Its checked-in declaration is the verified
types-only copy used by `s2s add`; building the consumer does not require BaseComm source or a
provider archive. Live mute/gag, authenticated command/menu behavior, and provider reload remain
separate CS2 acceptance gates; the offline VM and compiler tests do not establish them.
