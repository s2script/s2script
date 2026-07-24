---
"@s2script/sdk": minor
---

Chat messages now render their first color byte without a hand-written leading space. `Chat.toSlot` / `Chat.toAll` (and every chat-bound `ctx.reply`) auto-prefix each line with an invisible zero-width space (U+200B), so a Source 2 chat box no longer swallows a color control byte that sits at index 0. Write `Chat.toSlot(slot, ChatColors.Green + "hi")` and the green lands. The prefix is idempotent — a line you already lead with a space or a ZWSP is passed through unchanged — and chat-only, so console / rcon replies stay byte-clean.
