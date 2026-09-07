---
"@s2script/sdk": minor
---

Cookies.set and Cookies.setAuthId now return boolean admission results. False leaves the cache unchanged when identity is invalid or the bounded persistence outbox cannot reserve capacity. True means host ownership until database acknowledgement, including plugin reload and same-process core reinit; process exit is not crash durable. Callers should handle false. Structural mocks must return boolean instead of void.
