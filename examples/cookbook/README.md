# Cookbook

Each module in `src/recipes/` demonstrates one API. The plugin entry registers the
recipes together; copy individual recipes into your own plugin for regular use.

The zones recipe uses protocol 2 (`s2script.interfaceProtocol: 2`) and an optional
`@s2script/zones: ^1.0.0` dependency. Its verified declaration is a byte-copy of
`plugins/zones/api.d.ts`. Rebuild both provider and consumer when that contract
changes. The recipe watches for a compatible provider, subscribes during each
attachment, then queries `getZones()` for the current layout. Notifications do not
replay existing zones. Event handlers resolve players by `userId`.

The `sm_econ` and `sm_workshop` recipes are in the
[legacy-contracts companion](../legacy-contracts/README.md). Those community
contracts retain their protocol 1 behavior, including asynchronous workshop
methods. Protocol selection applies to the whole plugin, so they need a separate
archive from the protocol 2 cookbook.

Build from the repository root:

```sh
node packages/sdk/dist/cli.js build plugins/zones
node packages/sdk/dist/cli.js build examples/cookbook
node packages/sdk/dist/cli.js build examples/legacy-contracts
```
