---
"@s2script/sdk": patch
---

Game-event listeners a plugin registers at startup now actually fire. CS2 accepted them, then reset
its event manager while loading its event definitions and silently dropped every one, so `on(...)`
handlers such as `player_spawn` or `hegrenade_detonate` never ran. Each accepted listener is now
checked once the server is up and re-registered if the engine dropped it.
