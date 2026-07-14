# Sound — EmitSound + Precache (design)

- **Date:** 2026-07-13
- **Status:** approved (brainstorm) — pending spec review
- **Slice:** Sound. The biggest remaining CounterStrikeSharp functional-parity gap: s2script has zero
  sound support today. Plugins should be able to play a CS2 SoundEvent to a player / a set of
  players / everyone, and register custom sounds for precache.
- **Worktree/branch:** `/home/gkh/projects/s2script-sound` on `feat/sound` (off `origin/main`).

## Goal & scope

CS2 sound is the **SoundEvent** system (`soundevents_*.vsndevts`, events referenced by name → the
engine resolves name→hash internally), not raw `.wav` playback. This slice ships both halves the user
scoped:

- **Emit (v1 — the bot-provable core):** play a named **built-in** CS2 SoundEvent from a source
  entity to a recipient set (a player / a set of players / everyone), with volume. Returns the sound
  GUID (`SndOpEventGuid`, a `uint32`) or 0 on failure.
- **Precache (v2 — the second half):** register **custom** sound/resource paths for precache at map
  load, so a plugin's own `.vsndevts`/`.vsnd` content is loaded before it is emitted.

**Deferred** (do NOT build ahead): 3D positional params (origin/attenuation), pitch (CSSharp's own
code comments that pitch is effectively ignored by the game — `EmitSound_t.m_nPitch` is dead),
stop-sound-by-handle (`SoundOpGameSystem::StopSoundEvent`), soundevent param strings
(`SetSoundEventParamString`), and per-recipient positional/attenuation filters.

## References (studied before designing; verified against the tree)

- **CounterStrikeSharp v1.0.363** (`/home/gkh/projects/CounterStrikeSharp`). Its emit path is
  `CBaseEntity.EmitSound(name, RecipientFilter, volume, pitch)` → collapses recipients to a `ulong`
  slot mask → native `EmitSoundFilter(mask, entIndex, name, volume, pitch)` → rebuilds a
  `CRecipientFilter` from the mask → the sig-resolved
  `SndOpEventGuid_t __fastcall CBaseEntity_EmitSoundFilter(CRecipientFilter&, CEntityIndex, const EmitSound_t&)`.
  It deliberately avoids `CSoundEmitterSystem::EmitSound`, `EmitSoundByHandle`, and the
  `SoundOpGameSystem::*` family (the only reference to `engineSound->EmitSound` is dead/commented-out
  code). Sound identity is the **name string** in `EmitSound_t.m_pSoundName`; the engine does
  name→hash internally; CSSharp only reads back the returned `uint32` GUID. Built-in soundevents need
  **no** plugin precache. Custom resources are added via the Source2 `BuildGameSessionManifest`
  game-system event → `IEntityResourceManifest::AddResource(const char*)` (vtable slot 0), surfaced
  as the `OnServerPrecacheResources(ResourceManifest)` listener.
- **ModSharp git118** gamedata (`/home/gkh/Downloads/ModSharp-git118-linux/sharp/gamedata/`) +
  decompiled managed API (`Sharp.Shared.dll` / `Sharp.Core.dll` via `ikdasm`) +
  `libmodsharp.so` (demangled via `c++filt`/`objdump`). Ships sig hints for
  `CBaseEntity::EmitSoundFilter`, `CSoundEmitterSystem::EmitSound`, the `SoundOpGameSystem::*` family,
  `CBaseEntity::Precache` (vtable 7), `CGameRulesGameSystem::OnPrecacheResource` (vtable 7),
  `CGameResourceService::PrecacheEntitiesAndConfirmResourcesAreLoaded` (engine vtable 35), and the
  game-system-factory machinery `UTIL_GetGameSystemFactory` + `GameSystemFactory::m_FactoryList`
  (`CBaseGameSystemFactory::sm_ppFirst`). These are **HINTS** per `[[re-gamedata-strategy]]`, not
  numbers to trust — every fact is re-resolved UNIQUE + `.text`-guarded against our pinned
  `libserver.so` and gate-validated at load. **Decompile confirmed** (a) ModSharp's precache = a
  vtable hook of the *existing* `CGameRulesGameSystem::OnPrecacheResource(this, IResourceManifest*)`
  (NOT a new game system), with `IResourceManifest::AddResource(const char*)` at **vtable slot 0**
  (native disasm: `mov (%rdi),%rdi; mov (%rdi),%rax; mov (%rax),%rax; jmp *%rax`); and (b) ModSharp's
  per-entity emit demangles to `CBaseEntity::EmitSound(const char* name, const float* volume, const IRecipientFilter*)`
  (a member fn — no `EmitSound_t` struct), which it cross-references to the same address CSSharp calls
  `CBaseEntity_EmitSoundFilter`. Its recipient filter is the modern `IRecipientFilter*`; a `null`
  filter = broadcast to everyone (validating our all-valid-clients default).

## Emit — mechanism

### The sig-resolved engine function

`CBaseEntity::EmitSound(CRecipientFilter& filter, CEntityIndex ent, const EmitSound_t& params) → SndOpEventGuid_t`
(CSSharp's `CBaseEntity_EmitSoundFilter`). Primary sig hint (CSSharp gamedata, linux/server):

```
55 48 89 E5 53 48 89 FB 48 83 EC ? E8 ? ? ? ? 48 89 D8 48 8B 5D ? C9 C3 CC CC CC CC CC CC 48 B8
```

Cross-check hint (ModSharp `CBaseEntity::EmitSoundFilter`, linux/server):

```
55 48 89 E5 41 57 66 41 0F 7E C7 41 56 4D 89 C6
```

**Two candidate prototypes** — the two frameworks disagree on the exact shape:

1. **ModSharp (PREFERRED — best framework fit):** member
   `SndOpEventGuid_t CBaseEntity::EmitSound(const char* name, const float* volume, const IRecipientFilter*)`
   — needs **no `EmitSound_t` struct** (just primitives + the stable `IRecipientFilter` vtable; `this`
   = the source entity ptr, which we already resolve via the serial-gate). ModSharp cross-references
   this to the same address as CSSharp's key.
2. **CSSharp (FALLBACK — source-verified):** static
   `SndOpEventGuid_t CBaseEntity::EmitSound(CRecipientFilter&, CEntityIndex, const EmitSound_t&)`
   (fn-ptr type declared in `entity_manager.h:257`). Needs the ported `EmitSound_t`.

**Decision (per the charter's maintenance-treadmill / "layout is data, own a thin layer" goals): the
member prototype (1) is PREFERRED** — dropping the `EmitSound_t` struct removes a version-sensitive
field-offset layout from our maintenance surface (a struct layout can silently drift per CS2 update;
a member call over primitives + a stable interface vtable does not). **The implementer resolves the
sig UNIQUE against our `libserver.so` and disassembles the prologue to CONFIRM the true prototype AND
the semantics of the `const float*` arg** (ModSharp treats it as volume; classic Source used that slot
for sound *duration*). Use (1) if it resolves + its `const float*` genuinely controls volume;
otherwise fall back to the CSSharp `EmitSound_t` static path (2) for definitive volume control. **The
op/native/module interface is agnostic to which one wins** — both take a source entity + a recipient
filter + a name (+ volume); the discrepancy is contained entirely inside the shim's private call. If
both drift, re-locate via the string refs (`EmitSoundByHandle`, `public.distance_volume_mapping_curve`,
`Playing sound on non-networked entity %s`). Stored in `gamedata/core.gamedata.jsonc` `signatures`
(key `EmitSound`), resolved in `Load()` mirroring `CommitSuicide`/`GiveNamedItem`, `.text`-guarded on
every call via `IsAddressInServerText`.

### Ported structs (shim-only; Source2 types)

The CS2 SDK does not ship the modern `EmitSound_t`; the shim ports a minimal copy (from CSSharp's
`entity_manager.h`):

```cpp
struct EmitSound_t {
  const char*      m_pSoundName;      // soundevent name; engine resolves name→hash
  Vector           m_vecOrigin;       // (deferred: 3D positional) — zeroed
  float            m_flVolume;        // 1.0 default
  float            m_flSoundTime;     // 0
  CEntityIndex     m_nSpeakerEntity;  // -1
  SoundEventGuid_t m_nForceGuid;      // 0
  CEntityIndex     m_nSourceSoundscape;// -1
  uint16           m_nPitch;          // (dead in engine) — 0
  uint8            m_nFlags;          // 0 = attach to ent; (1<<4) = emit at m_vecOrigin (deferred)
};
struct SndOpEventGuid_t { SoundEventGuid_t m_nGuid; uint64 m_hStackHash; uint64 pad; };
typedef uint32 SoundEventGuid_t;
```

The recipient filter is a minimal `CRecipientFilter : IRecipientFilter` (our SDK's
`public/irecipientfilter.h` supplies the modern 4-method interface: `GetNetworkBufType` /
`IsInitMessage` / `GetRecipients` / `GetPredictedPlayerSlot`), storing a `CPlayerBitVec m_Recipients`,
`AddRecipientsFromMask(uint64)` slot-by-slot (bounded 0..63) — ported from CSSharp's
`recipientfilters.h`.

### Reused s2script patterns

- **Serial-gate the source entity:** `CEntityHandle h(idx, serial)` → `s2_deref_handle(h.ToInt())` →
  null (stale/free slot) → no-op return 0. Exactly the `pawn_commit_suicide` pattern. Sentinel:
  `entSerial < 0` → use `entIndex` directly without the serial check (for worldspawn / a global 2D
  sound emitted from index 0).
- **Bot-skip when building the filter:** for each requested slot, add it only if
  `s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot)) != null`. Bots have no netchannel — they can't hear
  the sound AND a send to a null-netchannel fake client risks a crash (the `user_message_send` /
  `client_print` precedent). The source entity being a bot pawn is fine (map entities emit sounds
  constantly); only **recipient** netchannels are the hazard. **The engine fn is called even if the
  filter ends empty after the skip** (as long as the caller requested >=1 slot + the entity resolved +
  the sig resolved): a `CRecipientFilter` with an empty bit-vec is a normal engine path (a PVS/PAS
  filter that excludes everyone → plays to nobody, no netchannel touched). This is deliberate — it is
  the only way a **bots-only** live gate exercises the resolved `EmitSound` (its sig, its 24-byte sret
  ABI, its prototype, no-crash); early-outing on an empty filter would leave the entire emit path
  unproven until a human client. The shim logs the call (`EmitSound '<name>' recipients=N -> guid=G`)
  so the gate can observe the resolved fn fired.
- **`.text`-guard** the resolved fn before every call.

### Op (ABI-appended after `entity_set_model`, the current struct tail)

```c
/* sound_emit: play a named CS2 SoundEvent from a serial-gated source entity to a slot set.
 * Sig-resolved CBaseEntity::EmitSound (preferred member overload (name, volume*, IRecipientFilter*);
 * CSSharp EmitSound_t static path as fallback — see spec). soundName = the soundevent name (engine
 * resolves name→hash). entSerial < 0 = emit
 * from entIndex without a serial check (worldspawn / global). slots[0..slotCount) = recipient slots
 * (bot slots are skipped — no netchannel). volume in [0,1]. Returns the SndOpEventGuid (nonzero) or
 * 0 (unresolved sig / stale entity / caller requested no recipients, i.e. slotCount<=0). NOTE: if the
 * caller requested >=1 recipient but ALL are bot-skipped, the engine fn is STILL called with an empty
 * filter — a normal, harmless engine path (plays to nobody), and the ONLY thing that makes the emit
 * path bot-provable at the live gate. ENGINE-GENERIC. */
typedef int (*s2_sound_emit_fn)(const char* soundName, int entIndex, int entSerial,
                                const int* slots, int slotCount, float volume);
```

Appended in three places in lock-step (the ABI discipline): the C header typedef + struct member,
the Rust `S2EngineOps` mirror, and BOTH in-isolate test op-structs.

### Native + module

- **Native** `__s2_sound_emit(soundName, entIndex, entSerial, slotsArray, volume) → number`
  (`core/src/v8host.rs`) — reads the JS slot array into a `Vec<i32>` (mirrors `user_message_send`),
  calls the op, returns the guid (0 = failed).
- **Engine-generic module `@s2script/sound`** (core prelude, `__s2pkg_sound`; a slot set + a
  soundevent name are Source2-generic, `EntityRef` is engine-generic):
  - `Sound.emit(name, opts)` — `opts = { entity?: EntityRef, recipients?: number[] /*slots*/, volume?: number }`.
    - no `entity` → emit from worldspawn (index 0, serial sentinel) = a global/2D sound.
    - no `recipients` → all valid client slots (enumerate `__s2_client_valid`, mirrors `Chat.toAll`).
    - default `volume` 1.0. Returns the guid (number) or 0.
  - Types-only package `packages/sound/{package.json,index.d.ts}`.
- **CS2 sugar** (`games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`):
  - `Pawn.prototype.emitSound(name, opts)` — emit from this pawn (its serial-gated `EntityRef`),
    same opts minus `entity`.
  - a small curated `Sounds` constant of a few known-good built-in soundevents (for convenience +
    the demo). CS2 soundevent names live exclusively in the CS2 layer, never in `core/src`.

## Precache — mechanism

CS2 precaches custom resources when the session resource manifest is built at map load. The target
is the EXISTING `CGameRulesGameSystem::OnPrecacheResource(IResourceManifest*)` — no new game-system
registration.

> **Mechanism amendment (Task-5 offline RE + reviewer's Critical #2, verified on the pinned
> build-2000873 `libserver.so`).** The original plan hooked this via *the live instance* (walk the
> game-system factory list → its `m_pInstance` → a manual `SH_ADD_HOOK`). That is **not implementable
> on this binary** and was replaced by a **class-vtable slot swap**:
> - **The factory cannot yield the instance.** `CGameRulesGameSystem`'s factory is a
>   `CGameSystemReallocatingFactory<CGameRulesGameSystem>` (factory vtable `0x24c9f88`; slot 8
>   `IsReallocating` → `mov $1;ret`, slot 9 `GetStaticGameSystem` → `xor eax;ret` = **nullptr**). The
>   `+0x18` field is `m_ppGlobalPointer`, **statically zeroed** at the sole construction site
>   (`movq $0x0, 0x2867798` @`0x18edbb0`) and never re-pointed — so `m_pInstance@24` was a *misread
>   hint*, and the factory holds no live instance. (The factory is also registered as
>   `"GameRulesGameSystem"` — **no leading `C`** — @`0x90f33e`, so the `"CGameRulesGameSystem"` strcmp
>   in the walk could never match either.)
> - **An inline detour can't patch the slot body.** `OnPrecacheResource`'s prologue @`0x18d48e0`
>   *starts* with a rip-relative `mov [rip+0xf92e79],rdi`, and the shim's inline-detour engine
>   (`s2detour`) refuses to relocate any rip-relative stolen instruction.
> - **A per-instance manual `SH_ADD_HOOK` is fragile** — the *reallocating* factory recreates the
>   instance per map, dropping an instance-scoped hook.
>
> **The grounded mechanism: swap the shared CLASS vtable slot.** Resolve the `CGameRulesGameSystem`
> class vtable by RTTI — `s2vtable::GetVTableByName("libserver.so", "CGameRulesGameSystem")` → the
> primary vtable `0x24c9d68` (offset-to-top 0, behind RTTI name `"20CGameRulesGameSystem"` @`0x83cef0`)
> — read the gamedata vtable **index** (`CGameRulesGameSystem_OnPrecacheResource` = 7, a validated
> HINT), `.text`-validate the resolved slot fn (`0x18d48e0`), then `mprotect` the RELRO page and
> **overwrite `vtbl[7]` with our free handler**, saving the original to chain. This is the same
> RTTI self-resolve the trace slice uses (`GetVTableByName` + a gamedata index), needs **no live
> instance / no factory walk / no SourceHook / no prologue relocation**, and — because it patches the
> shared class vtable — **survives the factory's per-map instance realloc**. The class vtable is
> static data present at module load, so it installs **once at `Load`** (no lazy retry). On `Unload`
> we write the saved original back.

- **The hook handler** (a free `void(void* this, void* manifest)` receiving the SysV register args)
  stashes `pManifest`, fires `PRECACHE_MUX`, clears the stash, then **chains to the saved original**
  (so the game's own resource precache still runs). `IResourceManifest::AddResource(const char*)` is
  at **vtable slot 0** (disasm-confirmed) — the `sound_precache_add` op calls it on the stashed
  pointer. Note: the manifest is `IResourceManifest*`, NOT `IEntityResourceManifest*` (the latter is
  the entity-level `CBaseEntity::Precache(CEntityPrecacheContext*)` path — a different mechanism we do
  not use).
- **Fallback (footnote only, not planned):** CSSharp registers its own game system
  (`IGameSystem_InitAllSystems_pFirst` + a static factory) to catch `BuildGameSessionManifest`.
  Heavier; used only if the class-vtable swap fails to resolve on our binary.

### Core notify-mux + op + API

- **`PRECACHE_MUX`** (mirrors `MAP_MUX` / `event_mux`): the shim's precache hook stashes the live
  manifest pointer + calls the FFI export `s2script_core_dispatch_precache` →
  `v8host::dispatch_precache` → fans out to JS `Sound.onPrecache` subscribers (snapshot-release,
  `try_borrow_mut` re-entrancy guard, `is_live` liveness, per-sub TryCatch — the
  `dispatch_map_start` pattern). Notify-only (no HookResult). Torn down on unload/shutdown.
- **Op** (ABI-appended after `sound_emit`):

```c
/* sound_precache_add: add a resource path (e.g. "soundevents/mypack.vsndevts") to the session
 * resource manifest currently being built. Valid ONLY during a precache-hook dispatch (the manifest
 * pointer is live only then; block-scoped like a game event). Returns 1 on add, 0 if no active
 * manifest / unresolved. ENGINE-GENERIC. */
typedef int (*s2_sound_precache_add_fn)(const char* path);
```

- **API:** `Sound.onPrecache(handler)` where `handler(ctx)` gets a block-scoped `PrecacheContext`
  with `.add(path)` → the `sound_precache_add` op. Synchronous-only (a stashed `ctx` used after the
  hook returns is a no-op — the manifest is gone). Fires at map load / mapchange.

## Architecture & layer boundaries

- **Core (engine-generic):** the `sound_emit` + `sound_precache_add` ops, `PRECACHE_MUX` +
  `dispatch_precache`, the `@s2script/sound` module (`Sound.emit`/`onPrecache`), and the natives. A
  recipient slot set, a soundevent name, and a resource path are all Source2-generic; `EntityRef` is
  engine-generic. **No CS2 identifiers in `core/src`.**
- **Shim (Source2 engine types):** the sig-resolution + `.text` guards, the `EmitSound_t` /
  `CRecipientFilter` ports, and the precache game-system hook. `CGameRulesGameSystem`,
  `IResourceManifest`, `GameSystemFactory` / `CBaseGameSystemFactory`, `IRecipientFilter` /
  `CRecipientFilter`, `EmitSound_t` are Source2 engine types → shim-only.
- **CS2 (game layer):** `pawn.emitSound` + the curated `Sounds` list, in `games/cs2/js/pawn.js` +
  `packages/cs2`.
- Both boundary gates (`check-core-boundary.sh`, `test-boundary-nameleak.sh`) stay green.

## Degrade-never-crash

Every path degrades to a no-op, never a crash: unresolved emit sig → return 0 (no engine call);
stale/free source entity (serial mismatch) → return 0; caller requested no recipients (`slotCount<=0`)
→ return 0; unresolved precache hook → the mux simply never fires (no `onPrecache` delivery), emit of
built-in sounds still works; `sound_precache_add` outside a live hook → return 0. (An all-bot-skipped
filter is NOT a degrade — the engine fn is still called; see Emit → bot-skip.) All natives
`catch_unwind`; the shim guards `!s_pEngine` / null fn ptr / out-of-`.text`.

## Testing (in-isolate, `RUST_TEST_THREADS=1`)

- `sound_emit` / `sound_precache_add` degrade-to-noop (return 0) when the ops are null.
- `@s2script/sound` module surface: `Sound.emit` builds the slot list (default all-valid) + calls the
  native; `Sound.onPrecache` registers into `PRECACHE_MUX`; `dispatch_precache` runs a subscriber.
- ABI struct parity: both in-isolate test op-structs include the two new members in tail order.

## Live gate (shared server — coordinate first)

One shared `s2script-cs2` (de_inferno, bots). Do NOT `docker compose restart cs2` while another gate
is mid-run — check first. Bot-provable gate:

- The new sigs resolve — `GAMEDATA VALIDATION` count goes up, **0 FAILED**.
- A demo `sm_playsound <name> [slot]` **actually invokes the resolved `EmitSound`** (the shim logs
  `EmitSound '<name>' recipients=0 -> guid=G`; recipients=0 because bots are skipped, but the fn IS
  called → its sig / 24-byte sret ABI / prototype / no-crash are proven) with no crash
  (`RestartCount=0`).
- The precache hook fires at map load — `Sound.onPrecache` logs (and `ctx.add(path)` returns 1).

**Deferred to a human-client test** (bots have no audio — the same ceiling as SayText2's visible chat
per `[[deferred-live-tests]]`): the *audible* sound on a real client and correct volume, and a custom
precached sound actually playing. (The engine call itself + its ABI are proven at the bots gate above;
only audibility is deferred.)

## Sequencing & build

- Branch is off `origin/main`. **PR #23 (`feat/entity-lifecycle-listeners`) is OPEN** — it appends to
  the same S2EngineOps ABI tail, adds `gamedata/core.gamedata.jsonc` signatures, and edits `Load()` +
  `install_natives`. **Rebase `feat/sound` onto PR #23 before the live gate** if it hasn't merged
  (`gh pr view 23 --json state`); resolve the ABI-tail / gamedata / `Load()` conflicts by appending
  after PR #23's members.
- **Sniper rebuild** required (core + shim):
  `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`.
  The `hl2sdk` submodule is not checked out in a fresh worktree — `git submodule update --init` it (or
  point at the main checkout) before building.
- Full slice cadence per `[[slice-workflow-cadence]]`: design → spec → plan → subagent-driven
  implementation → live gate → PR (+ a changeset since `packages/sound` and `packages/cs2` change).

## Orchestration (user directive)

The implementation **plan is authored by Fable**, and Fable **orchestrates the implementer models**
(dispatches the implementer subagents). **Fable must not implement code itself** — implementer agents
(opus/sonnet) write the code; Fable plans + coordinates + reviews.
