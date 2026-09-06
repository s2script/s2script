# Typed plugin notifications (protocol 2)

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
```

The CLI generates `.s2script/interfaces.d.ts` for name-based inference. New scaffolds include this file in `tsconfig.json`; add it to existing projects' `include` list. Refresh it with `s2s build` after changing declarations. Explicit caller-selected generics and authored `InterfaceContracts` augmentations do not authorize protocol 2 builds. Producer method agreement is checked even without an explicit implementation annotation. If the entry also declares method exports, those declarations must agree with `Contract.methods`.

## Wire values

The supported algebra is null, boolean, finite number, string, literal enums, arrays, finite object records with optional fields, discriminated object unions, and the SDK's existing `EntityRef`. Method results may also be void. All domain types must be declared in the entry file. Only SDK `Notification` and `EntityRef` imports are supported; external domain imports, re-exports, reference directives, unbounded dictionaries, recursive types, arbitrary classes, functions, BigInt, Promise, any, and unknown are rejected. This validates the documented wire algebra, not arbitrary TypeScript.

Values cross by copy. Every field is checked before any producer method or notification listener receives the value; method results are checked before returning to the consumer. Unknown fields, present-but-undefined fields, nonfinite numbers, sparse arrays, symbols, proxies, accessors, and `toJSON` coercions do not pass the strict copy path. Omit an optional field or argument instead of passing undefined. `__s2ref` is reserved for the existing entity-reference wire envelope, which retains the host's reference/liveness checks. Carry persistent player identity as copied identifiers and a map generation; a bare retained client slot is not a stable identity.

Notification identity is the interface name plus forward name. Subscription verifies dependency declaration, the existing version-range policy, declaration hash, canonical metadata digest, and forward membership. Emission verifies the owning provider and generation. The direct native entry points perform the same checks. Exactly one provider may own an interface.

Listeners run in registration order using a snapshot. Removed or stale listeners are skipped; listeners added during a dispatch start on the next dispatch. Every listener gets a fresh copy. Exceptions are logged with provider, consumer, and forward and do not prevent later listeners or later notifications. Thenable returns are observed and logged as synchronous-contract errors. Nested method/notification crossings share a limit of 32 active calls; call 33 throws `InterfaceRecursionLimit`, and the counter unwinds after an error.

## Archive and migration contract

SDK builds now stamp host API `3.x`. Protocol 2 archives carry `interfaceProtocol: 2`, a `contract` containing metadata format version 1 and its SHA-256 digest in each `publishes` entry, plus dependency-keyed `interfaceContracts`. `typesSha256` and `compiledAgainst` hash the exact resolved declaration bytes. Canonical metadata sorts object keys recursively and preserves array order. Downloaded copies carrying a receipt are rechecked against that receipt before building.

The API 3 host explicitly accepts API 2 protocol 1 archives. Their existing generic types and permissive wire behavior are retained. Protocol 2 requires API 3 and complete valid metadata; an archive cannot evade the requirement by declaring API 2. Old API 2 hosts reject newly built API 3 archives. Library acquisition and `.s2lib` bundling retain their existing workflow.

Protocol 1 interfaces remain available while a producer and its callers migrate together. Rebuild and deploy both ends with protocol 2; a protocol 1 consumer cannot silently bind to a protocol 2 provider without verified metadata. Interface version matching retains the repository's existing major-based range policy in this slice; exact declaration and metadata hashes provide the additional compatibility checks. Optional watches, hooks, transforms, and named binding helpers are separate slices.
