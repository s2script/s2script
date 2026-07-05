# Slice 6.13-A — basechat foundation: per-recipient colored chat + show-activity + sender-aware sm_say

**Status:** design (brainstormed 2026-07-05)

**Goal:** Bring SourceMod `basechat`'s sender-aware `sm_say` to s2script — an admin's say is shown to each recipient with a name customized by the `sm_show_activity` rules (normal players see `ADMIN`, admins see the real name), delivered as **per-recipient colored chat**. Plus the `@` all-chat trigger for it. This is the foundation the rest of basechat (`psay`, admin-chat, `@@`) sits on; those are a follow-up (Slice 6.13-B).

**North star:** the SM default plugins as `@s2script/base*` are the std-lib acceptance test. `basechat` is one of them.

---

## Why this is one slice (and what's deferred)

Every basechat feature needs **per-recipient messaging** (different text/name to different players). That per-recipient *colored* send is the one genuinely new, risky piece. So Slice A builds the foundation + proves it on `sm_say`; Slice B (`@@`→psay, `@`-in-team→admin-chat, remaining commands) is then trivial layering and gets its own spec.

---

## Architecture — three units

### 1. Per-recipient colored delivery (the foundation)

Today: `Chat.toAll` → the game's `UTIL_ClientPrintAll` (broadcast, true color, no controller, no crash). `Chat.toSlot` still uses raw SayText2 → **team-colored** (entityindex=player forces player-chat rendering).

Change: `Chat.toSlot(slot, msg)` sends via the game's **`ClientPrint(CBasePlayerController*, HudDestination, const char* msg, p1..p4)`** — the per-player function CSSharp uses for `PrintToChat` (confirmed: it's the only per-player print in the CSSharp gamedata; they pair it with their own `CSingleRecipientFilter`). Renders true custom color to one player, consistent with the green broadcast.

- **First implementation task is an RE spike to fix the earlier crash**, done OFFLINE (disassembly + reasoning), NOT live guessing: confirm `ClientPrint`'s prologue/args (already disassembled: `rdi`=controller, `esi`=HudDestination, `rdx`=msg, `rcx/r8/r9`=params), and determine why passing `s2_ent_by_index(slot+1)` faulted — most likely a wrong/mid-state controller pointer. Validate the controller before the call.
- **Fallback (explicit):** if `ClientPrint` cannot be made safe within the spike's budget, `Chat.toSlot` keeps the existing team-colored SayText2 path. `sm_say` still works; only the per-recipient color is degraded. Never ship a crash.
- Engine-generic: the shim resolves `ClientPrint` from gamedata (validated-unique per the RE doctrine); `HudDestination::Chat = 3` (from CSSharp's enum). No new core ABI op — reuses the existing `client_print` op (slot ≥ 0 → ClientPrint; slot < 0 → UTIL_ClientPrintAll, unchanged).

### 2. Show-activity system (a `FormatActivitySource` port)

A pure, engine-generic helper — no chat, no engine calls beyond admin lookup — so it is fully unit-testable.

`Activity.formatSource(actorSlot, recipientSlot) → { show: boolean, name: string }`
- `actorSlot < 0` ⇒ actor is the server console; name defaults to `"Console"`.
- Reads the `sm_show_activity` cvar (int bitmask). Flags (SM values): `1` kActivityNonAdmins, `2` kActivityNonAdminsNames, `4` kActivityAdmins, `8` kActivityAdminsNames, `16` kActivityRootNames. **Default `13` (`1|4|8`).**
- `name` = the actor's real name (`mode 0`) or a generic (`mode 1`): `"ADMIN"`, or `"PLAYER"` if the actor lacks `ADMFLAG.GENERIC`, or `"Console"`.
- Decision (verbatim port of SM's `FormatActivitySource`):
  - **Recipient is NOT an admin** (no `ADMFLAG.GENERIC`): `show` iff flag `1` or `2` set; `name` = real (`mode 0`) iff flag `2` set OR `recipient == actor`; else generic.
  - **Recipient IS an admin:** `is_root` = recipient has `ADMFLAG.ROOT`. `show` iff flag `4` or `8`, or (`16` and `is_root`); `name` = real iff flag `8`, or (`16` and `is_root`), or `recipient == actor`; else generic.
- Home: `@s2script/cs2` (needs the CS2 admin/name accessors) exports `Activity`; the `sm_show_activity` cvar is registered by the base plugin. (Revisit if a cleaner engine-generic split emerges during planning.)

### 3. Sender-aware `sm_say` + the `@` all-chat trigger

- `sm_say <message>` (ADMFLAG.CHAT): for each connected player `p`, `{show, name} = Activity.formatSource(callerSlot, p.slot)`; if `show`, `Chat.toSlot(p.slot, "(ALL) " + name + ": " + <colored message>)`. Per-recipient — the name differs by recipient.
- The colored message uses the inline-color model already shipped (e.g. a green body); `(ALL) name:` is the SM chat prefix.
- `@` all-chat trigger: a player typing `@message` in **all** chat runs `sm_say message` as themselves. Detected in the existing `Host_Say` detour (which already provides `teamonly`), mirroring the `!`/`/` trigger handling. Gated by ADMFLAG.CHAT like the command. (`@@` and `@`-in-team are Slice B.)

---

## Data flow

`@msg` in chat → Host_Say detour (teamonly=false) → dispatch `sm_say` as the speaker → for each connected player → `Activity.formatSource` → per-recipient line → `Chat.toSlot` → `ClientPrint(controller, Chat, line)`.

## Error handling / degrade

- `ClientPrint` unresolved or the spike inconclusive → `Chat.toSlot` falls back to team-colored SayText2 (no crash).
- `sm_show_activity` absent/garbage → default `13`.
- A recipient with no netchannel (bot) → skipped (existing guard).
- Non-admin using the command/trigger → denied by the existing `registerAdmin` gate.

## Testing

- **Unit (core/node:test):** `Activity.formatSource` across the flag matrix × {actor console/admin/non-admin} × {recipient admin/root/non-admin/self}. This is the correctness core and needs no engine.
- **Live gate (de_dust2, human client):** admin runs `sm_say hi` → the admin client shows `(ALL) gkh: hi`, a non-admin client shows `(ALL) ADMIN: hi`, message colored; `@hi` in chat does the same; `RestartCount=0`. (Two clients ideal; a single admin client + rcon-forced non-admin covers most.)

## Out of scope (Slice B / later)

`@@`→`sm_psay` (private), `@`-in-team→admin-only chat, `sm_msay`/`sm_csay`/`sm_tsay` (menu/center/top say), applying show-activity to other admin commands (kick/slap announcements), per-recipient color if the spike falls back to team-colored.
