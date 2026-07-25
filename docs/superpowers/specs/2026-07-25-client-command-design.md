# Client command execution — design

**Status:** Implemented and live-gated (2026-07-25). BOTH `command` and `fakeCommand` ship.
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
| `ICvar::DispatchConCommand` | `(ConCommandRef, const CCommandContext&, const CCommand&)` | two structs by reference — also out of scope |

Both restrictions exist because the *generic* path marshals a closed vocabulary across an ABI. A
purpose-built op has neither problem: the shim writes an ordinary C++ call, so the compiler emits the
variadic sequence correctly (including the `al` vector-register count), and the shim constructs the
`CCommand` itself rather than marshalling a struct from JS.

This is the charter reasserting itself — *the core owns every engine touchpoint*. A well-defined,
universally-useful capability belongs in core, not behind an `unsafe` escape hatch. Non-entity
receivers are consequently **dropped** from the parity roadmap: with this capability handled properly,
an interface allowlist would only buy `ClientPrintf` and cvar getters, and the Steam/sound interfaces
it was also meant to unlock are not acquired by the shim at all.

## 3. Two commands, and the CCommand detour

| API | Engine path | Meaning |
|---|---|---|
| `client.command(cmd)` | `IVEngineServer2::ClientCommand(slot, "%s", cmd)` | Tell the CLIENT to execute it. Needs a real client — a bot has no console. |
| `client.fakeCommand(cmd)` | `ICvar::DispatchConCommand(ref, ctx{slot}, args)` | The SERVER executes it attributed to that player. Works on bots. |

`fakeCommand` needs a `CCommand`, and **no shipped CS2 binary exports one's constructor or
`Tokenize`** — using them links fine (a shared library tolerates undefined symbols) and then kills
the entire addon at `dlopen`. That is how it was found: by taking a server down.

**That is not, however, a reverse-engineering problem, and an earlier revision of this spec was
wrong to call it one.** `CCommand`'s data members are `CUtlVectorFixedGrowable` templates declared
in `tier1/convar.h`, so the layout is already ours — the header IS the definition. Only the
out-of-line method bodies are missing, and hl2sdk vendors them at `tier1/convar.cpp`. Compiling that
wholesale drags in the ConVar system plus the `CUtlString`/`UtlVectorMemory`/tier0 cascade that
`tier1_shims.cpp` exists to avoid, so `shim/src/ccommand_shim.cpp` defines the four members we
need — the same doctrine that file already applies to `MurmurHash2LowerCase`. Defining them as
member functions gives private access and emits exactly the mangled symbols the linker wants.

The tokenizer is ours (Valve's routes through `CUtlBuffer::ParseToken` and
`g_pCVar->GetCharacterSet()`, both also unexported). It lives in `ccommand_tokenize.h/.cpp` free of
SDK types so it unit-tests standalone with no stubs.

## 4. The format-string hazard

`IVEngineServer2::ClientCommand` is declared `FMTFUNCTION(3, 4)` — its second parameter is a **format
string**. Passing plugin text directly means a `%` in player-supplied input is read as a conversion
specifier: at best garbage, at worst a crash or an information leak.

The op therefore *always* calls `ClientCommand(slot, "%s", cmd)`. The text can never be the format
string. This is exactly why the capability belongs in core: the hazard is neutralised once, in a
reviewed place, instead of being handed to every plugin author who declares a call.

## 5. Ops

One op, **appended** at the end of `S2EngineOps` (the struct is ABI-ordered; append only, never
insert), populated at the matching end of the shim initializer:

```rust
pub type ClientCommandFn = extern "C" fn(slot: c_int, cmd: *const c_char) -> c_int;
```

Returns `1` on dispatch and `0` when degraded: the engine interface is null (never acquired), the slot
is outside `[0, 64)`, or the command is empty. A caller can therefore distinguish "sent" from "not
sent" rather than getting a silent no-op.

## 6. Shim implementation

Mirrors `s2_server_command` (three lines plus guards):

```cpp
static int s2_client_command(int slot, const char* cmd) {
    if (!s_pEngine || !cmd || !cmd[0]) return 0;
    if (slot < 0 || slot >= kMaxClientSlots) return 0;
    s_pEngine->ClientCommand(CPlayerSlot(slot), "%s", cmd);   // "%s" is MANDATORY — see §4
    return 1;
}
```

`CPlayerSlot(int)` is a real constructor (`playerslot.h:13`) and IS exported — unlike `CCommand`'s.

**Re-entrancy is not a concern for `command()`.** It hands the command to the client; nothing
re-enters our dispatch path. That analysis WOULD matter for `fakeCommand`, which routes through the
same `ISource2GameClients::ClientCommand` our PRE hook watches — noted here for whoever picks up the
deferred spike.

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
```

## 8. Testing

- **Core (`cargo test -p s2script-core`):** both natives round-trip through recording fake ops
  (slot + string reach the op verbatim); slot out of range returns false; empty string returns false;
  ops-absent returns false.
- **Shim:** compiles and links.
- **`scripts/check-shim-symbols.sh`:** every mangled symbol the built shim needs must be exported by
  some shipped game binary. Verified both ways — it passes on the shipped shim, and reintroducing the
  `CCommand::Tokenize` call makes it FAIL naming that symbol. Wired into `ci-native.sh`.
- **Live gate: NOT bot-provable.** `command()` asks a *client* to execute, and a bot has no console,
  so there is no observable effect to assert with bots. What a deploy does prove — and what one
  already did prove the hard way — is that the addon loads. Effect confirmation needs a human client
  and is deferred, stated rather than glossed.

## 9. Out of scope

- ModSharp's `ExecuteStringCommand` (a third variant with different semantics) — YAGNI until asked.
- `ClientCommandKeyValues`.
- Any rate limiting or command filtering: `fakeCommand` deliberately reaches the same handlers a real
  client command does, and a plugin that wants to restrict what it sends can check before calling.

## 10. Success criteria

1. `client.command(cmd)` reaches the engine with the slot and text verbatim, including a literal `%`.
2. It returns `false` — never silently no-ops — for a bad slot, an empty string, or an unacquired
   interface.
3. Plugin text is never used as a format string.
4. `check-shim-symbols.sh` passes, and fails when the `CCommand::Tokenize` call is reintroduced.
5. `make ci` green, including `check-boundary`.

---

## 11. Live-gate result (2026-07-25)

Docker CS2, `de_inferno`, 12 bots. Sniper shim `1b5af372` / core `35f3b979` (GLIBC 2.17 / 2.30).

**PASS — the op dispatches end to end.** `sm_clientcmd 0 say hello` returned `true`, exercising the
whole chain: JS → `__s2_client_command` → op → `IVEngineServer2::ClientCommand`. All four degrade
paths returned the usage error rather than dispatching.

**The format-string hazard is neutralised in the SHIPPED binary**, proven by disassembling the
deployed `.so` rather than trusting the source:

```
lea 0x13cf01(%rip),%rdx   # 0x1cb091 -> "%s"      <- the FORMAT is the constant
mov %rsi,%rcx             #                        <- the plugin text is a VARARG
mov %edi,%esi             #                        <- slot
xor %eax,%eax             #                        <- al = 0, no vector regs (correct for varargs)
cmp $0x3f,%edi / ja       #                        <- UNSIGNED, so negatives are rejected too
```

**ABI append verified live.** `client_command` is the 106th and last op. `sm_voice 0 1` / `0 0`
round-tripped, so the adjacent tail-region ops did not shift.

**Effect confirmation deferred, stated not glossed.** `command()` asks a *client* to execute, and a
bot has no console, so there is no observable effect to assert with bots — the cookbook recipe says
so in its own reply. This is the same Tier-2 deferral the voice slices carry.

**What the gate really proved.** The first attempt at this slice took the server down at 15:54 with
the `CCommand::Tokenize` dlopen failure. The 16:11 boot with `check-shim-symbols.sh` green loaded
cleanly — 3 plugins, no faults. Server restored to its pre-gate baseline (`de0eb747`) afterwards.

---

## 12. FakeClientCommand: how it actually dispatches (2026-07-25)

**The wrong entry point cost the most time here.** `ISource2GameClients::ClientCommand(slot,
CCommand&)` looks like the dispatch API and is what SourceMod's CS:GO-era `FakeClientCommand` maps
onto. It is not. In CS2 it is the **inbound** callback the engine invokes to tell the game module a
client sent a command — which is exactly why this shim *hooks* it to observe player commands.
Calling it from the game module notifies the game and executes nothing: measured on hardware,
`say`, `kill` and a plugin command all returned cleanly with no effect.

The real path is the engine's own command dispatch:

```cpp
CCommand parsed;
if (!parsed.Tokenize(cmd)) return 0;
ConCommandRef ref = s_pCvar->FindConCommand(parsed.Arg(0), /*allow_defensive=*/true);
if (!ref.IsValidRef()) return 0;                        // a ConVar or unknown name — refuse
CCommandContext ctx(CT_NO_TARGET, CPlayerSlot(slot));   // <- the "as if this client sent it" part
s_pCvar->DispatchConCommand(ref, ctx, parsed);
```

Both are plain vtable calls on the already-acquired `ICvar`, so no new symbols.

### Live gate — PASS

```
sm_fakecmd 0 say from_slot_zero   -> [All Chat][Enforcer (0)]: from_slot_zero
sm_fakecmd 3 say from_slot_three  -> [All Chat][Aspirant (0)]: from_slot_three
say from_console                  -> [All Chat][Console (0)]:  from_console
```

Attribution is the proof: the faked command prints as **that bot**, not as Console, and slot 3
prints as a different bot than slot 0. Quoting survives end to end —
`sm_fakecmd 1 say "quoted   text  here"` arrives as one argument. Degrades correctly: an unknown
name and a ConVar (`mp_friendlyfire`) both return `false` rather than pretending.

### Known limitation, stated rather than glossed

Engine commands execute; **commands registered by s2script plugins do not**. Faking `sm_console`
returns `true` (the ref resolves, the dispatch is made) but the handler never runs, while invoking
it directly does. The engine refuses a client-context dispatch for commands lacking the
client-executable flag, and our `RegisterConCommand` path does not set one. Fixing that is a
separate change to command registration, not to this op.

### A methodology note worth keeping

Three probes in a row said "no dispatch" and all three were invalid: `docker logs` was not capturing
the container's current output at all, so *the control did not log either* — a fact I only checked
after drawing conclusions from it, and after writing one of them into a commit message. The rcon
**response** carries the server console output for the command being run, and comparing a direct
invocation against a faked one in that channel settled it immediately. Verify the probe on a known
positive before trusting a negative.
