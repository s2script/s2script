---
"@s2script/sdk": minor
---

Return disposable forward subscriptions and add typed watchOptional attachments for declared optional providers. Attachment scopes own local disposables, register synchronously after both plugins are Active, and clean up on failure, disposal, and provider or consumer unload. Reject undeclared optional watches and statically visible async attachment callbacks while preserving the types-only dependency workflow.
