---
"@s2script/sdk": patch
---

Game-event listeners and `UserMessages.onPre` hooks registered while the server starts now work.
Since plugins began loading before the first map, CS2 had not yet registered its events and user
messages, so those subscriptions silently never fired. They are now retried once the server is up.
