# Plugin-shippable gamedata & declared engine calls — design

**Status:** Approved — ready for planning.
**Audience:** plugin authors who need an engine function the core does not wrap; core/shim maintainers.
**Builds on:** the existing `gamedata/core.gamedata.jsonc` loader (`shim/src/gamedata.{h,cpp}`), the
`__s2_schema_offset` live offset resolver, the `__s2_entity_subobj_vcall` primitive, the `.s2sp`
archive format (`core/src/loader.rs::read_s2sp`), and the codegen doctrine already used by
`s2s gen-schema` / `gen-events` / `gen-nav`.

---

## 1. Goal

Let a plugin declare, in its own regenerable gamedata, an engine function the framework does not
natively wrap — and call it from TypeScript with generated types, load-time validation, and
per-descriptor degradation. Close the "core doesn't wrap X, so the plugin is stuck until we ship a
release" gap without punching a hole in the safety model.

**Non-goal:** turning s2script into a general FFI. The v1 surface is deliberately closed (§4).

## 2. The gap this closes

Today every engine call must be sig-resolved and wrapped in core. A plugin author who needs
`CBaseEntity::Ignite` or `CCSPlayer_ItemServices::DropActivePlayerWeapon` has no path at all: the
`EntityRef` raw read/write surface covers *state*, but there is no way to invoke a *function*.
`__s2_entity_subobj_vcall` exists in core but is not in any `.d.ts` — the prelude uses it, plugins
cannot.

Concretely reachable once this lands: `Ignite` (unblocks `funcommands` `sm_burn`, currently
deferred), `DropActivePlayerWeapon` (currently deferred), `CCSPlayerController::SetClanTag`,
`CCSGameRules::RestartGame`, `CCSWeaponBase::Holster`/`Deploy`, `SetParent`,
`SetBodyGroupByName`, `SetCollisionBounds`.

## 3. Failure posture

Signatures **and** vtable indices are both allowed, but a vtable index is never trusted bare.

This follows the RE doctrine (`docs/re-strategy.md`): self-resolve against *our* binary, or validate
at load — never a borrowed constant. The specific reason a bare index is unacceptable is recorded in
`gamedata/core.gamedata.jsonc:66-84`: on the pinned `libserver.so`, the borrowed ItemServices slots
24/25/26 are `GiveNamedItem`-overload **thunks**, and because thunks are valid in-range code the
`IsAddressInServerText` guard does **not** catch them — a call silently misbehaves rather than
crashing. `.text`-range validation is therefore necessary but provably insufficient.

So:

- `target.kind: "signature"` — byte-pattern resolved against our binary. Self-validating: it matches
  this build or it does not.
- `target.kind: "vtable"` — **requires** a `validate` predicate. A descriptor with an index and no
  `validate` **fails the build**, not the load.

**v1 defines exactly one validator**, evaluated shim-side after the `.text`-range check:

| Validator | Meaning |
|---|---|
| `prologue` | masked byte pattern (`??` wildcards) the resolved function must begin with |

`prologue` is what catches the thunk case: a thunk's first bytes differ from the real function's. It
reuses the existing `s2sig::ParsePattern` matcher, so it adds no new matching machinery — which is
why v1 ships this one and defers xref-style validators (§14).

## 4. Call surface (v1, closed)

- **Receiver** is always an `EntityRef`, optionally hopping through one schema-named sub-object
  pointer (`receiver.via`). The `this` pointer is resolved in-core from a books-gated ref.
- **Arg vocabulary:** `bool`, `int`, `float`, `string`, `vector`, `entity`.
- **Return vocabulary:** `void`, `bool`, `int`, `float`, `entity`.
- Calling convention: C++ member function under SysV — `this` in `rdi`, integer-class args in the
  remaining GP registers, float args in `xmm`. Overloads and variadics are out of scope.

**Arg budget: 9 integer-class args**, plus the receiver when the descriptor has one. Six is the SysV
*register* count, not a limit on the call — further integer args spill to the **stack**, and the
shim's max-arity prototypes declare enough slots to cover them. (The original budget was five + `this`
and rejected anything more with "stack-passed args are out of scope"; a seven-argument engine factory
was therefore undeclarable.)

Everything except `float` is **integer-class**: `bool`, `int`, `entity` (an entity
pointer), `string` (a `char*`), and `vector` (passed by address). SysV gives six GP argument
registers and `this` consumes the first, so the limit is:

- **at most 5 integer-class args**, and
- **at most 8 `float` args**

Any descriptor exceeding either bound **fails the build**. Stack-argument marshalling is deferred, so
there is deliberately no "6 args" rule — a flat count would silently spill to the stack.

**The invariant this preserves:** no raw pointer ever crosses into JS. JS holds descriptor *names*;
core and shim hold pointers. `string` args are marshalled into a temporary valid only for the
duration of the call; `vector` args into a stack temp passed by address.

**`returns: "entity"` never mints a ref from the returned pointer.** A returned `CBaseEntity*` is
converted **shim-side** into a `CEntityHandle` by reading the entity's own identity handle, and that
handle is then decoded through the existing books-gated `__s2_handle_adopt` path — the same route
`Pawn.forSlot` uses. A pointer that is null, outside the entity system, or whose handle the host's
books do not vouch for yields `null`, never a live ref. The raw pointer stays in the shim.

**Caveat (author's risk, `unsafe` by design):** a `string` arg's buffer is valid only for the duration
of the call. A callee that *retains* the pointer will dangle. v1 does not detect this.

`receiver.kind` is a **tagged** field so that non-entity receivers (named Valve interface, game
system) are an additive kind in a later slice, not a format redesign.

**`receiver.kind: "none"`** — a STATIC/free engine function, which has no `this` at all. The generated
callable takes no leading `self`, and the first declared arg occupies the register the receiver would
have used. `via` is rejected on a receiverless descriptor: a sub-object hop is a hop *from* a
receiver, so the combination is contradictory rather than merely redundant. This is what makes engine
FACTORIES declarable — they are static by nature, and without it a plugin needing one had no route
except a game-specific op in the core, which the boundary gates forbid.

## 5. Descriptor format

A plugin ships its gamedata in its own source tree, named
**`<plugin-name-without-scope>.gamedata.jsonc`** — `@me/burn` → `burn.gamedata.jsonc` — mirroring the
framework's own `gamedata/core.gamedata.jsonc`, where the file is named for whoever owns it. `s2s build`
**enforces** the basename (a `.json` extension is also accepted); the directory is not constrained,
though `gamedata/` is the convention. The file reuses the existing `core.gamedata.jsonc` shape exactly
— a named entry whose keys are **platform ids** (`linuxsteamrt64`), with the platform-specific details
nested inside — plus a new `calls` section.

```jsonc
{
  "signatures": {
    "ItemServices_DropActivePlayerWeapon": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 89 E5 41 57 41 56 ...",
        "resolve": "direct"
      }
    }
  },
  "calls": {
    "dropActiveWeapon": {
      "receiver": { "kind": "entity",
                    "via": { "class": "CBasePlayerPawn", "field": "m_pItemServices" } },
      "target":   { "kind": "signature", "name": "ItemServices_DropActivePlayerWeapon" },
      "args":     ["entity"],
      "returns":  "void"
    },
    "ignite": {
      "receiver": { "kind": "entity" },
      "target":   { "kind": "vtable",
                    "class": "CBaseEntity",
                    "linuxsteamrt64": {
                      "index": 214,
                      "validate": { "prologue": "55 48 89 E5 48 83 EC ?? 48 8B 07" }
                    } },
      "args":     ["float", "bool", "float", "bool"],
      "returns":  "void"
    }
  }
}
```

`resolve` is closed to the vocabulary the shim dispatches on — `direct`, `ctor-body-xref`, `lea-disp` —
so a typo fails the build naming the valid set instead of degrading silently at load. Plugin gamedata
introduces no new resolver kinds. `validate` is nested **under the platform key** alongside `index`,
because a prologue is as platform-specific as the slot number it guards.

A call may carry an optional **`argNames`** array, positionally matched to `args`:

```jsonc
"args":     ["float", "int", "entity", "float"],
"argNames": ["flFlameLifetime", "nFlags", "pAttacker", "flSize"],
```

It is documentary only — the runtime never reads it — but it is what makes the generated signature
legible, and on an `unsafe` FFI surface the parameter names *are* the documentation. It is a parallel
array rather than a richer `args` element type on purpose: the runtime only ever needs the kinds, so
`args` stays a flat string array and core parses the packed gamedata unchanged. Entries are validated
as plain identifiers, must be unique, must not be `self` (the generated receiver parameter), and must
match `args` in length.

`receiver.via` is resolved through the existing cached `__s2_schema_offset`, so the sub-object
pointer offset is **live-resolved, never baked** — a field move needs no plugin change.

> Every concrete value in the snippets above (pattern bytes, `index: 214`, the `Ignite` arg list) is
> **illustrative**. Resolving the real ones is RE work owned by the implementation plan, per
> `docs/re-strategy.md` — not settled by this spec.

## 6. Manifest & permissions

`permissions` is documented in CLAUDE.md but wired nowhere today; this slice defines it.

```jsonc
"s2script": {
  "gamedata": "gamedata/burn.gamedata.jsonc",
  "permissions": ["engine:calls"]
}
```

Two-part authorization:

1. **Manifest declaration is mandatory.** A plugin whose gamedata has a `calls` section but no
   `engine:calls` permission **fails the build**. This makes the capability auditable and lets
   `s2s install` surface it before an operator installs.
2. **Operator allow-list, default-deny.** `addons/s2script/configs/permissions.json` holds a JSON
   object mapping a permission name to an array of **exact-match** plugin ids (no globs in v1) —
   `{"engine:calls": ["@me/burn"]}` — mirroring the admin system's fail-safe default-deny. It ships as
   a template from `core/config-templates/` alongside `admins.json`/`bans.json`, and is materialized
   the same way. A plugin that declares the permission but is not allow-listed **loads normally with
   every declared call degraded** — not a hard load failure, so an operator's omission never takes a
   server down.

## 7. Package format

`s2s build` packs the parsed, platform-filtered gamedata into the `.s2sp` as a `gamedata.json`
member.

This is backward-compatible by construction: `read_s2sp` (`core/src/loader.rs:187`) reads
`manifest.json` and `plugin.js` **by name and ignores every other member**, and the archive already
carries extra `types/*.d.ts` members. An older runtime simply ignores plugin gamedata; `Manifest` is
serde-forward-compatible for the new `gamedata`/`permissions` fields.

## 8. Type generation

`s2s build` generates `.s2script/gamedata.d.ts` from the plugin's own gamedata — the same
data-to-types pipeline as `gen-schema`/`gen-events`/`gen-nav`. The gamedata is data; the types are
derived from it; the existing typecheck gate then enforces arity and arg types at build time.

```ts
// .s2script/gamedata.d.ts — GENERATED by `s2s build`. DO NOT EDIT.
import type { EntityRef } from "@s2script/sdk/entity";

declare module "@s2script/sdk/unsafe" {
  interface EngineCalls {
    dropActiveWeapon: (self: EntityRef, weapon: EntityRef | null) => void;
    ignite: (self: EntityRef, lifetime: number, npcOnly: boolean,
             size: number, byDesigner: boolean) => void;
  }
}
```

The generated file **must** import `EntityRef` — a `declare module` augmentation resolves type names
in its own file scope, so an un-imported `EntityRef` silently degrades to an error the gate's
`skipLibCheck` can swallow (the same trap `packages/cs2/index.d.ts:19-22` documents for `Weapon`).

Non-`void` calls generate a `T | null` return, matching the framework's degrade convention rather
than inventing a zero-value sentinel.

Parameter names come from the call's optional `argNames` (§5); without it they fall back to positional
`a0…aN`. Call names and arg names are both validated as plain identifiers before emission — they are
interpolated verbatim into this file, so an unvalidated name could inject an index signature into
`EngineCalls` and defeat the gate entirely, or simply emit invalid TypeScript.

## 9. Plugin-facing API — `@s2script/sdk/unsafe`

```ts
export declare const Engine: {
  /** The declared call, or null when its descriptor failed load-time gates. */
  call<K extends keyof EngineCalls>(name: K): EngineCalls[K] | null;
  /** Why a descriptor is unavailable ("available" when it is). For diagnostics/operator reports. */
  status(name: string): string;
};
```

`call()` returns a **plain callable or null**, so the guard happens once at load and call sites stay
clean:

```ts
export default plugin((ctx) => {
  const ignite = Engine.call("ignite");

  ctx.commands.registerAdmin("sm_burn", ADMFLAG.SLAY, (cmd) => {
    if (!ignite) return cmd.reply(`sm_burn unavailable: ${Engine.status("ignite")}`);
    for (const p of Player.target(cmd.arg(1), cmd.callerSlot)) {
      const pawn = p.pawn;
      if (pawn?.isValid) ignite(pawn.ref, 10.0, false, 0.0, false);
    }
  });
});
```

## 10. Architecture & layering

| Layer | Owns |
|---|---|
| **SDK** (`packages/sdk`, TS) | Format: parse gamedata, reject `vtable` without `validate`, reject `calls` without the permission, generate the `.d.ts`, pack `gamedata.json`. No engine knowledge. |
| **Core** (Rust) | Registry: per-plugin descriptor table (ledgered), arg marshalling, the `Engine.call`/`Engine.status` natives. |
| **Shim** (C++) | Resolution + invoke: sig scan, RTTI vtable-by-name, `validate` predicates, `IsAddressInServerText`, the actual call. |

**Boundary invariant:** class/field/signature names cross core as **opaque strings**, exactly as
`__s2_schema_offset` already does ("class/field are OPAQUE JS strings — no game identifiers appear
in core"). `make check-boundary` stays green; no game identifier is compiled into core.

This slice does **not** depend on exposing `Schema.offset` to plugins — the `via` hop resolves
inside core.

## 11. Resolution timing

Function addresses (signature or vtable) are resolvable at **plugin load**, since `libserver.so` is
loaded. Schema offsets are **not** — schema resolves at map-live, not at Load. Therefore:

- `Engine.call(name)` returns callable-or-null based on what is decidable at load: **target
  resolution + validation + authorization** (§6). It does *not* depend on the schema hop.
- `receiver.via` resolves **lazily at first call** via the cached `__s2_schema_offset`. A miss
  degrades that invocation and flips `Engine.status(name)` to a named reason.

This split is why a `via` descriptor can return a non-null callable that still no-ops on its first
invocation: authorization and the function address were known at load, the sub-object offset was not.

## 12. Error handling

| Stage | Failure | Result |
|---|---|---|
| Build | `vtable` without `validate`; unknown arg/return type; >5 integer-class or >8 float args (§4); `calls` without `engine:calls`; malformed JSON | **build fails** — no `.s2sp` |
| Load | signature miss; no entry for this platform; `validate` predicate rejects; slot outside `.text`; plugin not allow-listed | descriptor `degraded(reason)`; `Engine.call()` → `null`; reason logged once |
| Call | receiver ref stale; `via` offset unresolved | no-op → `void` / `null` |
| Unload | — | ledger drops the descriptor table; reload re-resolves |

Degradation is always per-descriptor with a named reason. One bad descriptor never disables the
plugin's other calls, and never the framework.

## 13. Testing

- **SDK (vitest):** gamedata parse; `vtable`-without-`validate` rejection; `calls`-without-permission
  rejection; arg-vocabulary and arity rejection; golden `.d.ts` generation.
- **Core (`cargo test -p s2script-core`):** descriptor registry; arg marshalling for each vocabulary
  member; all four degrade paths; ledger teardown on unload/reload.
- **Negative validation test:** the documented ItemServices index 24 must be **rejected** by
  `prologue` validation. This is the test that proves the §3 posture — `.text`-range alone passes it.
  Note this test does **not** require knowing the real `DropActivePlayerWeapon` prologue: it asserts
  *rejection*, so any prologue that differs from the bytes actually at slot 24 demonstrates the
  guard. Not knowing the true function is precisely the condition being defended against.
- **Live gate (Docker CS2):** `sm_burn` fires on a real player; a deliberately-corrupted `prologue`
  degrades that one call with a named reason while the plugin's other commands keep working.

## 14. Out of scope (YAGNI)

- Non-entity receivers (named Valve interface / game system) — additive `receiver.kind` later.
- Struct-by-value args, out-params, pointer args, `string` **returns** (a returned pointer needs
  provenance validation).
- Stack-passed args beyond the §4 register budget; overloads; variadics.
- Validators beyond `prologue` (string-xref / RTTI-membership predicates) — these need xref-search
  machinery the shim does not have today.
- Plugin-declared **hooks/detours**. This slice is calls only — hooks remain core-owned.
- Plugin-declared `interfaces`/`offsets` sections; v1 accepts `signatures` + `calls` only.
- Exposing `Schema.offset` to plugins (separate slice).
- Registry-side gamedata staleness reporting / auto-regeneration.

## 15. Success criteria

1. A plugin ships its own gamedata, declares `engine:calls`, is operator-allow-listed, and calls a
   sig-resolved engine function from TypeScript with generated types.
2. `sm_burn` works via a plugin-declared `Ignite` — a previously-deferred feature landed without a
   core engine-op.
3. A `vtable` descriptor with no `validate` fails the build.
4. ItemServices index 24 is rejected at load by `prologue` validation, with a named reason.
5. A degraded descriptor yields `null` from `Engine.call()` and never a crash; the rest of the plugin
   runs.
6. `make ci` green, including `check-boundary` (no game identifiers in core).
7. An older runtime loads the new `.s2sp` unchanged (extra zip member ignored).
