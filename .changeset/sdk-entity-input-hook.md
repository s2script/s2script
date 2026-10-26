---
"@s2script/sdk": minor
---

`onInput(classname, input, handler)` (and `ctx.entities.onInput`): hook an input an entity receives
before it runs, the counterpart of `onOutput`. Input names match case-insensitively, `"*"` matches
any class or input, and returning `HookResult.Handled` drops the input. The event carries the
receiving entity, the activator, the caller and the value as a string.
