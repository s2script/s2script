# UserMessage interception (`UserMessages.onPre`) — design spec

**Date:** 2026-07-16
**Status:** design (autonomous decision — no human gate) → implementation plan next
**Scope:** core + shim + `@s2script/sdk/usermessages` hook surface + demo. **No gamedata, no sig-scan, no new interface acquisition, no game-package change.**
**Consumer:** the TTT port — `SilentAWPItem.cs`, `PoisonShotsListener.cs`, `SuppressedRound.cs` (all hook msg 452 = `CMsgTEFireBullets`), `BombPlantSuppressor.cs` (msg 322 = `CCSUsrMsg_RadioText`).

## 1. Problem & consumer

The four TTT consumers use six CSSharp touchpoints, verified against `/home/gkh/projects/TTT/TTT/`:

1. `HookUserMessage(452|322, handler)` — always Pre mode — and `UnhookUserMessage(452, handler)` (SuppressedRound, on round end).
2. `msg.ReadUInt("item_def_index")` — the only typed read used (filter which weapon fired).
3. `msg.DebugString` split into lines and parsed as ` x: <f>` / ` y: <f>` / ` z: <f>` for the shot origin, then reverse-matched to a player by eye-position `DistanceSquared < 1` — a fragile hack that exists ONLY because CSSharp cannot read nested-message fields (`ReadMessage` is commented out, `UserMessage.cs:119-121`), so `origin.x` is unreachable.
4. `msg.Recipients.Clear()` + `return HookResult.Handled` — full suppression (belt-and-braces: either alone silences in CSSharp).
5. `return HookResult.Continue` to pass.

So the required primitive is: **synchronously intercept a named outbound user message, read scalar (and nested) protobuf fields, see the recipient set, and suppress the send via the standard HookResult collapse.** Nothing needs field mutation, post-mode hooks, or per-slot recipient filtering (see §9 deviations and §6 deferred).

## 2. The mechanism — THE key decision

**Verdict: pure interface + vendored-header REUSE. Zero new sig-scans, zero new offsets, zero new interface acquisitions.** One new SourceHook on a vtable method the shipping code already *calls*.

The intercept point is a SourceHook on **`IGameEventSystem::PostEventAbstract`** — the exact 8-arg overload s2script's own live-proven send path already invokes in `s2_client_print` and `s2_user_message_send` (`shim/src/s2script_mm.cpp:922`, `:1027`, on the `s_pGameEventSystem` pointer engineFactory-acquired at Load, `:2768`, key `GameEventSystemServerV001`). CSSharp's production `usermessage_manager.cpp` proves the identical hook covers both the TE range (452) and the CS usermessage range (322) at one choke point.

```cpp
// third_party/hl2sdk/public/engine/igameeventsystem.h:37-40 — the 8-arg overload.
SH_DECL_HOOK8_void(IGameEventSystem, PostEventAbstract, SH_NOATTRIB, 0,
    CSplitScreenSlot, bool, int, const uint64*,
    INetworkMessageInternal*, const CNetMessage*, unsigned long, NetChannelBufType_t);
```

- **Param 7 is `unsigned long` exactly** (matches the vendored header — ABI-critical, never `uint32`/`size_t`).
- SourceHook disambiguates the two overloads (there is a 6-arg `IRecipientFilter*` sibling immediately after) by the **parameter type list** from the member-function pointer — no numeric vtable index is ever written down. Because the send path calls this same overload live, the vendored header's slot for it is **transitively proven against our binary** — the strongest self-consistency argument available.
- Install is **lazy on the first-ever subscription** (the `usercmd_hook_install` precedent, `s2script_mm.cpp` `Shim_UsercmdHookInstall`), idempotent bool-guarded (the `m_eventHookInstalled` pattern at `:1932-1941`); `SH_REMOVE_HOOK` in `Unload()`. This is a sibling of the FireEvent + eight client-lifecycle + voice SourceHooks.
- **Message identity** inside the hook: `pEvent->GetNetMessageInfo()->m_MessageId` (a cheap struct read) for the hot-path gate, and `pEvent->GetUnscopedName()` (a virtual, no layout dependency) for the dispatch key.
- **Field reads:** protobuf reflection on `const_cast`'d `pData` → `AsProto()` — a byte-for-byte READ mirror (`Get*` for `Set*`) of the already-shipping `s2_user_message_set_*` ops (`s2script_mm.cpp` ~:994-1040) on the same statically-linked self-contained libprotobuf 3.21.8. Nested dotted paths (`"origin.x"`) walk `GetReflection()->GetMessage()` exactly as the shipping `s2_usercmd_read` does. **Every read carries the `is_repeated()` guard** — a scalar accessor on a repeated field is a protobuf FATAL that aborts the whole process.

### RE-doctrine compliance — the ONE borrowed layout fact

Everything rides on already-live-proven facts (`PostEventAbstract` slot, `BUF_RELIABLE`, the bit-per-slot clients mask, `GAMEEVENTSYSTEM_INTERFACE_VERSION`) **except one**: `NetMessageInfo_t::m_MessageId` (`inetworkserializer.h:53`) — a vendored-header struct offset the send path never reads. Its blast radius is deliberately tiny: **`m_MessageId` feeds only the hot-path bitmap pre-filter, and that pre-filter is self-consistent** (subscribe and dispatch read the *same* offset), while the **authoritative dispatch key is `GetUnscopedName()`** — a reliable virtual with no layout dependency. A drifted offset therefore degrades to at-worst a wasted dispatch the name-mux drops, or a fail-closed subscribe — **never a false suppression**. Per `docs/re-strategy.md` Rule 2 it is range-checked fail-closed at subscribe, plus an observe-only first-fire sanity banner:

1. **Subscribe-time validation (per-descriptor, fail-closed):** `usermsg_hook_sub(name)` resolves via `s_pNetworkMessages->FindNetworkMessagePartial(name)` (the live-proven SayText2 path), then requires (a) `GetNetMessageInfo()` non-null, (b) `m_MessageId` in `[0, 2048)` (observed CS2 ids are low hundreds; the bitmap bound), (c) the requested name is a substring of `GetUnscopedName()` (partial-match honesty). Any failure → the subscription is REFUSED with a named reason (`USERMSG descriptor 'message-id-extract' FAILED: <reason>` / `USERMSG sub FAILED: <reason>`) and `UserMessages.onPre` throws at plugin load — loud, per-descriptor, framework keeps running.
2. **Observe-only first-subscribed-fire sanity:** the first time a *subscribed* message reaches the hook (after the cheap bitmap gate — a message a plugin actually asked for, **not** the arbitrary first engine post, which may be bodyless and must never disable anything) the hook checks `pData->AsProto()` has non-null Descriptor+Reflection and `GetUnscopedName()` non-empty, logs `[s2script] USERMSG intercept validated (first subscribed fire: id=N name=X)`, and **NEVER dispatches or suppresses that fire** — the hook goes live on the next. A reflection failure skips **that one fire** with a named `USERMSG VALIDATION: ...` line and leaves everything else running — per-fire, never a global latch. *(Review removed the earlier draft's global-degrade + self-comparing round-trip: `rt = FindNetworkMessagePartial(GetUnscopedName())` returns the same engine singleton as `pEvent`, so `rt->…m_MessageId != mi->m_MessageId` compared the field to itself — vacuous when it matched, and a **false global disable** whenever a partial-name match resolved `rt != pEvent`.)*

## 3. Suppress + recipient semantics + HookResult collapse

- **Collapse rule:** the existing `core/src/multiplexer.rs` max-by-precedence collapse, verbatim (Continue 0 < Changed 1 < Handled 2 < Stop 3; Stop short-circuits; Monitor return ignored; an errored handler counts as Continue). NOTE: CSSharp's numeric values differ (Handled=3, Stop=4) — mapped **semantically**, never numerically.
- **Suppress = `RETURN_META(MRES_SUPERCEDE)` when the collapsed result >= Handled(2)** — the message is dropped for every recipient, mirroring the FireEvent-Pre suppress convention (`s2script_mm.cpp` ~:3684-3690). TTT's `Recipients.Clear() + Handled` maps to a bare `return HookResult.Handled`.
- **Recipients are READ-ONLY in v1**: `msg.recipients: number[]` (0-based slots decoded from the `const uint64* clients` mask — bit N = slot N, exactly how our own send builds it at `:1022`; the `clients==NULL && nClientCount==-1` broadcast form decodes to all currently-valid slots shim-side). No TTT consumer needs per-slot filtering; building it now would violate build-by-risk. The documented non-breaking extension is `removeRecipient(slot)` clearing a bit in the `const_cast`'d live mask before `MRES_IGNORED` (CSSharp-proven: the engine's original reads the mutated caller-stack array) — strictly block-scoped, never across an `await`.
- **Live-gate trade-off (flagged):** `PostEventAbstract` also drives server-side local listeners (`RegisterGameEventHandlerAbstract`, `igameeventsystem.h:27`) — SUPERCEDE silences those too, which is broader than "send to nobody". For client-bound user-message protobufs local listeners are rare, but the gate must verify a suppressed message breaks no server-side consumer. Fallback (shim-internal only, zero API change): recall-with-modified-mask — copy the mask, zero it, `SH_CALL` the original, then SUPERCEDE.

## 4. Block-scoped raw-view discipline + re-entrancy

- The live protobuf message and recipient mask enter shim statics (`s_hookMsg`, `s_hookClients`, `s_hookClientCount`) at the top of `Hook_PostEvent` and are **nulled immediately after the synchronous dispatch returns** — the `s_currentUserCmd`/`s_currentDamageInfo` pattern (`s2script_mm.cpp` ~:1472-1487). All read ops null-guard and return failure when no message is current, so a view captured across an `await` reads `null`, never a dangling pointer. Raw pointers never cross to JS.
- **The hook statics are SEPARATE from the send builder's `s_umInfo`/`s_umData`/`s_umMsg` (`:977-979`)** — a handler that builds+sends a NEW UserMessage mid-hook must not retarget the intercepted view (the designed-out bug).
- **Re-entrancy, two guards:** (i) core-side `HOST.try_borrow_mut()` graceful-skip in `dispatch_usermsg` (the doctrine guard every dispatch carries — a send from any JS context re-enters `PostEventAbstract` while HOST is borrowed); (ii) a shim-side `s_inUserMsgDispatch` recursion flag that returns `MRES_IGNORED` before even attempting the core call. Documented consequence (same class as "JS `fire` can't re-dispatch to JS subs"): **plugin/framework-originated messages sent from inside a JS dispatch do NOT re-trigger JS hooks** — a deliberate v1 deviation from SourceMod.

## 5. Hot-path cheap gate

`PostEventAbstract` fires for every outbound event/message, many per tick. Cost tiers:

- **Zero subscribers:** no SourceHook installed at all (lazy install on first-ever subscribe).
- **Hook installed, message not subscribed:** one virtual call (`GetNetMessageInfo`) + one struct read + one bit test against `static uint64_t s_userMsgSubBits[32]` (2048 ids, 256 bytes) → `MRES_IGNORED`. No strcmp, no protobuf, no FFI, no allocation.
- **Bitmap hit:** `GetUnscopedName()` once, set statics, one FFI dispatch into the name-keyed mux; protobuf reflection only when a handler actually reads a field.

The bitmap is maintained by `usermsg_hook_sub`/`usermsg_hook_unsub` engine ops driven from core's empty↔non-empty mux transitions (`event_mux::remove_by_owner` already returns the names that became empty, exactly for this).

## 6. API shape (exact TS surface)

Engine-generic, so it extends the existing `@s2script/sdk/usermessages` subpath (already in the exports map — `packages/sdk/package.json:112-114`; no new package, a `@s2script/sdk` **minor** changeset). PascalCase types, camelCase members. Message names/ids are **caller parameters** — the id→name catalog (452, 322…) stays consumer knowledge, with a codegen'd typed catalog in `@s2script/cs2` as the future home (mirrors the 272-event game-event catalog); never in core/shim.

```ts
// packages/sdk/usermessages.d.ts — additions (UserMessage builder class unchanged)
import type { HookResultValue } from "./events";

/** A BLOCK-SCOPED view of an intercepted outbound user message — valid only during a
 *  UserMessages.onPre handler; across an await (or stashed) all reads return null/[]/"". */
export interface UserMessageView {
  /** Canonical unscoped message name (e.g. "CMsgTEFireBullets"). */
  readonly name: string;
  /** Numeric network-message id (e.g. 452). */
  readonly id: number;
  /** Recipient slots (0-based) this post targets; a broadcast decodes to all live slots. Read-only in v1. */
  readonly recipients: number[];
  /** protobuf TextFormat dump — the documented FALLBACK for unmapped messages only; prefer typed reads. */
  readonly debugString: string;
  hasField(path: string): boolean;
  /** Scalar int read (int32/uint32/fixed32/enum; bool as 0/1). Dotted nested paths supported
   *  ("origin.x" walks sub-messages). null = no such field / repeated / no current message. */
  readInt(path: string): number | null;
  readFloat(path: string): number | null;
  readBool(path: string): boolean | null;
  readString(path: string): string | null;
}

export declare const UserMessages: {
  /** Pre-hook an outbound user message by unscoped name (partial match, SayText2-style; the view
   *  carries the canonical name). Runs SYNCHRONOUSLY before delivery. Return >= HookResult.Handled
   *  to SUPPRESS the send for every recipient; Continue/undefined passes it through. THROWS at
   *  subscribe time on an unresolvable name or a degraded intercept descriptor. */
  onPre(name: string, handler: (msg: UserMessageView) => HookResultValue | void): void;
  /** Removes ALL of the calling plugin's handlers for this name (mux off semantics — handler
   *  identity not compared, matching Events.off). */
  off(name: string, handler?: (msg: UserMessageView) => HookResultValue | void): void;
};
```

TTT parity mapping: `HookUserMessage(452, h)` → `UserMessages.onPre("CMsgTEFireBullets", h)`; `HookUserMessage(322, h)` → `onPre("CCSUsrMsg_RadioText", h)`; `ReadUInt("item_def_index")` → `readInt("item_def_index")`; `Recipients.Clear()+Handled` → `return HookResult.Handled`; `UnhookUserMessage` → `UserMessages.off` (and unload teardown makes the manual off optional, not load-bearing — the ledger is the teardown authority).

## 7. Architecture — new ops, where everything lives

| Piece | Where | Why |
|---|---|---|
| `SH_DECL_HOOK8_void` + `Hook_PostEvent` + bitmap + block-scoped statics + first-fire validation + recursion flag | **shim** (`s2script_mm.cpp`/`.h`) | The hot path must be shim-local; the interface pointer + libprotobuf already live there. |
| 8 ops: `usermsg_hook_sub`/`unsub` + `read_int`/`read_float`/`read_string`/`has_field` + `recipients` + `debug` | `S2EngineOps` tail — **appended after `transmit_stats`** (`shim/include/s2script_core.h:259/:386`, `core/src/v8host.rs:372`, wiring `s2script_mm.cpp:3589`, every test op-struct — grep `transmit_stats: None\|mock_transmit_stats`) | Positional C ABI; see the collision note below. |
| `USERMSG_MUX` (name-keyed, collapsing) + `dispatch_usermsg(name, id) -> i32` + `s2script_core_dispatch_usermsg` ffi export | `core/src/v8host.rs` + `core/src/ffi.rs` | A verbatim copy of the `OUTPUT_MUX`/`dispatch_output` shape (v8host.rs:624/~6608, ffi.rs:185) incl. `try_borrow_mut` skip and errored=Continue. |
| `__s2_usermsg_on/off/read_int/read_float/read_string/has_field/recipients/debug` natives + `UserMessages` in the `__s2pkg_usermessages` prelude (v8host.rs:1309) | `core/src/v8host.rs` | Engine-generic module surface; core-side name→id map for ledgered teardown. |
| `.d.ts` | `packages/sdk/usermessages.d.ts` (§6) | Existing subpath; minor changeset. |
| demo | `examples/usermsg-demo` | The 4-consumer shapes + the D-09 probe. |

**ABI-tail collision (flagged, load-bearing):** `transmit_stats` is the origin/main tail TODAY, but the unmerged **round-control (#67)** and **voice (#71)** stacks append at the same tail. `S2EngineOps` has no size/version handshake (copied by value at init) — whichever stack merges later MUST re-anchor its ops after the newly-merged tail in **every** place (C typedef + C member + Rust alias + Rust field + all test op-structs + shim `ops.*` wiring). A missed re-anchor is a silent function-pointer misdispatch, not a compile error. The plan's per-PR gate re-verifies the three-way tail order at merge time.

## 8. Boundary check

*Would it still be true on a different Source 2 game?* Yes for every core/shim/sdk piece: `IGameEventSystem`, `INetworkMessageInternal`, `CNetMessage`, protobuf reflection, and "intercept a named outbound net message" are Source2 concepts. Core speaks only strings (names/paths), ints (ids/slots), and floats — no CS2 identifier crosses the C ABI or appears in `core/src`; the shim compares only ids it was handed. `"CMsgTEFireBullets"`, `"CCSUsrMsg_RadioText"`, 452/322, and the `player` handle decode live in the **demo/TTT plugins** (and later a codegen'd `@s2script/cs2` catalog). Gates: `make check-boundary` + `scripts/test-boundary-nameleak.sh` stay green.

## 9. Fix-don't-port: D-09 + every deviation logged

- **D-09 (RESOLVED — fix, don't port):** msg 452 is `CMsgTEFireBullets` (`third_party/hl2sdk/game/shared/cs/cs_gameevents.proto:20-51`) and carries **`optional fixed32 player = 6 [default = 16777215]`** — the shooter as a packed entity handle, a top-level scalar. The port reads `readInt("player")` + decodes via the existing `__s2_handle_decode`, plus `readInt("item_def_index")` for the weapon filter — **`findPlayerByCoord` and the whole DebugString eye-position reverse-match are deleted.** Two live validations before the TTT consumers ship on it: the fixed32 index/serial bit-split (the one documented sample, 6390016, is ambiguous between 14/15-bit layouts) and whether `player` references the pawn or the controller. `debugString` ships anyway as the documented fallback for unmapped messages, and dotted nested reads (`readFloat("origin.x")`) make the origin readable typed — the capability CSSharp lacks that forced the hack.
- **D: `Recipients.Clear()` has no equivalent** — suppression is spelled `return HookResult.Handled` (consistent with every other s2script pre-hook); recipients are read-only v1.
- **D: name-keyed, not id-keyed** — `onPre("CMsgTEFireBullets")` not `HookUserMessage(452)`; ids stay data, and the mux matches every other string-keyed mux.
- **D: getters-only view v1** — no TTT consumer mutates fields; the `set*` reflection twins are near-free later (the send-path setters already exist) but are deferred per build-by-risk.
- **D: Pre-only** — no CSSharp `HookMode.Post` (no suppress value, pure mux surface growth).
- **D: plugin-originated sends inside a JS dispatch are not re-hookable** (§4) — deviation from SourceMod, documented in the `.d.ts`.
- **D: HookResult numerics differ from CSSharp** (Handled 2 vs 3) — semantic mapping only.
- **D (BombPlantSuppressor): blanket radio-text block ported as-is**; precision filtering on `msg_name` needs a live capture of the bomb-plant token — logged as a consumer-level follow-up, not built.

## 10. Deferred (do NOT build ahead)

- Per-slot recipient removal (`removeRecipient`) — the const-cast in-place mask mutation, API-additive later.
- Field setters (`setInt`…), repeated-field reads/mutation, byte reads. **64-bit reads** (int64/uint64/fixed64) are deferred per the decimal-string doctrine (a value > 2^53 can't cross as an f64 without loss) — `readInt` **refuses** them (returns `null`), never truncates; a decimal-string read op is the additive future home. Zero TTT consumers read 64-bit.
- `HookMode.Post`.
- A codegen'd typed message catalog in `@s2script/cs2` (protos are already vendored in-tree).
- Hooking `PostEventAbstract_Local`/`PostEntityEventAbstract`/the 6-arg `IRecipientFilter` overload — no evidence any TTT-relevant message flows through them (TTT works in production on CSSharp, which hooks only the 8-arg overload); revisit only on live-gate evidence.

## 11. Live-gate checklist (main loop's job) / STOP conditions

1. Boot/first-subscribe log: `usermsg: PostEventAbstract hook installed (lazy, first subscribe)`, then on the first *subscribed* message `USERMSG intercept validated (first subscribed fire: id=N name=X)`. **Confirm the id/name are the ones you expect** for that message (e.g. `452`/`CMsgTEFireBullets`, `322`/`CCSUsrMsg_RadioText`). A wrong id here is the loud signal that the `m_MessageId` offset drifted — suppression still works (name-keyed dispatch is authoritative, the bitmap is self-consistent), but file the drift finding. A `USERMSG VALIDATION: ... lacks readable protobuf reflection` line means that one message carried no protobuf body — expected for some messages, **not** a stop. (There is no longer a global-disable path; the old `USERMSG VALIDATION FAILED` latch was removed in review.)
2. Demo `onPre("CCSUsrMsg_RadioText")` + a bot radio/plant → handler log with name/id/recipients; toggle suppress → the radio line stops appearing client-side; **verify no server-side breakage from SUPERCEDE** (the local-listener question). Breakage → switch to recall-with-modified-mask, shim-internal.
3. Demo `onPre("CMsgTEFireBullets")` + bot gunfire → `item_def_index`, `origin.x/y/z` (dotted read), and `player` logged; decode `player` against the known shooting pawn/controller — settles the D-09 bit-split + pawn-vs-controller questions.
4. A message TTT needs never hitting the hook → the 6-arg-overload deferral was wrong; extend coverage (new decision, not improvisation).
5. `RestartCount=0`, no panic, no protobuf FATAL in logs (the `is_repeated` guards hold).
6. Hot-path sanity: with the demo unsubscribed (plugin removed), no `usermsg` log lines and no hook install.
