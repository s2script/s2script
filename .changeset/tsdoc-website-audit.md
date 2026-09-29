---
"@s2script/sdk": patch
"@s2script/cs2": patch
---

TSDoc brought in line with shipped behavior. `Player` documents its connection lifetime (#188),
`Menu.freezePlayer` notes that the CS2 center renderer ignores it (#168), `Database.query` /
`execute` name their admission and size errors, and the structured UI results, owned surfaces,
click subscriptions and modal/dashboard lifecycles are documented. Doc comments and runtime
load-window errors name the free imports (`use`, `tryUse`, `bindForwards`, `createScope`,
`hook.on`, `translations.load`, `OnPlayerRunCmd`) instead of the removed `ctx.*` API.

The doc-model extractor keeps type parameters in function and method signatures
(`create<const ButtonId extends string = string>(…)`), so generated API references show them.
