# Client command execution — design

**Status:** Approved — ready for planning.
**Audience:** plugin authors needing to drive a client-side action; core+shim maintainers.
**Builds on:** the already-acquired `s_pEngine` (`IVEngineServer2`) and `m_gameClients`
(`ISource2GameClients`) interface pointers, and the `s2_server_command` op pattern
(`shim/src/s2script_mm.cpp:1497`).

---

## 1. Goal

Make a client run a console command — SourceMod's `ClientCommand` / `FakeClientCommand`, ModSharp's
`IGameClient.Command` / `FakeCommand`. It is the last simple, universally-useful gap in the client
category: s2script can already run commands on the *server* (`Server.command`) but has no way to
originate one on behalf of a *client*.

## 2. Why this is a core op, not a plugin-declared call

This capability was originally slated to arrive via non-entity receivers for plugin-declared engine
calls. **That framing was wrong**, and it is worth recording why, because the same trap will recur.

Routed through the generic declared-call machinery, both engine entry points are unreachable:

| Route | Signature | Blocked by |
|---|---|---|
| `IVEngineServer2::ClientCommand` | `(CPlayerSlot, const char* szFmt, ...)` | variadic — out of scope in the gamedata spec |
| `ISource2GameClients::ClientCommand` | `(CPlayerSlot, const CCommand&)` | struct by reference — also out of scope |

Both restrictions exist because the *generic* path marshals a closed vocabulary across an ABI. A
purpose-built op has neither problem: the shim writes an ordinary C++ call, so the compiler emits the
variadic sequence correctly (including the `al` vector-register count), and the shim constructs the
`CCommand` itself rather than marshalling a struct from JS.

This is the charter reasserting itself — *the core owns every engine touchpoint*. A well-defined,
universally-useful capability belongs in core, not behind an `unsafe` escape hatch. Non-entity
receivers are consequently **dropped** from the parity roadmap: with this capability handled properly,
an interface allowlist would only buy `ClientPrintf` and cvar getters, and the Steam/sound interfaces
it was also meant to unlock are not acquired by the shim at all.

## 3. Two commands, not one

They are different mechanisms and both have SourceMod parity, so both ship:

| API | Engine path | Meaning |
|---|---|---|
| `client.command(cmd)` | `IVEngineServer2::ClientCommand(slot, "%s", cmd)` | Tell the CLIENT to execute it. Requires a real, cooperating client — a bot has no console, so this is a no-op on bots. |
| `client.fakeCommand(cmd)` | `ISource2GameClients::ClientCommand(slot, CCommand)` | The SERVER processes the command as if the client had sent it. Works on bots, and is what SourceMod's `FakeClientCommand` does. |

`fakeCommand` is the one most plugins want (it is how you make a player "say" something, trigger a
`sm_` command on their behalf, or drive a menu selection), and it is the one that can be gated with
bots.

## 4. The format-string hazard

`IVEngineServer2::ClientCommand` is declared `FMTFUNCTION(3, 4)` — its second parameter is a **format
string**. Passing plugin text directly means a `%` in player-supplied input is read as a conversion
specifier: at best garbage, at worst a crash or an information leak.

The op therefore *always* calls `ClientCommand(slot, "%s", cmd)`. The text can never be the format
string. This is exactly why the capability belongs in core: the hazard is neutralised once, in a
reviewed place, instead of being handed to every plugin author who declares a call.

## 5. Ops

Two ops, **appended** at the end of `S2EngineOps` (the struct is ABI-ordered; append only, never
insert), populated at the matching end of the shim initializer:

```rust
pub type ClientCommandFn     = extern "C" fn(slot: c_int, cmd: *const c_char) -> c_int;
pub type ClientFakeCommandFn = extern "C" fn(slot: c_int, cmd: *const c_char) -> c_int;
```

Both return `1` on dispatch and `0` when degraded: the interface pointer is null (it was never
acquired), the slot is outside `[0, 64)`, the command is null/empty, or — for `fakeCommand` —
`CCommand::Tokenize` rejected the string. A caller can therefore distinguish "sent" from "not sent",
rather than getting a silent no-op.

## 6. Shim implementation

Mirrors `s2_server_command` (three lines plus guards):

```cpp
static int s2_client_command(int slot, const char* cmd) {
    if (!s_pEngine || !cmd || !cmd[0]) return 0;
    if (slot < 0 || slot >= kMaxClientSlots) return 0;
    s_pEngine->ClientCommand(CPlayerSlot(slot), "%s", cmd);   // "%s" is MANDATORY — see §4
    return 1;
}

static int s2_client_fake_command(int slot, const char* cmd) {
    // m_gameClients is a MEMBER of S2ScriptPlugin (s2script_mm.h:124), not a file-static like
    // s_pEngine — a file-scope op must reach it through the global instance, the same way the
    // frame-hook code uses g_S2ScriptPlugin.m_server.
    ISource2GameClients* gc = g_S2ScriptPlugin.m_gameClients;
    if (!gc || !cmd || !cmd[0]) return 0;
    if (slot < 0 || slot >= kMaxClientSlots) return 0;
    CCommand parsed;
    if (!parsed.Tokenize(cmd)) return 0;                       // refuse rather than dispatch garbage
    gc->ClientCommand(CPlayerSlot(slot), parsed);
    return 1;
}
```

`CPlayerSlot(int)` is a real constructor (`playerslot.h:13`). `Tokenize` takes a `CUtlString`, which a
`const char*` converts to implicitly.

**Re-entrancy note.** The shim already installs a PRE `SourceHook` on
`ISource2GameClients::ClientCommand` (Slice 6.11c — it is how player console commands reach JS). A
`fakeCommand` call will therefore pass through our own hook and dispatch to JS handlers, exactly as a
real client command would. That is the correct and desirable behaviour — it is what makes the
capability useful — but it means a plugin can drive its own command handler, so the existing
`try_borrow_mut` re-entrancy guard in the dispatch path is load-bearing here. No new guard is added;
the slice's testing asserts the existing one holds.

## 7. Plugin API

Added to the existing `Client` class in `@s2script/sdk/clients` — no new subpath, so no
`check-examples-coverage` consumer is required:

```ts
/**
 * Tell this client to run `cmd` in their own console, as if they had typed it.
 * Requires a real client — a bot has no console, so this is a no-op on bots. Use
 * {@link Client.fakeCommand} for server-side execution. False when not dispatched.
 */
command(cmd: string): boolean;

/**
 * Have the SERVER process `cmd` as if this client had sent it (SourceMod `FakeClientCommand`).
 * Works on bots. This dispatches through the same path a real client command takes, so it WILL
 * reach command handlers — including your own. False when not dispatched.
 */
fakeCommand(cmd: string): boolean;
```

## 8. Testing

- **Core (`cargo test -p s2script-core`):** both natives round-trip through recording fake ops
  (slot + string reach the op verbatim); slot out of range returns false; empty string returns false;
  ops-absent returns false.
- **Shim:** compiles and links.
- **Live gate (Docker CS2, bots):** `fakeCommand(botSlot, "sm_help")` on a bot must produce the
  command's output in the server log — end-to-end proof that the server dispatched it. This is
  gateable with bots precisely because `fakeCommand` is server-side; `command()` is **not** bot-
  gateable (no console) and is deferred to a human session, stated rather than glossed.

## 9. Out of scope

- ModSharp's `ExecuteStringCommand` (a third variant with different semantics) — YAGNI until asked.
- `ClientCommandKeyValues`.
- Any rate limiting or command filtering: `fakeCommand` deliberately reaches the same handlers a real
  client command does, and a plugin that wants to restrict what it sends can check before calling.

## 10. Success criteria

1. `client.fakeCommand("sm_help")` on a bot dispatches server-side and the command's output appears.
2. `command()` and `fakeCommand()` return `false` — never silently no-op — for a bad slot, an empty
   string, or an unacquired interface.
3. Plugin text is never used as a format string.
4. `make ci` green, including `check-boundary`.
