# Command reply source — design

**Date:** 2026-07-22
**Status:** approved, ready for planning

## Problem

`ctx.reply()` does not reply in the channel the caller used. It branches on the caller's slot alone
(`core/src/v8host.rs:1809`):

```js
reply: function (m) {
  if (s < 0) { console.log(String(m)); return; }
  var msg = String(m);
  globalThis.__s2pkg_timers.nextFrame().then(function () { globalThis.__s2pkg_chat.Chat.toSlot(s, msg); });
},
```

Every reply from a player therefore lands in chat, including a command the player typed at their own
developer console. A player who types `sm_help` gets ten lines of paginated output spammed into chat
instead of into the console they typed it in.

SourceMod models this correctly: `ReplyToCommand` consults a *reply source* set by the invocation path,
so a console invocation replies to the console and a chat trigger replies to chat. We want that parity.

### Why the information is lost today

Three distinct invocation paths collapse into one `dispatch_concommand(name, slot, args)`, which
carries no record of which path it came from:

| Path | Shim site | Core entry | Today's `reply()` | Correct |
|---|---|---|---|---|
| Server console / rcon | shared ConCommand trampoline (`s2script_mm.cpp:838`) | `s2script_core_dispatch_concommand` | server print | server print |
| Player console | `ISource2GameClients::ClientCommand` hook (`s2script_mm.cpp:4728`) | `s2script_core_dispatch_client_command` | **chat** | **client console** |
| Chat trigger (`!` / `/`) | `Host_Say` detour | `s2script_core_dispatch_chat` | chat | chat |

The `ClientCommand` hook `RETURN_META(MRES_SUPERCEDE)`s a command it handles, so a player-console
command never also reaches the ConCommand trampoline. The paths do not overlap.

## Goals

- `reply()` lands in the channel the caller used, matching SourceMod.
- Plugins can read the source and can force a channel explicitly.
- No change required at any of the ~200 existing `reply`/`replyT` call sites.

## Non-goals

- A mutable `SetCmdReplySource` equivalent. `replySource` is readonly; forcing a channel is done with
  an explicit method at the call site, not by mutating hidden state.
- Stripping control bytes in `Client.print`. That is a separate, deliberately raw API.
- Re-tuning `adminhelp`'s page size now that its output can land in a console.

## Approach

Thread the source as an explicit parameter from the Rust entry point through to the JS invocation
context. Rejected alternatives:

- **An ambient thread-local in Rust**, read by a new native. Leaves wrapper signatures untouched, but
  reintroduces hidden mutable state needing save/restore discipline under re-entrancy — and since
  `__s2cmd_ctx` is the only reader, it buys nothing.
- **Routing inside Rust** via one new `__s2_cmd_reply(slot, source, msg)` native. One routing site, but
  it re-implements `Chat.color` and the one-frame chat deferral in Rust, splitting chat policy across
  two layers.

Threading a parameter matches the file's existing snapshot-and-pass discipline, keeps routing policy in
the JS prelude next to the existing `reply`, and nests correctly for free when a command dispatches
another command.

### Data flow

```
shim (UNCHANGED)                    core/src/ffi.rs              core/src/v8host.rs
─────────────────────────────────────────────────────────────────────────────────────
ConCommand trampoline      ──▶ _dispatch_concommand      ──▶ ReplySource::from_slot(slot)
  (server console / rcon)                                     = slot < 0 ? Server : Console
ClientCommand hook         ──▶ _dispatch_client_command  ──▶ ReplySource::Console
  (player typed sm_help)
Host_Say detour            ──▶ _dispatch_chat            ──▶ ReplySource::Chat
  (player typed !help)
                                                              │
                              dispatch_concommand(name, slot, args, src)
                                                              │  3rd JS arg (i32)
                                                              ▼
                                    wrapper(slot, argString, src) ──▶ __s2cmd_ctx(slot, argString, src)
```

`ReplySource` is `#[repr(i32)] { Server = 0, Console = 1, Chat = 2 }`, crossing to JS as a plain number
and mapped to a string in `__s2cmd_ctx`.

**The shim is not touched.** Each path already enters core through its own C-ABI export, so the source
is derived on the Rust side. No C++ rebuild, no ABI bump.

## API surface

### `CommandInvocation` (`packages/sdk/commands.d.ts`)

```ts
/** Where the command was invoked from — determines where `reply` lands.
 *  `"server"` = server console/rcon (callerSlot -1) · `"console"` = a player's developer
 *  console · `"chat"` = a `!`/`/` chat trigger. */
export type ReplySource = "server" | "console" | "chat";

readonly replySource: ReplySource;

/** Reply to the caller in the channel they used (SM `ReplyToCommand`). */
reply(message: string): void;
/** Force the reply into the caller's chat, whatever `replySource` says (SM `PrintToChat`). */
replyToChat(message: string): void;
/** Force the reply into the caller's developer console (SM `PrintToConsole`). */
replyToConsole(message: string): void;
```

`replyT` keeps its signature and routes through `reply`, so it inherits the fix.

### `Commands` (same file)

```ts
dispatch(name, slot, argString, replySource?: ReplySource): boolean;
//   omitted → slot < 0 ? "server" : "console"   (SM FakeClientCommand parity)
handleChatTrigger(slot, message): { silent, ran } | null;   // always dispatches as "chat"
```

## Behaviour

### Routing table

`reply(msg)` dispatches on `replySource`, with a slot-based degrade on each branch:

| `replySource` | `callerSlot` | Lands in | Timing | C0 stripped |
|---|---|---|---|---|
| `"server"` | `-1` | `console.log` → server console (rcon-redirected) | immediate | yes |
| `"console"` | `>= 0` | `__s2_client_console_print(slot, msg + "\n")` | immediate | yes |
| `"console"` | `-1` | `console.log` *(degrade)* | immediate | yes |
| `"chat"` | `>= 0` | `nextFrame()` → `Chat.toSlot(slot, msg)` | **+1 frame** | no |
| `"chat"` | `-1` | `console.log` *(degrade — the server has no chat)* | immediate | yes |

`replyToChat` / `replyToConsole` enter the corresponding row directly, ignoring `replySource` —
including the slot degrades, so neither can ever be a hard error.

### Chat keeps its one-frame defer

Unchanged from `v8host.rs:1806`. A `!cmd` runs inside the `Host_Say` *pre*-hook, before the player's own
`!slap Bob` line is broadcast, so a synchronous reply would land above the message that caused it. The
defer applies to *every* chat reply — including an explicit `replyToChat` from a console-sourced
command — rather than being conditional on the source. It costs one frame, preserves today's chat
behaviour byte-for-byte, and avoids a second timing rule to reason about.

### Control-character stripping

One internal helper, applied on the console and server paths only:

```js
function __s2cmd_stripCtl(s) { return String(s).replace(/[\x00-\x1F\x7F]/g, ""); }
```

Full C0 range, with **no `\t` / `\n` / `\r` exemption**. `games/cs2/js/pawn.js:437` defines
`Yellow: "\x09"`, `Silver: "\x0A"`, `BlueGrey: "\x0D"` — exempting those three bytes would leak three
chat colours through as stray whitespace. A reply is one line by contract (`adminhelp` already calls
`reply` once per line) and `replyToConsole` appends its own `"\n"`, matching `Client.print`.

Framing this as "a console line carries no control bytes" keeps it engine-generic: it is true on any
Source 2 game, so core acquires no game knowledge.

The chat path stays raw — colour is content the caller owns.

### Fallback when the source is absent

`__s2cmd_ctx(slot, argString, src)` treats a missing or out-of-range `src` as
`slot < 0 ? "server" : "console"`. This covers `Commands.dispatch` called without a source, and means
a stale wrapper can never produce an `undefined` source.

Note this fallback is a *JS-side* guard only. Adding the 4th parameter to the Rust
`dispatch_concommand` is a compile-time break for its callers: the two internal call sites plus the
nine existing test call sites in `v8host.rs` and the one in `ffi.rs` must each be updated to pass a
`ReplySource`. That is mechanical and lands inside the same PR as the signature change.

### Knock-on effects

- `registerServer`'s *"[SM] This command can only be run from the server console"* and `registerAdmin`'s
  *"[SM] You do not have access to this command"* now land in the caller's console when they typed the
  command at their console. SM parity.
- `Chat.color`'s global prefix applies on the chat path only, because it lives inside `Chat.toSlot`.
  Already true incidentally; this design makes it a documented guarantee in `commands.d.ts`.
- A `/cmd` silent trigger still replies to **chat**. `dispatch_chat` passes `Chat` regardless of
  `silent`, which only controls broadcast suppression. SM parity.
- `ctx` remains usable across `await`: it holds only a slot and a source, no pointers. A post-`await`
  reply targets a slot that may have disconnected, which degrades to a no-op at the shim exactly as it
  does today.

## Blast radius

All ~200 `reply` / `replyT` call sites across `plugins/` and `examples/` are unchanged and get correct
routing for free. `Commands.dispatch` and `handleChatTrigger` have no in-repo consumers, and both
changes are additive (an optional trailing parameter). `packages/sdk/commands.d.ts` gains three
members — a **minor** bump, not a major one.

## Testing

### Unit tests (`cargo test -p s2script-core`, in-isolate)

The harness already supports everything needed — `load_body`, `dispatch_concommand`,
`eval_in_context_string`, `LOG` — and both `__s2_client_console_print` and `__s2pkg_chat.Chat.toSlot`
are resolved through `globalThis` at call time, so a test stubs them as spies in pure JS (the pattern at
`v8host.rs:16952`). No new test infrastructure.

| # | Test | Asserts |
|---|---|---|
| 1 | `reply_routes_by_reply_source` | `Server` → `LOG`; `Console` → console spy; `Chat` → chat spy (after a frame drain) |
| 2 | `reply_source_derives_from_entry_point` | each of the three Rust entry points yields the right `ctx.replySource` |
| 3 | `reply_source_falls_back_to_slot` | absent/out-of-range `src` → `slot < 0 ? "server" : "console"` |
| 4 | `console_reply_strips_control_bytes` | via `replyToConsole`/`replyToChat`: `"\x04a\x09b\x0Ac"` → `"abc"` on the console spy, raw on the chat spy |
| 5 | `explicit_reply_targets_override_source` | `replyToChat` from `Console`, `replyToConsole` from `Chat` |
| 6 | `explicit_reply_targets_degrade_at_slot_minus_one` | both fall back to `LOG`, neither throws |
| 7 | `commands_dispatch_reply_source_param` | default by slot; explicit 4th arg honoured; `handleChatTrigger` always `"chat"` |
| 8 | `replyt_inherits_routing` | extends `ctx_replyt_localizes` to the console path |

The existing `command_dispatch_builds_ctx_and_routes_reply` and `chat_triggers_parse_and_dispatch` stay
green unmodified — they dispatch at `slot -1`, which the fallback maps to `"server"`.

### Gate suite — per PR, not once at the top

`cargo test -p s2script-core` · `make check-boundary` ·
`./scripts/check-plugins-typecheck.sh` (mandatory for every `.d.ts` edit) · the `check-*-generated.sh`
set · `test-boundary-nameleak.sh`.

### Live gate (Docker CS2, after the stack is rebased on `main`)

| Input | Expect |
|---|---|
| `sm_help` at the server console | server console |
| `python3 scripts/rcon.py "sm_help"` | returned to the rcon caller |
| Player types `sm_help` in their developer console | **their console**, no stray colour bytes — the bug this slice fixes |
| Player types `!help` in chat | their chat, unchanged |
| Player types `/help` | their chat, originating message suppressed |
| Non-admin types `sm_slap` at their console | *"[SM] You do not have access…"* in **their console**, not chat |

The rcon row is the one behaviour not provable without a running server. It is pre-existing
`console.log` behaviour that this design does not change, but `"server"` is now a named source, so it is
worth confirming during the gate.

## Delivery — a 5-PR stack (`cmd-reply/*`)

Ordered so the single behaviour-changing PR is a small diff a reviewer can actually scrutinise.

| PR | Branch | Change | Behaviour change? |
|---|---|---|---|
| 1 | `cmd-reply/explicit-targets` | `__s2cmd_stripCtl` + `replyToChat`/`replyToConsole` (`replyToChat` extracts today's `reply` chat branch verbatim) + `.d.ts` + tests 4–6 | No — purely additive |
| 2 | `cmd-reply/thread-reply-source` | Rust `ReplySource` enum, `dispatch_concommand` 4th param, 3 entry points, `ctx.replySource`, `.d.ts` + tests 2–3 | No — `reply` still routes as before |
| 3 | `cmd-reply/route-by-source` | `reply` switches on `replySource` + tests 1, 8 | **Yes — this is the fix** |
| 4 | `cmd-reply/dispatch-source-param` | `Commands.dispatch(…, replySource?)`, `handleChatTrigger` forces `"chat"`, `.d.ts` + test 7 | No |
| 5 | `cmd-reply/docs` | `PROGRESS.md` slice entry; document the `Chat.color`-never-on-replies guarantee | No |

PRs 1–4 touch `packages/sdk/commands.d.ts`, so each carries its own changeset (minor; three additive
members).
