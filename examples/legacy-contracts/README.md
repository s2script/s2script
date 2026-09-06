# Legacy community contracts

This companion preserves the cookbook's `sm_econ` and `sm_workshop` examples.
It uses the existing optional protocol 1 `econ` and `workshop` interfaces; absent
providers leave the commands available with their existing installation guidance.
The workshop contract remains asynchronous.

These examples live in a separate plugin because the main
[cookbook](../cookbook/README.md) opts into protocol 2, whose verified wire
contracts require synchronous methods. Existing community providers require no
migration to use this example.

From the repository root, build the CLI and this plugin:

```sh
npm run build --workspace=@s2script/sdk
node packages/sdk/dist/cli.js build examples/legacy-contracts
```

Install `dist/_example_legacy-contracts.s2sp` on a development server and run
`sm_econ <slot>` or `sm_workshop`. Like the cookbook, this is a demo and is not
included in the base plugin release.
