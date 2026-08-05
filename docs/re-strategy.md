# Reverse-engineering & gamedata strategy — the one true way to resolve engine facts

**Status:** the governing doctrine for every engine touchpoint (schema, signatures, offsets, vtable indices,
interface strings). Written after the Slice-6.8 `sm_slay` failure exposed the recurring class of bug.

## The problem, precisely

Our CS2 binary is a **specific build** (currently `2000860` / patch `1.41.6.7`, from `csgo/steam.inf`).
Every engine fact we consume is one of two kinds, and only one kind is safe:

| Kind | How it's resolved | Version-correct? | Example |
|---|---|---|---|
| **Self-resolving** | re-resolved against *the loaded binary* | **always** — cannot be version-wrong | schema offsets (live `SchemaSystem` dump → `schema-catalog.json`); function addresses via byte-signature (`FindPattern`) or string-xref (`ResolveCtorXref`) |
| **Hardcoded constant** | a number *copied into gamedata* | **only** for the exact build it was derived from | struct offsets (`336`, `168`…); a raw vtable index (`CommitSuicide: 400`) |

**Every RE failure we have hit lives in the second row.** `sm_slay` is the canonical case: we copied vtable
index `400` from ModSharp's gamedata, which was generated for *ModSharp's* CS2 build — not `2000860`. The
number was never valid for our binary; `vtable[400]` on our build is a valid-but-wrong function, so the call
silently did nothing (the `.text` guard prevented a crash but nothing announced "this is broken").

Schema and our sig-scans **cannot** have this bug: they re-resolve against whatever `libserver.so` is loaded.
(Verified: our `DispatchTraceAttack` signature matches *uniquely* on `2000860`.)

## Why mature frameworks (SourceMod, CounterStrikeSharp, ModSharp) don't hit this

Not magic — discipline we skipped:
1. **They regenerate/validate gamedata per CS2 patch, against the exact binary they run.** It's a treadmill.
2. **They never mix framework-A's numbers with binary-version-B.** We did.
3. **They prefer signatures, and increasingly RTTI-based vtable resolution, over raw indices** — because a
   sig/RTTI scan self-heals across updates while a raw index breaks on any reorder.

We already do (1)/(3) for **schema** (regenerable dump) and **signatures** (regenerable scan). The two gaps:
we hardcoded one borrowed index, and we have **no validation gate** to make a stale/mismatched entry loud.

## The principle underneath: the weakest resolver, not the shortest

The three rules below are usually carried as habits — "prefer sigs", "don't hardcode". They are
consequences of a single principle, and naming it is what tells you what to do in a case the rules
don't already cover.

Michael Timothy Bennett, *The Optimal Choice of Hypothesis Is the Weakest, Not the Shortest*
([arXiv:2301.12987](https://arxiv.org/abs/2301.12987), AGI-23): given a set of hypotheses that all
fit what you have observed, the one most likely to generalise to what you have *not* observed is not
the shortest — it is the **weakest**, the one whose extension (the set of situations it admits) is
largest. Compression, he proves, is "neither necessary nor sufficient." The razor:

> Explanations should be no more specific than necessary.

That is our situation exactly. Every engine fact is inferred from a single observation — the build in
front of us — and has to hold on builds nobody has seen yet. Many resolvers fit that one observation
equally well; what separates them is how many *other* builds they also admit. That count is their
weakness, and it is the whole of what "survives the treadmill" means.

The ordering, weakest last:

| Resolver | What it asserts about the binary | Extension — builds it still names correctly |
|---|---|---|
| baked struct offset / vtable index | this exact layout | **one**: the build it was derived from |
| byte pattern with operands pinned | those bytes **and** those inter-address distances | ~one — a `lea rip+disp32` shifts on any recompile |
| byte pattern, operands wildcarded, ties broken structurally | this instruction *shape* | every build where the codegen shape holds |
| string-xref | this string still exists and is referenced here | every build carrying the string |
| RTTI type-name → vtable, `ISchemaSystem` field-by-name, typed SDK virtual | this class/field is still **called** this | every build that keeps the name |

Description length runs the **opposite** way down that table. `400` is the shortest possible
description of `CommitSuicide`'s vtable slot; the RTTI walk that derives it honestly is thirty lines.
That inversion is exactly why "just hardcode it" keeps feeling like the clean choice, and why it is
reliably the one that breaks. Longer code, wider extension: take it.

Two things this does **not** license:

- **Weakness never excuses a resolver that doesn't fit.** In the paper you maximise weakness over the
  *models* of the task — hypotheses that already explain the observation. A pattern matching zero
  times, or matching uniquely at the wrong function, is not a candidate however weak it is. Rule 2
  (validate, fail loud) is what establishes membership; weakness is the tiebreak among survivors.
- **It is an argument, not a theorem about our domain.** Bennett's result holds under a specific
  formalism of enactive cognition and assumes tasks are uniformly distributed; future CS2 builds are
  not. We adopt the razor because it predicts our actual failure record, which is the same standard
  this document applies to every other borrowed claim — another framework's number is a hint until it
  is checked against our binary, and so is another field's proof.

## The doctrine (do this for every engine fact, no exceptions)

**Rule 1 — Prefer self-resolving resolution; never ship a bare borrowed constant.**
- **Functions** → resolve the *address* on our binary by byte-signature or string-xref, then **call it
  directly**. Do NOT store a vtable index. (This is exactly how `DispatchTraceAttack`/`GameEventManager`
  already work — and why they're robust.)
- **Vtable indices** (only when a virtual call is genuinely required) → **derive the index on our binary at
  runtime**: locate the class vtable via its RTTI type_info (the mangled type-name strings — e.g.
  `13CCSPlayerPawn`, `15CBasePlayerPawn` — are present in our `libserver.so`), find the target function's
  address by signature, and scan the vtable for it. Never copy an index from another framework.
- **Schema struct offsets** → the live `SchemaSystem` dump (`schema-catalog.json`), resolved per-access.
  Already correct; keep it.
- **Non-schema struct offsets** (the client-list: `NetworkServerService`/`CServerSideClient`) → no reflection
  exists, so these stay hardcoded — but they MUST be validated (Rule 2) and carry a documented re-derivation
  recipe for the treadmill.

**Rule 2 — Validate every gamedata entry against the loaded binary, and FAIL LOUD.**
A silent no-op (what `sm_slay` did) is the enemy. At load *and* as a treadmill CLI check, resolve every
signature/offset/derived-index and emit a structured pass/fail summary:
- a signature must match **exactly once** in the module (0 = not found → the pattern moved; >1 = ambiguous);
- a resolved function address / vtable entry must land inside `libserver`'s `.text`;
- a non-schema offset, where possible, must dereference to a sane value (a non-null pointer, an in-range int).
Any failure → a named `GAMEDATA descriptor 'X' FAILED: <reason>` line + a one-line `N/M resolved` banner, so a
version mismatch screams at boot instead of surfacing feature-by-feature as silent breakage.

**Rule 3 — Pin the build; other frameworks' gamedata is a HINT, never a number.**
Pin the exact CS2 build. Treat ModSharp/CSSharp/SM gamedata as pointers to *which* function and *what*
string/pattern to look for — then re-resolve the actual address/index/offset against **our** binary. On each
CS2 update, re-run the treadmill: re-dump schema, re-scan signatures, re-derive indices/offsets, run the gate.

**Rule 4 — a repair must be weaker than what it broke.**
This is the rule for the other half of the loop: not how a fact is resolved the first time, but what
you are allowed to replace it with once the treadmill has broken it. The reflex is to re-derive the
same *kind* of thing with fresh numbers. That restores the feature and schedules the identical
failure for the next patch, because the replacement has the same extension as the hypothesis that
just proved too narrow. So the repair step has an acceptance criterion, asked out loud every time:

> **Is the replacement weaker than what it replaced — and if not, why not?**

"Why not" has legitimate answers (no reflection exists for the client-list structs, so those offsets
stay hardcoded under Rule 1). It just has to be *stated*, in the entry's comment, alongside the
re-derivation recipe — an unexplained same-strength repair is the thing this rule exists to catch.

Both repairs we have actually shipped pass it:

- **The client list** broke on build `2000870`: six hand-committed engine-identity offsets moved. It
  was not fixed with six new offsets — the whole path was replaced by typed SDK virtuals, resolved by
  the compiler against the pinned hl2sdk. Extension widened from one build to every build the SDK
  tracks, which is why it has not broken since.
- **`SetModel`** shipped with `48 8D 05 3D ED 28 01` baked in to break a tie between two identical
  candidates. The displacement went stale, the signature resolved to nothing, and `EntityRef.setModel`
  became a silent no-op — invisible corpses downstream. The fix wildcards the operand and breaks the
  tie **structurally**, and `scripts/check-gamedata-sigs.sh` now fails the build on any pinned
  `call`/`jmp` rel32 or rip-relative displacement. That gate is this razor enforced mechanically, for
  the one case where over-specificity is machine-detectable.

The same razor governs **validators**, pointing the other way: a validator that asserts more than it
needs starts refusing correct binaries. A carried-forward `validate` block whose new pattern moved the
instruction it reads must be re-derived, not left to reject a good signature. Assert the semantic that
identifies the function — not every byte you happened to observe next to it.

## Consequences for open work — both closed, and both closed the Rule-4 way

- **`sm_slay`** (was: branch `slice-6.8-slay`, borrowed index `400`, violating Rule 1). **Landed.** Not
  by finding the *right* index — index 400 is a `m_flDeathTime` getter on our build and `CommitSuicide`
  is really 819 — but by dropping the index entirely for a by-address prologue signature
  (`CBasePlayerPawn_CommitSuicide` in `gamedata/cs2/game.cs2.jsonc`), self-resolved on our
  `libserver.so`, with the rip-relative `lea` displacement and the stack-frame immediate masked out. A
  correct index would have been the same-strength repair Rule 4 refuses; the signature is weaker on
  both axes, and the entry carries the full identity derivation.
- The **validation gate** (Rule 2). **Built** — boot-time resolution with a named per-descriptor
  reason, plus `scripts/check-gamedata-sigs.sh` at build time.

**One sentence:** *"Layout is data, semantics are code" — so every layout fact must be either self-resolving
against our binary or validated against it at load (a bare borrowed constant is neither, and that is the bug);
and among the resolvers that clear that bar, take the weakest, never the shortest.*
