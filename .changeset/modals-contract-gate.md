---
"@s2script/cs2": patch
---

Keep modal footer actions bound to each player's painted view and gate MODALS against the workshop markup.

Players viewing different menu pages or action sets no longer overwrite one another's footer
handlers. This also keeps automatic pager buttons working when another player has a one-page
list. Closing or forgetting a viewer discards their handlers. `ModalSpec.buttons` now allows
per-player actions; the former shared-table restriction and diagnostic warning are removed.
