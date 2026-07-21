---
"@s2script/sdk": minor
---

L1 lifecycle v2: the plugin is a typed artifact. New `@s2script/sdk/plugin` subpath
(`plugin()`, `PluginContext`, `Scope`, `PluginHooks`); every registration verb moves to `ctx`;
`CommandContext`→`CommandInvocation` (param naming: `cmd`); usercmd `Cmd`→`UserCmdView`;
apiVersion major is now 2.x. Old ambient registration verbs are deprecated and removed in-series.
