# s2script

TypeScript plugin framework for Source 2 engine games. The core owns every engine touchpoint; plugins compose through one `HookResult` contract.

## Language

**Command**:
A named ConCommand handler a plugin registers. One handler, three invocation channels: server console, client console, and a `!`/`/` chat trigger.
_Avoid_: console command (too narrow — chat triggers the same handler), invocation

**Reply source**:
Which channel invoked the command — server, console, or chat. Decides where `ctx.reply` lands.
_Avoid_: caller, origin

**Say**:
A player chat line that did not match a command trigger. Subscribers (`ctx.clients.onSay`) may consume it (`Handled` stops later subscribers). Not a separate module from Command — the same inbound path.
_Avoid_: Chat.onMessage (deleted public name), chat message (ambiguous with outbound print)

**Client**:
A connected engine slot. Lifecycle and print live here. `onSay` is authored on the client context because a player said something; the implementation is Command's unmatched-say fan-out.
_Avoid_: Player (CS2 pawn/controller — game package), user
