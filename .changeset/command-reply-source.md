---
"@s2script/sdk": minor
---

**Behaviour change:** `CommandInvocation.reply` now answers in the channel the caller used, matching
SourceMod's `ReplyToCommand`. A player who types `sm_help` at their developer console gets the reply
in that console instead of having it spammed into chat; `!help` still answers in chat, and the server
console/rcon still answers on the server console (now with control bytes stripped). Control bytes are
stripped because a chat colour is a control byte and renders as garbage in a console. No plugin change
is needed — every existing `reply` / `replyT` call site is routed correctly automatically.

Alongside it, `CommandInvocation` gains:

- a readonly `replySource` (`"server" | "console" | "chat"`) recording how the command was invoked,
  plus the exported `ReplySource` type;
- `replyToChat` and `replyToConsole` — explicit targets that force the answer into a specific channel
  regardless of how the command was invoked (SM `PrintToChat` / `PrintToConsole`).

`Commands.dispatch` takes an optional trailing `replySource` so a plugin re-dispatching a command on
a player's behalf can say which channel the answer belongs in; omitted, it falls back to the caller's
slot (SM `FakeClientCommand` parity). `Commands.handleChatTrigger` always dispatches as `"chat"`,
including for the silent `/` trigger.

Also fixes a latent hazard on the same surface: the context's reply methods no longer depend on their
receiver, so a plugin that hands `cmd.reply` to a helper as a bare function reference keeps working
instead of throwing a silently-swallowed `TypeError`.
