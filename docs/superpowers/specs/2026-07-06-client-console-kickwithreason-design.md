# Sub-project 2 — console-print + `Client.kickWithReason` + `Client.ip`

**Goal:** Give `@s2script/clients`' `Client` the last pieces the ban-reason feature needs: `Client.print(msg)` (developer console), `Client.ip`, and `Client.kickWithReason(reason, delaySeconds?)` — the helper that shows a reason (chat + console) then kicks after a beat. Builds directly on sub-project 1's `Client` handle + lifecycle events.

**Non-goals (deferred):**
- **The ban-plugin refactor + flipping the shim `ClientConnect` to always-admit → sub-project 3.** This slice adds the *capability* (`kickWithReason`); wiring it into `@s2script/bans` and removing the instant-reject is the next slice.
- **A `TextMsg`/usermessage console path** — the engine `ClientPrintf` is proven and sufficient; the usermessage path stays a documented fallback only if `ClientPrintf`'s console rendering ever turns out wrong on a real client.

## Background — the feasibility outcome

- **Console-print is `IVEngineServer2::ClientPrintf(CPlayerSlot, const char* szMsg)`** (`eiface.h:238`, SDK comment "Print szMsg to the client console"). Already **live-proven safe in 6.1b** (it was only superseded by SayText2 in 6.1c because that needed the chat *box*). `s_pEngine` is already held; the bot-skip guard (`if (!s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) return;`) is already in production at `s2script_mm.cpp:606`. ~Zero RE.
- **`Client.ip` = `IVEngineServer2::GetPlayerNetInfo(slot)->GetAddress()`** (`inetchannelinfo.h:58`, `const char*`, `"IP:port"` — strip the port). Bots → null netinfo → `""`.
- **`kickWithReason` is pure-JS** over sub-project 1's `onActive` + `Client.chat`/`Client.kick`/`isValid()` + `@s2script/timers` `delay` (Slice-4 async-liveness-guarded — a scheduled callback never fires after teardown).

## Architecture

1. **Two new engine ops (shim + core), ABI-appended after `pawn_commit_suicide`:**
   - `client_console_print(slot, msg)` → the shim guards `GetPlayerNetInfo(slot) != null` then `s_pEngine->ClientPrintf(CPlayerSlot(slot), msg)`. Native `__s2_client_console_print(slot, msg)`.
   - `client_address(slot) -> const char*` → `GetPlayerNetInfo(slot)` null? `""` : `GetAddress()`, copied into a `static std::string s_addressBuf` (mirror `s_steamidBuf`, `s2script_mm.cpp:642`). Native `__s2_client_address(slot) -> string` (copies the C string during the call, mirror `s2_client_name`).
2. **JS on `Client` (the `@s2script/clients` prelude in `v8host.rs`):**
   - `Client.prototype.print(msg)` → `__s2_client_console_print(this.slot, String(msg) + "\n")`.
   - `Client` `ip` getter → `__s2_client_address(this.slot)`, strip a trailing `:port` (`.split(":")[0]`) → bare IP (`""` for bots).
   - `Client.prototype.kickWithReason(reason, delaySeconds?)` — see below.
3. **`.d.ts`** (`packages/clients/index.d.ts`): add `print(message: string): void`, `readonly ip: string`, `kickWithReason(reason: string, delaySeconds?: number): void`.

### `kickWithReason(reason, delaySeconds = 5)`

The helper hides the "deliver only once the client can receive messages, then kick after a beat" dance. **Resolved design:** a per-slot **pending map** consumed by a **single, lazily-wired persistent `onActive`** subscription — so N calls never leak N `onActive` handlers.

```js
// module-level in the @s2script/clients prelude closure:
var __s2_pendingKicks = {};        // slot -> { reason, delay }
var __s2_kickWired = false;
function __s2_wireKickOnActive() {
  if (__s2_kickWired) return; __s2_kickWired = true;
  __s2_client_on("active", function (c) {
    var p = __s2_pendingKicks[c.slot]; if (!p) return;
    delete __s2_pendingKicks[c.slot];
    c.chat(p.reason); c.print(p.reason);                       // deliver: chat (SayText2) + console
    globalThis.__s2pkg_timers.delay(p.delay * 1000).then(function () {
      var cc = __s2_clients.fromSlot(c.slot);                  // re-resolve; null if they left -> no kick
      if (cc) cc.kick(p.reason);
    });
  });
}
Client.prototype.kickWithReason = function (reason, delaySeconds) {
  __s2_wireKickOnActive();
  __s2_pendingKicks[this.slot] = { reason: String(reason), delay: (delaySeconds == null ? 5 : delaySeconds) };
};
```

- **Contract: call it from `onConnect`** (the ban path — sub-project 3 checks the ban at connect, before the client is active). The pending entry is set at connect; the persistent `onActive` fires when the client reaches in-game and delivers + schedules the kick. This is exactly the SourceBans admit→message→kick shape.
- **Timer:** `globalThis.__s2pkg_timers.delay(ms)` (referenced at call-time, so prelude ordering is irrelevant) — Slice-2 tick-integrated, Slice-4 async-liveness-guarded (never fires after teardown).
- **Disconnect edge:** the scheduled kick re-resolves `Clients.fromSlot(slot)` at fire time → a client who already left reads `null` → no kick. No explicit `onDisconnect` cleanup needed for correctness.
- **"Already active" caveat (documented, acceptable for v1):** if `kickWithReason` is called for a client who is *already* in-game (not the ban path), the deliver waits for the *next* `onActive`, which may not come. v1 targets the `onConnect` ban path; an immediate-deliver-when-already-active path is a deferred enhancement, not needed by sub-project 3.

## Testing

**In-isolate (cargo):** the two natives degrade cleanly with no engine (`__s2_client_console_print` no-ops; `__s2_client_address` → `""`); the prelude exposes `Client.print`/`ip`/`kickWithReason`; `ip` strips `:port` (`"1.2.3.4:27005"` → `"1.2.3.4"`, `""` → `""`). The 2 test op-structs gain the two new `None` fields (ABI-order).

**Boundary:** both ops take `slot`/string — engine-generic; `IVEngineServer2` is shim-only. Both gates green.

**Live gate (de_dust2):** the SEND paths + non-crash are bots-provable — `Client.print` to a bot is skipped (netinfo null, no crash); `Client.all()[0].ip` reads `""` for a bot without crashing. The VISUAL confirmations (a human sees the console line; a real client's `.ip` is their address; `kickWithReason` shows chat+console then kicks after the delay) carry the standing bots-gate caveat → a **deferred live test** (same as sub-project 1's fresh-connect), run with a human client. `RestartCount=0`.

## Risks / decisions

- **Low-risk overall:** both ops reuse held interfaces + proven patterns (ClientPrintf live-safe 6.1b; the static-string + copy pattern from `client_steamid`/`name`). No reflection, no new interface, no sigscan.
- **ABI discipline:** append the two ops AFTER `pawn_commit_suicide` in BOTH the C header struct and the Rust mirror (same order), and add the two `None` fields to the test op-structs — the ABI is positional.
- **The `\n` on console-print** — `ClientPrintf` prints verbatim; append `"\n"` (in JS) for a clean line.
- **`kickWithReason` deliver-timing** is resolved above (pending map + one lazily-wired persistent `onActive`; `onConnect`-oriented contract). The "already-active" caveat is a documented, deferred enhancement — not needed by sub-project 3.

## Build order (for the plan)

- **Task 1 — the two engine ops** (`client_console_print` + `client_address`): C header + Rust mirror + natives + shim impl + the 2 test-struct None fields + cargo tests + boundary gate + sniper build.
- **Task 2 — the JS** (`Client.print`/`ip`/`kickWithReason` in the prelude + `.d.ts` + extend `clients-demo`) + typecheck/build.
- Then: deploy + live gate (bots: send-path + non-crash; human: deferred visual) + merge.
