# Typed Plugin Interoperability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make plugin services strongly typed and extensible while retaining types-only dependency acquisition and safe reload behavior.

**Architecture:** Extend the current interface registry with opt-in protocol-2 contracts, generated wire metadata, typed notification/hook/transform dispatch and owned subscriptions. Prove the design in shipped plugins before adding explicit named-handler bindings.

**Tech Stack:** TypeScript compiler API, Node test runner, Rust, embedded V8, existing resource ledger, Linux CS2 acceptance server.

**Spec:** [2026-09-06-plugin-interop-design.md](../specs/2026-09-06-plugin-interop-design.md). Read both documents and CLAUDE.md before any implementation.

## Global Constraints

- Preserve `s2s add`'s types-only plugin dependency workflow.
- Identity is `(published interface name, forward name)`.
- Exactly one live provider may own an interface.
- No registry borrow may remain held while executing JS.
- Bound nested dispatch to 32 active interop calls per isolate.
- Explicit subscriptions are primary.
- No implicit whole-module export scanning in this implementation.
- Existing protocol-1 archives retain their behavior until rebuilt/migrated.
- New protocol-2 paths have no ambient-any fallback and cannot select an unrelated contract through a caller-chosen generic.
- Each slice is one complete branch/PR containing SDK, runtime, CLI and caller changes needed for its feature.
- Missing infrastructure is an explicit pending gate, not a pass.

## Stack base and publication

Inspected 2026-09-06 after `git fetch origin`. The existing runtime/UI stack has 17 PRs, #186 through #202. Its top is PR #202, branch `codex/ui-07-acceptance`, commit `e9f4177`. This planning branch is `codex/interop-00-plan`, based directly on that tip. Do not rebase or renumber the existing stack to publish this plan. The user's original checkout contains unrelated untracked UI planning documents; leave them untouched.

Create implementation branches only when a slice starts, from the completed predecessor below. Do not publish empty implementation PRs. Each new PR targets its immediate parent and links this plan. These are complete feature slices, not separate types/runtime/caller PRs.

1. `codex/interop-01-typed-notifications` from `codex/interop-00-plan`.
2. `codex/interop-02-hook-transforms` from 01.
3. `codex/interop-03-provider-lifetimes` from 02.
4. `codex/interop-04-zones-contract` from 03.
5. `codex/interop-05-basecomm-service` from 04.
6. `codex/interop-06-basebans-service` from 05.
7. `codex/interop-07-named-bindings` from 06.
8. `codex/interop-08-live-acceptance` from 07.

Before starting, fetch and check whether #202 or any predecessor changed. Rebase the planning branch once onto the new top if necessary, then build upward. During review, propagate changes bottom-up using `git rebase --onto <new-parent> <recorded-old-parent> <child>`, test, and push with `--force-with-lease` only for owned branches. After a squash merge, use the recorded old parent tip to avoid replaying merged ancestor commits. Retarget children as parents merge. Never rewrite a branch another worker is editing.

## Subagent execution contract

The coordinator owns branch creation, commits, cross-file integration, PR descriptions, and final acceptance. Dispatch a fresh implementation subagent per task with both documents, exact base SHA, owned paths, and acceptance criteria. Within a slice, independent test-fixture preparation may run alongside implementation; writers must use separate worktrees or disjoint files. Runtime registry and V8 dispatch changes are sequential, not competing edits to shared state. Service implementations can be prepared independently only after slice 03's API freezes, then integrated in stack order.

Each task requires implementation review against the spec and a separate code-quality/safety review before publication. Reviewers return concrete findings with paths and tests. The coordinator resolves findings, reruns affected gates, and records commit SHAs and results here. Planning completion is not implementation approval evidence or a passing runtime test.

## File ownership map

- SDK contract types: `packages/sdk/interfaces.d.ts`, `plugin.d.ts`, `index.d.ts`; preserve existing exports.
- Contract resolution: `packages/sdk/src/contracts.ts`, `src/typecheck/typecheck.ts`, `src/workspace/interfaces.ts`, `src/workspace/siblings.ts`.
- New compiler unit: `packages/sdk/src/interop-contracts.ts` for protocol-2 extraction, canonical metadata and generated type-map production.
- Packaging: `packages/sdk/src/build.ts`, `src/registry/add.ts`, `src/registry/deploy.ts`, `src/types-pack.ts`, `src/publishes.ts`, `src/publish-gate.ts`, `src/publish-scan.ts`, and `core/src/loader.rs` Manifest/PublishSpec parsing. Keep registration in these existing owners rather than introducing a parallel manifest validator.
- Pure runtime registry: `core/src/interfaces.rs`; protocol parsing/loading: `core/src/loader.rs`, `core/src/loader_worker.rs` where preparation belongs.
- V8 dispatch: `core/src/v8host.rs`, `core/src/v8host/lifecycle.rs`, `core/src/v8host/natives.rs`, `core/js/prelude.js`.
- Tests: `packages/sdk/test/interop-contracts.test.mjs` (new), `packages/sdk/test/fixtures/interop/` (new), existing build/typecheck/registry/workspace tests, `core/src/v8host/tests.rs`, pure registry tests beside `interfaces.rs`.
- Integration: `plugins/zones`, `plugins/basecomm`, `plugins/basebans`, verified copies and consumers under `examples/`.

## Task 1 / slice 01: Typed notifications end to end

**Produces:** protocol-2 metadata, inferred interface handles, validated synchronous methods and notifications, compatible protocol-1 path.

The following is the author-visible target (type helper implementation must preserve payload inference without exposing any):

```ts
// Producer api.d.ts; imported types must belong to the supported wire algebra.
import type { Notification } from "@s2script/sdk/interfaces";
export interface Contract {
  methods: { getCount(): number };
  forwards: { OnCountChanged: Notification<{ count: number }> };
}

// Consumer; CLI generates the interface-name-to-Contract association.
const counter = use("@demo/counter");
counter.on("OnCountChanged", event => console.log(event.count));
// Producer; implementation is checked against Contract.methods.
const producer = publish("@demo/counter", { getCount: () => 1 });
producer.emit("OnCountChanged", { count: 1 });
```

- [ ] Add valid and invalid fixture pairs in `test/fixtures/interop/`: misspelled name, missing field, wrong field type, wrong provider name, missing producer method, wrong method result, missing verified copy, mismatched sibling contract. Compile negative fixtures separately and assert diagnostic location/message, not just a nonzero exit. Example compile assertions:

```ts
const counter = use("@demo/counter");
counter.on("OnCountChanged", e => { const n: number = e.count; });
// @ts-expect-error undeclared event
counter.on("OnCountChangd", () => {});
// @ts-expect-error wrong emitted payload
publish("@demo/counter", { getCount: () => 1 }).emit("OnCountChanged", { count: "1" });
```

- [ ] Run `cd packages/sdk && npm test`; verify fixtures fail for the missing protocol support before implementation.
- [ ] Implement type extraction with the TypeScript checker, not regexes. Generate a dependency-keyed InterfaceContracts augmentation from authoritative resolved contracts. Reject unresolved/unsupported wire types and generic-name mismatch. Require self-contained authored domain declarations; permit only supported SDK helper/ref imports governed by API and metadata versions. Reject an imported domain helper fixture even if its entry-file bytes are unchanged. Extend publish-scan for watchOptional/bindForwards and imported aliases; extend publish-gate from presence checks to implementation agreement. Preserve workspace sibling precedence and hash exactly the resolved declaration artifact. Ensure producer methods are checked even if the author omits an annotation.
- [ ] Emit canonical metadata version 1 with deterministic key ordering, SHA-256 digest, method schemas, notification payload schemas, and interfaceProtocol 2. Trace manifest schema consumers and update archive/load validation atomically; bump the minimum host API version using the next valid repository version. Older archives remain accepted under protocol 1. Protocol 2 rejects missing metadata/digests; add malformed archive tests.
- [ ] Add runtime notification tests using a producer and two consumers. Test same event name under two interfaces, undeclared and wrong-hash subscription rejection, serializable payload copying, consumer exceptions, provider impersonation, stale generations, unknown events, invalid nested fields, nonfinite numbers, unsupported JSON values, thenable listeners, recursive dispatch depth 32/33 and reentrant calls. Assert no later listener receives a malformed payload and exceptions do not stop valid subsequent notifications.
- [ ] Implement protocol-2 method/notification validation in the current registry/native paths. Enforce declared dependency, version/hash, and forward membership at subscription; enforce invoking producer identity/generation at emission. Route direct native calls through these checks as well as SDK calls. Check schema before JS side effects; validate method results before returning. Preserve existing EntityRef revival, liveness and ledger invariants. Avoid claiming arbitrary TypeScript types can be validated.
- [ ] Test types-only acquisition with a fake registry that records requested URLs: `s2s add` must request metadata/types only and never a plugin archive. Build a consumer with no producer binary/source present; test publish/download round trip, self-contained declaration enforcement and stale hash refusal. Retain library `.s2lib` behavior unchanged.
- [ ] Run SDK suite and native tests (`cargo test -p s2script-core` on supported Linux), then `bash scripts/ci-js.sh` and `bash scripts/ci-native.sh`. Add SDK changeset and docs explaining migration. Commit/publish one atomic slice, with actual gate results.

## Task 2 / slice 02: Decision and transformation forwards

**Consumes:** protocol-2 metadata and validated dispatch. **Produces:** `Hook<P>`, `Transform<P,W>`, typed `dispatch(name,payload)` results. Notification names are rejected by dispatch; hook/transform names are rejected by emit.

```ts
export interface Contract {
  methods: {};
  forwards: {
    OnRequest: Hook<{ identity: string }>;
    OnFormat: Transform<{ identity: string; text: string }, "text">;
  };
}
// Hook handler returns HookResult. Transform handler returns this shape:
const change = { result: HookResult.Changed, patch: { text: "updated" } };
// dispatch(OnRequest, ...) -> HookResult
// dispatch(OnFormat, ...) -> { result: HookResult; payload: { identity: string; text: string } }
```

- [ ] Add compile fixtures for illegal result/patch keys and async hook handlers. Add runtime cases for Continue/Changed/Handled/Stop, invalid numeric actions, exceptions, rejected Promises and no listeners.
- [ ] Implement monotonic registration sequence and snapshot traversal. Reuse HookResult values/collapse conventions from core/src/multiplexer.rs and core/src/channels.rs; do not introduce competing action enums or expose monitor/priority behavior in this slice. Write the core algorithm as: validate input; snapshot IDs; for each live ID invoke with fresh copied payload; validate response; conditionally apply patch; collapse maximum result; break on Stop; return result and final payload. Release all RefCell borrows before each callback.
- [ ] Prove with listeners A/B/C that B sees A's accepted text patch, identity cannot change, a rejected patch applies nothing, Handled still reaches C and Stop does not. Test internal registry removal during dispatch, nested calls, and provider unload during a listener. Public Subscription.dispose and late scoped-registration tests belong to slice 03; do not introduce an unsealed registration path for these tests.
- [ ] Extend canonical metadata to include kind/writable keys, with a hash mismatch test where payload fields stay identical but kind changes. Runtime listener failures use the exact policy in the spec.
- [ ] Run SDK/native suites and both CI scripts. Document sync-only semantics and ordering limits, add changeset, review and commit the whole slice.

## Task 3 / slice 03: Owned subscriptions and optional provider attachments

**Consumes:** typed registry dispatch. **Produces:** `Subscription.dispose(): void`, `watchOptional(name, attach): Subscription`, and attachment scope with `own<T extends { dispose(): void }>(resource: T): T`.

```ts
watchOptional("@demo/counter", (counter, scope) => {
  scope.own(counter.on("OnCountChanged", e => console.log(e.count)));
});
```

- [ ] Write registry/V8 tests for consumer-before-provider, provider-before-consumer, provider removal/reload, consumer unload, incompatible provider, duplicate publication, attachment exception, disposal during dispatch and repeated disposal.
- [ ] Implement availability watch bookkeeping independent of published entries; provider removal must not lose pending watchers. Queue attachment after Active at a safe lifecycle boundary. Explicitly ledger V8 callbacks and indexes so disposing a watch releases both. Dispose by subscription ID, not the legacy off(name,event,handler) bulk removal. Preserve before-isolate-drop clearing and generation-tagged release in core/src/v8host/lifecycle.rs and Resource entries in core/src/plugin.rs.
- [ ] Allow registrations during a host-authorized attachment scope, not arbitrary runtime registration. Teardown attachment locally without calling unavailable provider methods. Failed attachment clears every partial resource; retry only on a new compatible generation.
- [ ] Add a churn test with 1,000 provider attach/detach cycles. Assert registry rows, V8 callback maps, pending attachments and ledger counts return to baseline; exactly one notification per active attachment. Preserve hard-dependency reverse-unload behavior.
- [ ] Run SDK/native tests and gates; document ownership and optional declaration requirements; review and commit.

## Task 4 / slice 04: Zones as the first real typed provider

**Files:** `plugins/zones/api.d.ts`, `plugins/zones/src/plugin.ts`, its package manifest, `examples/cookbook/.s2script/types/@s2script/zones/index.d.ts`, zone recipes, `packages/sdk/test/zones-contract.test.mjs`.

**Produces:** protocol-2 zones methods and typed existing enter/leave/stay/created/deleted notifications. Keep names and meanings; bump published contract version according to repository policy.

- [ ] Replace regex-only signature expectations with consumer compilation covering each payload. Preserve byte-copy verification and sibling precedence tests.
- [ ] Declare Contract and migrate producer handle to inferred publication. Generate/check producer-as-import declarations so existing import style stays useful. Update every in-repo caller in this same slice; locate them with `rg '@s2script/zones' examples plugins packages`.
- [ ] Test created/deleted notifications through actual operation paths, existing-zone queries after late attachment, and disconnect/map-change identity handling. Getters describe current state; notifications are not historical replay.
- [ ] Build zones and examples, run SDK suite and JS gate, then validate real enter/leave and reload on CS2. Record pending live evidence if unavailable; review/commit without claiming acceptance.

## Task 5 / slice 05: Basecomm public service

**Files:** create `plugins/basecomm/api.d.ts`; modify `plugins/basecomm/src/plugin.ts` and manifest; create `examples/interop-observer/` with package, plugin, verified contracts and README; add SDK integration tests.

**Produces:** `isMuted(steamId): boolean`, `isGagged(steamId): boolean`, `setMuted(steamId, state): boolean`, `setGagged(steamId,state): boolean`; notifications `OnClientMuteChanged` and `OnClientGagChanged` with `{ steamId: string; state: boolean }`. State is the basecomm policy state, not a claim of audio delivery. Existing command permissions remain enforced by commands; document that service callers are trusted plugins.

- [ ] Write integration tests for command/menu/API convergence and no notification for unchanged state. Verify silence changes both properties through the same setters.
- [ ] Publish protocol-2 contract and route all mutation paths through one implementation per property, preserving existing engine updates and reconnect behavior. Emit only after the policy state changes.
- [ ] Add observer example using optional attachment, initial state query and subsequent notifications. Verify it builds using only copied types without basecomm source or binary in its project.
- [ ] Run SDK tests, plugin typecheck/build gate and live mute/gag/API/reload checks. Document limitations and version contract, review/commit.

## Task 6 / slice 06: Basebans public operations and hooks

**Files:** create `plugins/basebans/api.d.ts`; modify `plugins/basebans/src/plugin.ts`, manifest, observer example and basebans tests; inspect `packages/sdk/bans.d.ts` and native Bans implementation before wiring outcomes.

**Produces:** `ban(request): BanResult`, `unban(request): boolean`; `OnBanRequested: Hook<BanRequest>`, `OnBanRecorded: Notification<BanRecord>`, `OnBanRemoved: Notification<{ steamId: string }>`.

```ts
interface BanRequest {
  steamId: string;
  minutes: number;
  reason: string;
  source: "command" | "menu" | "plugin";
  actorSteamId: string | null; // null includes server console
}
interface BanResult { recorded: boolean; result: HookResult }
interface BanRecord { request: BanRequest; until: number }
```

- [ ] Write tests for validation, hook Continue/Handled/Stop, actual Bans failure behavior, console actor, offline ban, command/menu/API routing, unban no-op and a player disconnecting/reusing its slot during callbacks.
- [ ] Validate request first. Dispatch request hook; Handled/Stop suppress the default record and kick, returning recorded false. Otherwise update Bans, verify the documented store result, emit OnBanRecorded once and perform the existing kick policy after re-resolving player identity. Changed alone has no patch effect. Never treat request interception as successful recording.
- [ ] Preserve command target/immunity/permission checks outside the shared operation and document trusted API callers. Route sm_ban, sm_addban and menu through the operation. Keep reconnect enforcement separate; it must not emit a new-ban event.
- [ ] Explain that OnBanRecorded describes the store's actual observable guarantee, not durable disk acknowledgement unless the native API proves it. Add a fixture store failure case matching actual behavior; do not invent success flags around a void native.
- [ ] Run SDK/JS gates and live command/menu/API/offline/reconnect checks, review/commit with contract and example updates.

## Task 7 / slice 07: Explicit named-handler bindings

**Files:** SDK interfaces/plugin declarations, prelude binding helper, new compile fixtures and runtime tests; `packages/sdk/src/interop-contracts.ts` if build-time extraction needs extension.

**Produces:** `bindForwards(name, handlers): Subscription`, keys checked against the chosen provider; each handler receives the exact payload/result type. This is a typed registration map, not automatic export discovery.

```ts
bindForwards("@demo/racing", { OnRunFinished: OnRaceFinished });
bindForwards("@demo/parkour", { OnRunFinished: OnParkourFinished });
export function OnRaceFinished(e: RaceResult): void { console.log(e.elapsedMs); }
export function OnParkourFinished(e: ParkourResult): void { console.log(e.checkpoints); }
```

- [ ] Add two-provider fixtures with identical forward names and different payloads. Assert swapped handlers, unknown keys and async decision handlers fail compilation. Test whole-map disposal and provider isolation at runtime.
- [ ] Implement binding through the existing on machinery under the normal load window. If any binding fails, dispose all registrations created by this call before throwing. No new global callback registry or broad export scan.
- [ ] Update example/readme showing .on for inferred inline callbacks and binding aliases for named exports. Build/test/gate, review/commit.

## Task 8 / slice 08: Integrated acceptance and documentation

**Files:** new `tools/interop-acceptance/` plugin fixtures and README; new `scripts/test-interop.sh`; integrate appropriate checks into `scripts/ci-js.sh` and `scripts/ci-native.sh`; update `docs/ARCHITECTURE.md`, `docs/PROGRESS.md`, SDK README and example index.

- [ ] Build a producer plus two consumers with same-named forwards across two interfaces, notification/hook/transform probes, optional-provider restart and explicit binding aliases. Expose a server command producing a deterministic JSON summary of counts, actions and final payloads; include expected values in the fixture README.
- [ ] Test registry add/build with no producer archive in the consumer tree. Test stale/missing contracts and old-host protocol refusal. Verify protocol-1 archives still load under documented compatibility.
- [ ] On Linux, run `npm ci`, SDK tests, `bash scripts/ci-js.sh`, `bash scripts/ci-native.sh`. Build deployable native artifacts via `scripts/build-sniper.sh`, package with `scripts/package-addon.sh`, and use the authorized test-server workflow in docs/BUILDING.md. Never deploy host-built binaries to CS2.
- [ ] Run notification isolation, hook collapse, transform copyback, 1,000 attachment cycles, consumer/provider unload, map change, malformed value and recursion-limit scenarios on CS2. Record binary/build identity, branch SHA, commands, output, and resource baselines. Assert no duplicate callbacks, stale player actions or surviving subscription growth.
- [ ] Check basecomm and basebans through commands, menus and inter-plugin calls; record any human-only checks separately. Extend existing CI scripts rather than creating divergent workflows.
- [ ] Update architecture and CLI docs with supported wire types, lifecycle/error semantics, migration, named-binding examples and types-only workflow. Run `git diff --check`, review final stack and publish the final acceptance PR with honest outstanding gates.

## Planning self-review / execution status

- [x] Base verified at PR #202 / e9f4177; implementation stack extends its tip.
- [x] Type-only acquisition and sibling contract resolution retained.
- [x] Runtime metadata explicitly generated; erased types are not treated as runtime validation.
- [x] Notification, decision, transformation, provider lifecycle, service adoption and named binding tasks assigned.
- [x] Broad global-name discovery and new mapchooser implementation excluded explicitly.
- [ ] Slices 01–08 implemented, reviewed and accepted (future work).

This document is the handoff to subagent execution. The current task publishes the plan/spec only; it does not start those implementation slices.
