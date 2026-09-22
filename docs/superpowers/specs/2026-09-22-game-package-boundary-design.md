# Game-Package Boundary and Bootstrap

**Date:** 2026-09-22

**Status:** Approved architecture; written design for review

**Stack:** S3, above [S1: engine bindings](2026-09-22-engine-bindings-design.md) and [S2: engine functions](2026-09-22-engine-functions-design.md)

## Decision

S3 makes the selected game package the owner of game semantics. The process still has one small
runtime and one Source 2 native bridge. S1 supplies checked resolution and KHook bindings; S2
supplies the shared function service. A manifest selects adapter modules and data shipped in the
same addon. S3 introduces neither an extension DSO loader nor a second hook system.
The first package remains `@s2script/cs2`, with compatible API and behavior unless a separate
migration says otherwise. Neither core nor shim names it, knows its bootstrap filename, or owns
policies such as acquisition receiver meaning or HUD click delivery timing.

## Goals and non-goals

S3 must:

- keep core engine-generic and the native bridge limited to Source 2 mechanics;
- select packaged game code and gamedata from data rather than `@s2script/cs2` and `pawn.js`
  literals in runtime code;
- move CS2 semantic adapters onto the S2 function service without changing their public behavior;
- give damage, ammo, and ordinary custom functions the same generic call/hook path;
- preserve the independent gamedata owner and target-game dimensions;
- make package, callback, borrowed-value, handle, reload, and shutdown lifetimes explicit.

S3 does not claim arbitrary native ABI support, universal Source 2 portability, native plugin loading, or proof that another game runs because a fixture loads. S2 owns the bounded ABI matrix.

## Ownership boundary

| Layer | Owns | Must not own |
| --- | --- | --- |
| Small runtime (`core`) | V8 contexts, plugin/package module registry, permissions, owner generations, ledgers, generic values and handles | game ids, class names, team/item rules, CS2 callback policy |
| Source 2 native bridge (`shim`) | engine interface acquisition, module/range discovery, schema access mechanics, checked S1 resolve/bind/install calls, generic native value projection | a selected package name/path, CS2 receiver hops, HUD or acquisition meaning |
| Shared function service (S2) | target, ABI, projection, call/pre/post surfaces, compatible binding fan-out, status/provenance, subscriber lifetime | a hardcoded gameplay-capability catalog or inference of semantics from an ABI shape |
| Game package | public game API, schema wrappers, event overlays, semantic adapters, game-owned function declarations and target data | raw pointers, unchecked casts, independent detour/scanner code |

These are logical boundaries in the existing binaries plus packaged JavaScript and data, not one
binary per row. The deployed addon initially ships the CS2 adapters, types, schema, and gamedata.

## Package manifest and bootstrap

Each first-party game package has a source manifest beside its package code. Packaging emits the
deterministic deployed manifest `addons/s2script/game-packages.json`, for example:

```jsonc
{
  "schemaVersion": 1,
  "packages": [{
    "id": "@s2script/cs2",
    "match": { "engine": "source2", "game": "csgo" },
    "gamedataOwner": "cs2",
    "bootstrap": { "path": "game-packages/cs2/index.js", "sha256": "<sha256>" },
    "gamedata": { "path": "game-packages/cs2/gamedata.json", "sha256": "<sha256>" }
  }]
}
```

The illustrative `<sha256>` values are replaced by packaging with lowercase
64-character SHA-256 digests. The gamedata bundle retains the owner's layout,
keys, target variants, and S2 function definitions, not only its functions. The
`gamedataOwner` field maps the reserved package id to the existing owner tree and
operator override directory; CS2 keeps the `cs2` mapping so existing custom files
are not silently ignored. Hashes cover the shipped artifacts; operator overlays
are applied afterward by the shared loader with their own provenance.

The native bridge supplies the detected game token; runtime code does not branch on it. Matching is
deterministic. Zero matches leaves the generic runtime available but refuses plugins requiring a
game package. Multiple matches name every candidate and fail. The host reserves the selected id.

Bootstrap and gamedata paths are addon-relative, normalized, and confined to the game-package root.
The loader verifies the packaged SHA-256 values before registering source and the declared owner
mapping. Duplicate owner mappings are rejected. Manifest, hash, or selection failure leaves no game package selected and reports a
named process status. On plugin reload, permission, schema, or descriptor failure during candidate
preparation preserves the running generation. Once JavaScript bootstrap or activation starts, the
baseline loader has retired the old generation: failure safely tears down the candidate and reports
it unavailable; S3 does not promise rollback.

Core's per-context bootstrap iterates the selected manifest records. The shim no longer contains a
`Cs2JsPath`, `pawn.js` fallback, package-name literal, or CS2-specific crash-fingerprint read. Build
packaging may still concatenate modules, but the manifest names the resulting artifact and its hash;
the runtime does not know the concatenation recipe or entry filename.

The manifest loads JavaScript/data only. Native Source 2 support remains in the existing Metamod
shim; no manifest field names or loads another `.so`.

## Layout data and semantic code

Layout data says where and how to reach a native fact: module, signature, resolver, vtable slot,
field offset, interface/version string, ABI, projection, and validators. Semantic code says what the
fact means: player/team mapping, item-acquisition decisions, HUD-click routing and timing, damage
field meaning, ammo policy, and the game API presented to authors.

The split is based on ownership and consumption, not filenames. `owner` answers who declares and
may override a descriptor. `target` answers which game/engine/OS binary the descriptor applies to.
They remain orthogonal. A core-owned Source 2 primitive may legitimately have a `game.cs2` target;
its filename is not a reason to transfer ownership. CS2-owned layout consumed only by the CS2
package moves with that package. Provenance always records both owner and target.
Common defaults, game/engine and OS variants, and custom overrides keep their ordered merge and
attribution. S2 plugin overrides stay namespaced; S3 adds no second precedence system.

## Semantic adapters on the function service

S2 deliberately separates native ABI, safe value projection, and named semantic policy. S3
registers the named policies under the reserved game-package owner and attaches them to ordinary S2
function declarations. A policy id is an explicit reviewed contract; an ABI shape never selects one.

The CS2 acquisition adapter performs the item-services-to-player mapping, supplies the compatibility
result view, and preserves the existing most-restrictive subscriber collapse. The HUD adapter owns
receiver/argument selection, copied string boundaries, re-entrancy behavior, and delivery ordering.
Existing HUD callbacks currently run in the compatibility "post" surface before the engine original;
S3 preserves that observable order. Renaming or normalizing it requires a separate public migration.

Adapters use only S2-supported ABI rows and projections. They may construct game-level wrappers from
`EntityRef` or registered handles, but cannot expose or retain a native pointer. A borrowed argument
or result view is valid for its synchronous callback epoch only, rejects access after return, and
cannot cross `await`. Data that must outlive the callback is copied or represented by a serial-gated
handle with host-owned liveness.

Damage follows the same path: a normal function declaration and KHook subscription use a registered,
bounded native projection codec where scalars are insufficient, then a CS2 adapter gives fields their
public meaning. Such reusable codec mechanics may remain in the bridge; game policy moves. Ammo uses
ordinary schema/function declarations and a CS2 wrapper. Neither gets a bespoke hook installer or
hardcoded gameplay-capability catalog.

Community plugin functions are first-class peers in the S2 registry. They use their own owner id,
target, ABI, generic projection, permissions, status, and receipts. A public game package may export
a named adapter contract; private semantic adapters cannot be selected by merely copying an ABI.

## Public API compatibility

`@s2script/cs2`, its subpaths, `Player`, `Pawn`, schema wrappers, damage APIs, acquisition hooks, HUD
APIs, and base-plugin imports keep their current author-visible contracts. The internal bootstrap
artifact may stop being called `pawn.js`; that is not a plugin API.

Any contract change is declared separately with a deprecation window or major-version gate. S3
cannot silently alter null behavior, ordering, suppression, mutation, self-call, or unload behavior.

## Lifetime and teardown

Package selection and the verified source/data registration live for one shim load. The selected
package has a reserved owner and contract hash. Each plugin-context evaluation has its own generation
and ledger for wrappers, subscriptions, handles, and other context state.

Function targets and compatible physical bindings may be shared, but registrations and callbacks
remain owner- and generation-gated. Reload removes only the old owner's receipts. A community plugin
reload cannot detach the game package or another plugin's subscriber.

Per-context teardown is:

1. stop new dispatch into the retiring owner/generation;
2. detach its policy/function subscriptions so no new callback can enter;
3. let active synchronous callback frames unwind; an unload requested inside a callback schedules
   retirement after the outermost frame and never waits there;
4. invalidate borrowed epochs, wrappers, and owned handles, then release S1 binding receipts;
5. dispose context-local package state and its provenance records.

Map teardown additionally invalidates map-scoped entities and handles before new-map callbacks. A
stale closure, binding, subscription handle, or borrowed view must fail without touching native
memory. Host ledgers provide cleanup even when package or plugin JavaScript omits it.

Unloading one plugin context never removes process-level package source/data registration or another
context's instance. Those registrations are cleared only during shim teardown, after all contexts and
in-flight callbacks have retired.

## What SourceMod establishes

SourceMod provides useful evidence and warnings about where complexity accumulates:

- Core reads `GameExtension` from gamedata and loads a selected compiled extension: [loader](https://github.com/alliedmodders/sourcemod/blob/master/core/sourcemod.cpp), [mapping](https://github.com/alliedmodders/sourcemod/blob/master/gamedata/core.games/common.games.txt). Its [CS](https://github.com/alliedmodders/sourcemod/blob/master/extensions/cstrike/extension.cpp) and [TF2](https://github.com/alliedmodders/sourcemod/blob/master/extensions/tf2/extension.cpp) extensions validate their game and load extension-owned gamedata.
- [GameConfigs.cpp](https://github.com/alliedmodders/sourcemod/blob/master/core/logic/GameConfigs.cpp) parses offsets, signatures, addresses, and keys with game/engine/platform sections and custom files. Custom precedence is powerful but can surprise, so S2/S3 retain provenance and conflict checks; see [gamedata updating](https://wiki.alliedmods.net/Gamedata_Updating_(SourceMod)).
- [SDKTools](https://wiki.alliedmods.net/SDKTools_(SourceMod_Scripting)) requires plugin code to declare call type, return, parameters, and passing rules: target data alone is insufficient. [SDKHooks](https://github.com/alliedmodders/sourcemod/blob/master/extensions/sdkhooks/extension.cpp) is a compiled catalog enabled by offsets. [DHooks](https://github.com/alliedmodders/sourcemod/blob/master/extensions/dhooks/natives.cpp) supports runtime definitions and `FromConf`, with more ABI and shared-configuration complexity.
- SDKHooks removes hooks on entity destruction or plugin end, and DHooks cleans plugin-owned hooks. S3 applies that discipline through receipts and ledgers: [SDKHooks API](https://github.com/alliedmodders/sourcemod/blob/master/plugins/include/sdkhooks.inc), [DHooks extension](https://github.com/alliedmodders/sourcemod/blob/master/extensions/dhooks/extension.cpp).

S3 takes the data-selected package and explicit ownership lessons without adding SourceMod's native
extension loader. KHook remains a compile-time typed native mechanism. Gamedata may select a checked
address or slot, but it does not make KHook accept an arbitrary runtime ABI; only S2's proven matrix
is public.

## Portability claim

A synthetic second-game fixture proves that package selection, context bootstrap, owner isolation,
and an ordinary S2 call/hook do not require a CS2 literal or native rebuild. It does not prove a real
second game runs.

Another Source 2 game may still need native interface/version support, schema differences, platform
ABI coverage, new stock-KHook-compatible signatures, and bridge capability work. Those additions
belong in the generic Source 2 bridge only when their behavior is truly shared; game meaning belongs
in that game's package. A full support claim requires native tests and a live server gate for that
game and platform.

## Acceptance criteria

S3 is accepted when:

- core and shim bootstrap contain no `@s2script/cs2`, `pawn.js`, or equivalent single-game branch;
- a packaged manifest deterministically selects CS2, registers its source/data owner, and reports
  missing, ambiguous, invalid, or failed packages with provenance;
- the synthetic second-game fixture selects and uses one ordinary S2 function without recompiling
  native code, while documentation makes no live-game claim from that fixture;
- existing CS2 public types, imports, base plugins, and observable callback behavior pass unchanged;
- acquisition and HUD compatibility policies live in CS2 adapter code, with exact collapse, copy,
  re-entrancy, and engine-original ordering tests;
- damage and ammo use shared S1/S2 resolution, projection, subscription, and teardown paths, with no
  capability-specific native installer or core catalog entry;
- ordinary plugin functions and first-party functions share status/provenance and compatible binding
  fan-out while retaining distinct owner/generation cleanup;
- owner/target tests include a core-owned descriptor targeted at CS2 and a CS2-owned descriptor,
  proving that a target filename does not decide ownership;
- borrowed views, handles, stale closures, plugin reload, map teardown, and shim unload fail safely
  and release every ledgered receipt in the specified order;
- a published capability matrix states which Source 2 interfaces, platforms, and ABI rows are truly
  supported and rejects everything else by name;
- PR A's inherited shutdown SIGSEGV/139 and required human/client evidence are resolved before S3
  implementation acceptance;
- full native and JavaScript CI passes, and the deployable sniper release with all default plugins
  passes live CS2 map, reload, and ordinary-shutdown gates with the engine child's actual exit status.

## PR ownership

S1 owns checked address resolution, target validation, stock-KHook registration, and physical
binding receipts. S2 owns the normalized function contract, ABI/projection matrix, plugin authoring,
permissions, operator overrides, typed bindings, and generic subscription lifecycle.

S3 owns the package manifest/bootstrap, `games/cs2`, `packages/cs2`, CS2-consumed gamedata, and named
CS2 semantic adapters. It may wire existing generic core/shim entry points to manifest records, but
does not redesign S1 binding or S2 authoring. Shared layout remains with its actual consumer even
when its target is CS2. Any lower-layer contract change returns to the owning PR instead of being
hidden in the extraction.
