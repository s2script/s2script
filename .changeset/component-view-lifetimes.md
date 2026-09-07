---
"@s2script/cs2": patch
---

Bind retained HUD component views and delayed component callbacks to the current client and component lifetime. Add `isValid()` to retained HUD, modal, dashboard, badge, MOTD, and hudkit player views so reconnects, releases, and layout replacement are observable without targeting a new occupant.
