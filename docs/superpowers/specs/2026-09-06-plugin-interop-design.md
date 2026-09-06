# Typed plugin interoperability

Status: design for implementation; no runtime behavior is implemented by this document.

## Intent

A plugin is a service other plugins can call, observe, and extend. Preserve `s2s add`'s types-only plugin dependency workflow. Extend the existing interface registry rather than introducing a global event bus. The initial proofs use zones, basecomm, and basebans; mapchooser in the discussion was illustrative and does not exist in this tree.

## Contract and authoring

Introduce an opt-in `interfaceProtocol: 2` on published interfaces and require protocol-2 imports to carry verified contracts. Existing protocol-1 archives retain their behavior until rebuilt/migrated. New protocol-2 paths have no ambient-any fallback and cannot select an unrelated contract through a caller-chosen generic.

A producer's single declaration entry point exports `Contract`, with `methods` and `forwards` members. A forward descriptor is type-only: `Notification<P>`, `Hook<P>`, or `Transform<P, W>`, where W is a union of writable keys of object payload P. Each forward takes one payload. Reuse the existing `HookResult` enum. SDK helpers and host implementations live in the SDK/runtime, never in the dependency artifact.

The CLI resolves each declared interface name to its verified declaration entry point and generates a local `InterfaceContracts` module augmentation. `use("@provider/service")`, `publish("@provider/service", implementation)`, and `bindForwards("@provider/service", handlers)` infer from that map. The CLI validates calls and producer agreement using the TypeScript checker; an arbitrary generic or forged local augmentation is not authority for the emitted manifest. Keep existing producer-as-import support, generating/checking the corresponding module surface against Contract.

Protocol 2 includes CLI-generated canonical wire metadata in the archive manifest. It describes method arguments/results and forward names, kind, payload shape, and writable keys. It is derived from the same resolved contract bytes used for `compiledAgainst`, not handwritten beside the implementation. Include a metadata format version and canonical metadata digest in compatibility checks. Old hosts must refuse archives requiring the new protocol using the repository's existing host API-version mechanism; determine the next valid API version from the implementation base before changing constants.

The supported wire algebra is deliberately bounded: null, boolean, finite number, string, literal enums, arrays, finite object records with optional fields, discriminated unions, and the existing EntityRef encoding. Void is allowed for method results. Reject any, unknown, functions, BigInt, unbounded dictionaries, recursive types, arbitrary class instances, and Promise in protocol-2 contracts. Model player identity as copied steamId/userId/map-generation data or existing validated refs, never a bare slot retained across time. Do not invent a ClientRef public type. Protocol 2 requires one self-contained authored declaration entry point: all domain payload types are declared in that file. External declaration imports/re-exports are rejected except the SDK descriptor helpers and supported EntityRef type. Those SDK definitions are governed by the minimum host API version and canonical metadata version/digest. Local type aliases resolve through the checker. Hash the exact entry bytes, whether obtained from a workspace sibling or downloaded verified copy; no unhashed domain declaration closure is permitted.

## Dispatch

Identity is `(published interface name, forward name)`. Exactly one live provider may own an interface. Provider identity/generation and consumer identity/generation are checked at every crossing. Registering or emitting unknown forwards and invalid wire values yields a named error; no silent JSON omission/coercion of unsupported values.

Notifications return void. Listener errors are logged with provider, consumer, and forward; later listeners still run. An async function may satisfy TypeScript's void callback convention, so runtime thenables must be detected, rejection observed, and a synchronous-contract error logged; async results never influence dispatch.

Hooks return the highest HookResult, default Continue; Stop ends dispatch. Handled does not end dispatch. Changed does not imply mutation. A throwing/invalid listener contributes Continue and logs an error. Consumers of security-sensitive operations must explicitly consider that error policy; this mechanism does not promise transactional rollback.

Transforms return `{ result: HookResult; patch?: Partial<Pick<P, W>> }`. Validate the entire response before applying anything. Apply a shallow patch only when result is Changed; a patch with any other result is invalid. Pass a fresh copy of the updated payload to subsequent listeners. Return `{ result, payload }` to the producer. Highest result wins and Stop ends dispatch. A failed listener contributes no patch and Continue. Each contract lists writable fields; identity is immutable unless deliberately included. No deep-merge or shared JavaScript references.

Dispatch order is monotonically assigned registration order, not HashMap iteration. Snapshot subscription IDs at dispatch start, recheck liveness before each call, skip disposed entries, and defer newly registered listeners until the next dispatch. No registry borrow may remain held while executing JS. Bound nested dispatch to 32 active interop calls per isolate; exceeding the bound throws a named error. This order is deterministic within a registration history, not a promised priority across server restarts.

## Lifetime and optional providers

`on` returns an idempotent disposable subscription ledgered to the consumer. Existing code ignoring the return remains valid. `watchOptional(name, (service, scope) => void)` is declared during the normal load window and requires an optional dependency declaration. The host activates it after both participants are Active, once per compatible provider generation. The supplied scope owns registrations created during attachment and permits subscription registration for this callback even though the original load window is over.

Provider removal disposes attachment resources without executing provider methods; consumer removal disposes the watch and attachment. Reappearance schedules a fresh attachment at a safe lifecycle boundary, not recursively inside publication. Existing hard dependencies retain their reverse-dependency lifecycle rules. Incompatible providers do not attach and expose a diagnostic. Callback failure rolls back its scope; no automatic retry until another provider generation. Multiple providers for one interface remain an error.

## Named handlers

Explicit subscriptions are primary. Add explicit typed `bindForwards(name, handlers)` after the lifecycle API is proven. Handler keys are provider forward names; values are local functions, which may also be exported under any local name. Binding uses the same subscription machinery and load-window policy. Reject unknown keys and incompatible signatures. Two providers may expose OnRunFinished without collision because bindings retain provider identity. No implicit whole-module export scanning in this implementation: automatic bare-name matching is deferred, eliminating an unnecessary ambiguity policy and preserving type inference through explicit object bindings.

## Adoption and compatibility

Migrate first-party callers atomically with each producer. Zones retains its existing event names and meanings; stronger typing does not require a naming-only break. Basecomm publishes state queries/setters and mute/gag notifications through shared operation paths. Basebans publishes one shared operation path for command, menu, and public method calls, with request hooks and successful-state notifications; document persistence failure and kick semantics from actual Bans implementation before promising success. Never claim a disk commit from an in-memory update. Do not expand into new voting engines or move game behavior into core.

Each slice is one complete branch/PR containing SDK, runtime, CLI and caller changes needed for its feature. Fresh subagents implement bounded tasks; integration and review are owned by one coordinator. Native changes require the native gate and live CS2 acceptance on the supported Linux environment. Missing infrastructure is an explicit pending gate, not a pass.

## Research basis

- [SourceMod natives and include contracts](https://wiki.alliedmods.net/Creating_Natives_(SourceMod_Scripting))
- [Global/private forwards](https://wiki.alliedmods.net/Function_Calling_API_(SourceMod_Scripting))
- [Dispatch and global-name lookup](https://github.com/alliedmodders/sourcemod/blob/master/core/logic/ForwardSys.cpp)
- [Optional dependencies](https://wiki.alliedmods.net/Optional_Requirements_(SourceMod_Scripting))
- [Basecomm contract](https://github.com/alliedmodders/sourcemod/blob/master/plugins/include/basecomm.inc)

SourceMod supplies precedent for the capabilities; the scoped identity, generated metadata, strict wire algebra, and lifetime semantics above are s2script design decisions.
