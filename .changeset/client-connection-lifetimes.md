---
"@s2script/sdk": patch
"@s2script/cs2": patch
---

Bind Client and Player handles to a host connection lifetime so saved handles cannot target a replacement in the same slot. Disconnect callbacks expose a synchronous read-only identity snapshot; stale getters and actions return safe defaults. Cookie loads and notifications now remain tied to their original connection.
