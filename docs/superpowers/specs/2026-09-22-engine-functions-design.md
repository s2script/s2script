# Engine Functions: Plugin Authoring and Runtime Contract

**Date:** 2026-09-22

**Status:** Written design approved for implementation on 2026-09-22; implementation and acceptance remain in progress.

**Implementation plan:** [Engine functions](../plans/2026-09-22-engine-functions.md)

**Stack:** S2, above [S1: engine bindings](2026-09-22-engine-bindings-design.md) and below S3 game-package extraction

## Purpose

Plugin-owned gamedata repeats each function across signature/call/hook entries, parallel parameter
arrays, unused `expose.ctx`, package paths, declaration roots, and permissions. Plugin archives also
bypass operator `custom/` repair, requiring a rebuilt `.s2sp` for a signature update.

S2 replaces that authoring model with one **engine function** declaration. A function owns one
target, one ABI, and any enabled call/pre/post surfaces. The SDK discovers, validates, packages, and
types it automatically. The loader applies deterministic operator overrides, resolves it through S1,
and exposes one typed binding to the declaring plugin.

This clean public model has a v1 migration path. Existing packed descriptors may remain an internal
compatibility input but do not constrain new authoring.

## Relationship to S1 and S3

S1 owns the resolver, validators, range checks, KHook primitives, and legacy typed shapes. S2 owns
the bounded ABI adapter, fixtures, and spike. A missing primitive is an explicit S1 prerequisite;
it does not justify a second backend or forbid necessary S2 adapter work.

S2 owns plugin authoring, normalization, generated types, archive metadata, permission requests,
operator overrides, plugin-scoped registration, and the generic JavaScript binding.

S2 introduces named compatibility adapters for existing Acquire/HUD paths so ABI shape never selects
semantics. S3 later relocates them into CS2 without changing behavior. S2 otherwise adds no game API.

## Exclusions

S2 excludes universal FFI, arbitrary prototypes/calling conventions, raw pointers, runtime-authored
signatures, cross-plugin private access, and inferred semantics. It does not make every C++ method
hookable or move the CS2 package.

## Authoring format

`s2s build` discovers `<plugin>/gamedata/functions.jsonc`; no file means no capability. There is no
`s2script.gamedata`, `requiresGamedata`, package-derived filename, or manual tsconfig entry.

The package id is the declaration namespace. A local name `ignite` in `@demo/fire` has canonical id
`@demo/fire::ignite`. Authors use the local name; manifests, diagnostics, override paths, and runtime
registries use the canonical id. Duplicate local names and duplicate JSON keys are build errors.

### Example

```jsonc
{
  "schemaVersion": 2,
  "functions": {
    "ignite": {
      "target": {
        "module": "libserver.so",
        // Illustrative only; derive a deployable pattern from the pinned server binary.
        "pattern": "55 48 89 E5 41 56 ...",
        "validate": { "vtable-member": "CBaseModelEntity" }
      },
      "receiver": { "type": "entity" },
      "parameters": [
        { "name": "flameLifetime", "type": "f32", "mutable": "pre" },
        { "name": "flags", "type": "i32" },
        { "name": "attacker", "type": "entity?" },
        { "name": "size", "type": "f32" }
      ],
      "returns": "void",
      "surfaces": ["call", "pre", "post"]
    }
  }
}
```

Parameter order, name, type, and mutability appear together once. Defaults are `optional`,
`resolve: "direct"`, generic projection, `surfaces: ["call"]`, standard `HookResult` collapse for
pre hooks, and `bypass-own-hooks` for calls through this binding. Ordinary authors add only target,
receiver, parameters, and return; `surfaces` and `mutable` opt into hooks. Advanced declarations may
set `requirement`, supported resolver inputs, or an authorized named projection adapter.

`target` remains data because it changes with the game binary. The declaration may instead refer to
a named target within the same file when several functions intentionally share an address recipe.
References are local to the owner and are flattened during normalization.

## Plugin API

The generated augmentation makes local names and their complete function contracts visible:

```ts
import { Engine } from "@s2script/sdk/unsafe";
import { HookResult } from "@s2script/sdk";

const ignite = Engine.function("ignite");

if (!ignite.available) {
  console.log(ignite.status);
} else {
  ignite.call(pawn.ref, 10, 4, null, 0);
  ignite.onPre((view) => {
    view.flameLifetime = Math.min(view.flameLifetime, 20);
    return HookResult.Changed;
  });
  ignite.onPost((view) => console.log(view.flameLifetime));
}
```

The binding is a discriminated union. Optional functions require `available`; required functions
resolve before activation and return the available branch. `status` includes canonical id and
provenance. The fixed API is `Engine.function`, `.available`, `.status`, `.call`, `.onPre`, `.onPost`.
Availability means validated and authorized; native hook observation remains a separate
Pending/Active/Failed status under S1. Lazy registration failures are reported on the
subscription and binding status, not misrepresented as an observed callback.

Subscriptions return ledgered disposable handles. Plugin unload disposes them automatically; an
author may dispose early. A pre callback uses the existing `HookResult` collapse contract where the
function's policy permits suppression. Post callbacks cannot suppress an original call that already
ran. Unsupported surface/policy combinations fail the build rather than becoming no-ops.

For a generic non-void function, PRE returns `Continue`, `Changed`, or `void`. Suppression is
`{ action: HookResult.Handled | HookResult.Stop, returnValue: R }`; bare `Handled`/`Stop` is valid only
for void functions. An invalid suppression decision is reported and ignored, never replaced by an
invented numeric/default return. POST receives readonly arguments plus the typed effective
`returnValue` and cannot override it in generic v2. Named compatibility adapters keep their existing
public callback contracts. Observation is explicit:

```ts
ignite.onPre({ observeOnly: true }, (view) => console.log(view.flameLifetime));
```

That callback's type permits no mutation or action result.

## ABI, projection, and policy are separate

Every normalized function has three independent layers:

1. **ABI** describes the native receiver, ordered parameters, return, widths, and supported ownership
   classes. It is sufficient to call or intercept the native function correctly.
2. **Projection** describes which safe values JavaScript sees and how native values become them. The
   generic projections are scalars, copied strings/vectors, `EntityRef`, registered opaque handles,
   and callback-scoped borrowed views.
3. **Policy** describes enabled surfaces, pre/post timing, mutation, suppression, self-call behavior,
   required/optional behavior, and permissions.

An ABI shape never selects a semantic adapter. S2 registers two audited compatibility adapters:
`legacy.acquire.v1` performs the existing item-services-to-player mapping and synthetic result view;
`legacy.hud-click.v1` performs the existing argument selection and string copy. They are selected by
explicit internal adapter id plus contract hash, not by ABI, and community descriptors cannot name
them. S3 relocates the adapters to the CS2 package while preserving ids and contracts.

`legacy.hud-click.v1` preserves current dispatch timing, copy boundary, re-entrancy behavior, and
ordering relative to engine/map script. A raw KHook pre/post phase cannot silently replace or shift it.

## Bounded KHook feasibility gate

KHook is the interception mechanism, not arbitrary FFI. Before public API implementation, S2 runs a native
spike against pinned stock KHook in the deployable bullseye build. It records receiver forms, integer and
floating widths, pointer-sized opaque slots, stack arguments, return classes, pre mutation, post
observation, original suppression, re-entrant calls, peer-hook effective results, cleanup, and
concurrent subscribers. It proves one newly supported native signature without adding a
signature-specific thunk.

Only matrix rows proven by native tests and a live CS2 gate become public ABI types. A declaration
outside the matrix is rejected with the first unsupported ABI feature named. There is no fallback to
an unchecked cast, a legacy detour, or a newly hand-written thunk per custom function. If the bounded
runtime adapter fails, S2 stops and returns to design review; it does not self-authorize a reduced
call-only release.

## Validation and failure behavior

Build validation checks schema version, duplicate keys, identifiers, target completeness, validator
presence, ABI support, parameter uniqueness, projection legality, surface/policy compatibility, and
permission derivation. Normalized output includes a contract hash over ABI, projection, and policy.
Generated view fields `self` and `returnValue` are reserved parameter names; the
builder reports a naming collision rather than overwriting a value accessor.
For S1's `validated-call` resolver, call-site validators run on every candidate before derivation.
The derived target runs target-stage validators only when the recipe specifies them; call-site
validators are never replayed at the callee. S2 must not flatten this into derive-first resolution.

Load repeats all safety-relevant checks. Resolution and validation are fail-closed per function. A
required preparation failure rejects the candidate before the current plugin unloads, preserving the
old generation. An optional failure produces a named unavailable binding. No failed function installs
a hook or exposes a callable.

The host interns a native-target record keyed by module identity, resolved address, and ABI
fingerprint. Projections may differ per subscriber. A declaration with a conflicting ABI
is rejected before interning another record at that address. All mutating or suppressing subscribers
must share an exact policy id/version; incompatible policies are rejected with both canonical names.
Generic POST and explicit observe-only PRE may join game adapters because they cannot change state.
One physical KHook registration fans out to every compatible projection.

Within a policy domain, mutating PRE callbacks run in stable host registration order; each edit flows
to the next, and `Stop` ends remaining PRE callbacks in that domain only. Observational subscribers
still run. S2 makes no ordering claim relative to external KHook peers.
For the generic policy, the strongest valid action wins; the first return value at
that action strength wins a tie. Named legacy policies retain their existing folds.
Observe-only PRE runs after the mutating domain with a readonly view of its final
arguments; POST observes the effective result supplied through KHook. Neither phase
silently falls back to an earlier result if the feasibility gate cannot prove it.

## Permissions

Enabled surfaces derive existing capabilities into the packed manifest: `call` requests
`engine:calls`; `pre` or `post` requests `engine:hooks`. Suppression and mutation are additional
install-visible risk metadata, not new permission names.

Authors do not repeat these strings in `package.json`. `s2s build` prints the derived request and
`s2s inspect` shows it without loading the plugin. The operator allow-list remains explicit and
default-deny by plugin id and capability. An override cannot grant a surface the archive did not
request, widen an ABI, or change an optional function to required. First-party game-package trust is
represented by its reserved runtime owner, not by spoofable plugin metadata.

## Operator overrides and provenance

Plugin overrides live at `addons/s2script/gamedata/plugins/id-<base64url>/custom/*.jsonc`, where
`<base64url>` is RFC 4648 URL-safe base64 without padding over the plugin id's exact UTF-8 bytes
(`@demo/fire` becomes `id-QGRlbW8vZmlyZQ`). No case folding or scope stripping occurs. On plugin
load or explicit reload, the loader reads the embedded base, then sorted custom files. Overrides are
target repairs by default: module, pattern, resolver inputs, and validator.
They cannot change ABI, projection, surfaces, policy, requirement, or generated TypeScript contract.

Every override names the base contract hash it expects. A replacement target must include a complete
validator; validators are never silently dropped or inherited from a target with a different
pattern. If two custom files touch the same function, the later lexical file must explicitly name the
earlier file in `supersedes`; otherwise the function degrades with a conflict. This gives deterministic
precedence without making accidental last-writer-wins look successful.

An override resolves a candidate owner against a new native-target record and rebinds that candidate.
It never retargets an existing shared record or its live subscribers.

Diagnostics and crash fingerprints record archive hash, base contract hash, every applied override
path and hash, final target hash, resolver/validator result, and whether the function is required.
Missing custom directories are normal. Malformed overrides affect only their named plugin/function
unless the file cannot be attributed safely, in which case that plugin's candidate reload is refused.

Overrides apply on the next ordinary plugin load/reload; S2 adds no watcher or transaction.
Preparation happens before unload. Once activation begins, baseline lifecycle applies: the old
generation unloads first; a JavaScript factory/start failure cleans up the failed candidate but does
not restore the old generation.

## Lifetime and value safety

Raw pointers never enter JavaScript. Entity receivers and arguments resolve through the host's live
books for each call. Returned entity pointers pass through handle adoption and may yield `null`.
Opaque native objects require a registered handle kind with host-owned liveness and invalidation.

Borrowed arguments and return views are valid only during their pre/post callback. Accessors carry a
dispatch epoch, reject reads and writes after the callback, and cannot cross `await`. Strings and
vectors are copied. A generic projection cannot dereference, offset, cast, retain, or return a native
pointer. Callback mutation is limited to parameters whose single declaration includes that phase.

Bindings and subscription handles are owner- and generation-gated. Unload invalidates bindings,
removes ledger entries, and detaches subscribers through S1's registration receipt. Reload never lets
a callable captured from the old context address the new generation.

`bypass-own-hooks` skips only subscriptions owned by the calling plugin generation for that target and
invocation. Other plugins, provider/game-package subscribers, and KHook peers still observe it. The
bypass state is properly nested and thread-local. Re-entry is part of the native proof; an unsupported
case fails by name rather than silently dropping observers.

## Migration from v1

`s2s migrate engine-functions` reads `s2script.gamedata`, `signatures`, `calls`, `hooks`, generated
declarations, and source references, then writes `gamedata/functions.jsonc` plus a report. When
mechanically provable, it zips `args`/`argNames`, carries validators explicitly, joins identical
call/hook targets and ABIs, decodes proven shapes, converts `bypassWith`, removes plugin-only
`expose.ctx`, and derives permissions.

Missing names, empty validators, mismatched call/hook targets, lossy shape projections, unsupported
ABI rows, and ambiguous receiver hops stop migration with a named item. The tool never guesses and
never emits a partially weakened function.

For one published deprecation window, the builder accepts v1 and normalizes it through the same v2
intermediate representation. `Engine.call` and `Engine.hook` remain generated compatibility facades
with their old timing and null behavior. Removal or behavior change requires an SDK major release,
an archive schema gate, and a build error with the migration command. A v1 validator or public API
contract is never silently dropped to make a build pass.

## Lifecycle

Build order is parse, migrate/normalize, validate, generate declarations, typecheck, derive manifest,
and pack normalized functions. Load order is read archive, read permissions and overrides, construct
candidate provenance, validate, resolve required and optional functions, register checked bindings,
then evaluate plugin JavaScript. Hooks install lazily on first subscriber.

All persistent bindings, subscriptions, and shared-address registrations are ledgered. Teardown
follows the ledger and does not depend on plugin cleanup. Descriptor status is queryable without
exposing addresses. S2 promises preservation only for failures found during pre-unload preparation.

## Acceptance criteria

S2 is accepted when:

- a new plugin declares a typed call and pre/post hook in only `gamedata/functions.jsonc`;
- no package path, permission strings, generated include, `expose.ctx`, parallel names, or manual archive step;
- generated editor types and the strict build gate agree for call, pre, post, mutation, and return;
- required preparation failure preserves the old generation; activation failure cleans the candidate;
- an operator repairs a plugin target without rebuilding the archive, with deterministic provenance;
- conflicting or stale overrides fail closed and cannot change the public contract;
- same-address compatible declarations share one registration; incompatible ABIs are refused;
- borrowed views, stale entities, bindings, and subscription handles cannot reach native memory;
- v1 migration preserves validators, timing, permissions, and API behavior or names the ambiguity;
- stock KHook proves reentrancy, peer results, cleanup, and a new signature without a new thunk;
- core remains engine-generic and the base-plugin/JS/native CI gates stay green.

## Agent and PR ownership

S1 owns resolver/KHook primitives, legacy typed shapes, and their native evidence. S2 owns its bounded
runtime ABI adapter and fixtures, SDK schema/codegen, archive/loader contract, plugin registry, unsafe
binding, migration, compatibility adapters, and worked example. Missing primitives return to S1 as
prerequisites. S3 owns `gamedata/cs2`, `games/cs2`, `packages/cs2`, and behavior-preserving relocation
of the S2 Acquire/HUD adapters.

Each PR is independently reviewable at its layer and uses an explicit interface from the PR below.
If a higher layer discovers an S1 contract change, that change returns to S1 rather than being hidden
inside S2 or S3.
