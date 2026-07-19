---
"@s2script/sdk": minor
---

config: new `s2s config gen <plugin.s2sp...> --out <dir>` command. It reads each staged `.s2sp`'s
baked manifest and emits the operator's default config file — commented JSONC (defaults + a
`// type — description` line per decl, sections nested) byte-compatible with the core's
`generate_default_jsonc`, at a filename that matches the runtime's ConfigPath sanitizer exactly
(`@s2script/funvotes` -> `_s2script_funvotes.json`). Plugin-scoped: it knows nothing about the
framework templates, which the release script ships separately.
