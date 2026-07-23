---
"@s2script/sdk": minor
---

The `s2s` CLI is now interactive: arrow-key prompts for `create` and `login`, a no-arg command menu (`s2s` in a terminal), and spinners on `build`/`deploy`/`add`, with consistent styling. Every non-interactive path (flags, `-y`, `--ci`, no TTY, CI) behaves exactly as before — same output and exit codes; `build` still prints the plain `.s2sp` path to stdout. Powered by `@clack/prompts`, bundled into the CLI, so there is no new runtime dependency.
