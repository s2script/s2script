---
"@s2script/sdk": minor
---

UserMessage interception: `UserMessages.onPre(name, handler)` / `UserMessages.off(name)` with a
block-scoped `UserMessageView` (typed scalar reads with dotted nested paths, read-only recipients,
`debugString` fallback). Returning >= `HookResult.Handled` suppresses the send for every recipient.
Fail-closed: an unresolvable name (or a degraded intercept descriptor) throws at subscribe time.
