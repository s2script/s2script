---
"@s2script/cs2": patch
---

`Player.all()`, `Player.allConnected()` and `Player.fromSlot` check each slot through faster engine calls, so listing players costs about a tenth of what it did. No API change.
