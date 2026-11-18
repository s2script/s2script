---
"@s2script/cs2": patch
---

`HudInput` holds its usercmd subscription only while a slot is armed, so a plugin that once armed Tab no longer runs JS for every player's every command afterwards. The ui focus reconciler no longer copies an empty list each frame. No API change.
