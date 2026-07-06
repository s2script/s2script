# funcommands (v1: gravity / blind / noclip / freeze) — design + plan

**Goal:** Ship `@s2script/funcommands` with `sm_gravity`, `sm_blind`, `sm_noclip`, `sm_freeze` (SM `funcommands` fun effects). Defer `sm_burn`/`sm_beacon` (real from-scratch RE — no framework sig to port; documented TODOs).

**Feasibility outcome:** gravity + blind are pure JS on already-generated writable setters. noclip + freeze need one tiny, broadly-useful engine add — the entity *write* native handles only I32/F32/BOOL (reads have all 11 kinds); adding the narrow-int write kinds unlocks `m_MoveType` (uint8) writes.

## Architecture

**Task 1 — narrow-int entity write kinds (core, ~15 additive lines + 1 sniper).** Mirror the EXISTING read arms. In `core/src/entity.rs` add `write_i8`/`write_i16`/`write_u8`/`write_u16`/`write_u32` (mirror `write_i32`). In `core/src/v8host.rs` `s2_ent_ref_write`, add the `KIND_I8`/`KIND_I16`/`KIND_U8`/`KIND_U16`/`KIND_U32` arms (mirror the read arms `v8host.rs:1397-1401` + the `KIND_I32` write arm). Add the JS wrappers `writeInt8`/`writeInt16`/`writeUInt8`/`writeUInt16`/`writeUInt32` on `EntityRef` (`v8host.rs:~512`, beside `writeInt32`; the `K` map already has the constants). Cargo tests: a `writeUInt8`/`writeInt16` round-trips through `readUInt8`/`readInt16` on a fake buffer (mirror the existing write round-trip tests). Engine-generic; boundary green. (64-bit writes stay deferred — they need bigint handling; only the narrow ints here.)

**Task 2 — pawn move-type access (CS2 game layer: `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`).** `m_MoveType`/`m_nActualMoveType` are `enum MoveType_t` (uint8) — the codegen skips enums, so hand-write the accessor (like the 5C.4 hand-written `pawn.origin`), offsets LIVE-resolved via the schema-offset native (grep how the generated code / the hand-written nav resolves an offset — e.g. `__s2_schema_offset("CBaseEntity", "m_MoveType")`).
- `pawn.moveType` (getter) → `this.ref.readUInt8(offset("CBaseEntity","m_MoveType"))` (`0`/null-safe).
- `pawn.moveType = v` (setter) → writes BOTH `m_MoveType` AND `m_nActualMoveType` via `writeUInt8` + `this.notifyStateChanged()` (CS2 noclip/freeze need both fields set; document that the setter writes both). Offsets resolved live per access (self-healing, per the nav discipline).
- `.d.ts`: `moveType: number` on the `Pawn`/`CCSPlayerPawn` interface. Note the `MoveType_t` values are the plugin's concern (const.h: NONE=0, WALK=2, NOCLIP=7).

**Task 3 — `@s2script/funcommands` plugin (`plugins/funcommands/`).** `registerAdmin(ADMFLAG.SLAY)` (SM funcommands' flag), `Player.target` resolution, per-target apply. Imports `@s2script/commands`, `@s2script/admin`, `@s2script/cs2` (Player), `@s2script/timers` (delay, for freeze auto-restore).
- **`sm_gravity <target> [factor]`** — `factor` float (default `1.0`; `0` = reset). Per target: `const pw = p.pawn; if (pw) { pw.gravityScale = factor; pw.actualGravityScale = factor; }`. Reply.
- **`sm_blind <target> [amount]`** — `amount` int (default `255`; `0` = un-blind). Per target: `if (pw) { pw.flashDuration = amount <= 0 ? 0 : (amount / 51); pw.flashMaxAlpha = amount <= 0 ? 0 : 255; }` (the CS2 flash white-out; `flashDuration` is seconds — map an SM-style 0-255 "amount" to a duration, or take `amount` as seconds directly — pick seconds for clarity: `amount` = seconds of blindness, default 5, `pw.flashDuration = amount; pw.flashMaxAlpha = 255`). Reply.
- **`sm_noclip <target>`** — toggle: per target, `if (pw) pw.moveType = (pw.moveType === 7 ? 2 : 7)` (NOCLIP=7 ↔ WALK=2). Reply.
- **`sm_freeze <target> [time]`** — per target, `if (pw) { pw.moveType = 0; }` (MOVETYPE_NONE); if `time > 0`, `delay(time*1000).then(() => { const q = Player.fromSlot(p.slot); if (q && q.pawn) q.pawn.moveType = 2; })` (auto-restore to WALK; re-resolve the player at fire time — slot may be stale). Reply. (No time → stays frozen until `sm_unfreeze`/manual; add `sm_unfreeze <target>` → `moveType = 2`.)

Constants (MoveType) live in the plugin (CS2-specific): `NOCLIP=7`, `WALK=2`, `NONE=0`.

## Testing / live gate

- **In-isolate (cargo):** the new write kinds round-trip (writeUInt8→readUInt8, writeInt16→readInt16) on a fake buffer.
- **Boundary:** the write kinds are engine-generic (entity.rs/v8host.rs); pawn.moveType + funcommands are CS2. Both gates green.
- **Live (bots-provable + immediately verifiable):** on a bot — `sm_gravity Rex 0.2` → the bot floats (low gravity); `sm_gravity Rex 1` resets; `sm_noclip Rex` → the bot noclips (moveType 7; verify via a read or the visible effect on a human); `sm_freeze Rex` → the bot stops; `sm_blind Rex` → (human-visible only — bots don't render). Most are verifiable on a human client; the field WRITES (moveType/gravity) are confirmable via a read-back or the visible effect. `RestartCount=0`, no crash.
- **Deferred human check:** the visible effects (float / white-out / noclip-through-walls) on a real client.

## Risks / decisions

- **The U8 write add is low-risk** (mirrors the proven read arms + writeInt32; additive; a fake-buffer test covers it). One sniper.
- **`pawn.moveType` setter writes BOTH `m_MoveType` and `m_nActualMoveType`** — CS2 uses the Type/ActualType pair; setting only one may not take. Documented.
- **freeze auto-restore** re-resolves the player at the timer fire (slot may be reused) — a stale slot reads a null/different pawn → guarded.
- **gravity single-field sufficiency** — set both `gravityScale` + `actualGravityScale` (both pure JS); live-verify which sticks (ModSharp uses a SetGravityScale fn, but the field write is SM-cstrike parity — try the field first).
- **Deferred:** `sm_burn` (ignite game-fn, no framework sig → from-scratch RE), `sm_beacon` (tempent/particle subsystem), the SM `CUserMessageFade` black-fade blind (the flash white-out suffices for v1).

## Build order
- Task 1 — narrow-int write kinds (core) + cargo tests + boundary + sniper.
- Task 2 — `pawn.moveType` get/set (pawn.js) + `.d.ts`.
- Task 3 — `@s2script/funcommands` (gravity/blind/noclip/freeze/unfreeze) + typecheck/build.
- Then: deploy + live gate + merge.
