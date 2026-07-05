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

## Consequences for open work

- **`sm_slay`** (branch `slice-6.8-slay`, unmerged): its borrowed index `400` violates Rule 1. It must be
  re-landed with `CommitSuicide` resolved on our binary — ideally its *address* (string-xref → direct call,
  no index), or its index derived via the RTTI scan above. Do not merge the borrowed-index version.
- The **validation gate** (Rule 2) is the highest-leverage missing piece — it is what makes the whole update
  treadmill trustworthy, and it is what would have caught `sm_slay` at boot. Build it first.

**One sentence:** *"Layout is data, semantics are code" — so every layout fact must be either self-resolving
against our binary or validated against it at load; a bare borrowed constant is neither, and that is the bug.*
