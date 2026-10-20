---
"@s2script/sdk": patch
---

A plugin suppressing a game event no longer crashes a server that also runs MultiAddonManager 1.6.2.
s2script fired the suppressed event itself, which frees it, before other addons' FireEvent hooks ran.
