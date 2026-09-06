# BaseComm interop observer

This example consumes `@s2script/basecomm` as an optional protocol 2 plugin service. It
subscribes synchronously through `watchOptional`, owns both subscriptions with the attachment
scope, and queries the current policy for connected SteamIDs after subscribing. A compatible
provider reload creates a fresh attachment and repeats the current-state query.

The checked-in `.s2script/types/@s2script/basecomm/index.d.ts` is an exact verified copy of
`plugins/basecomm/api.d.ts`. It is the only BaseComm artifact needed to build the observer; the
provider source and `.s2sp` binary are not part of the consumer project. In a separate project,
obtain the same types-only artifact with:

```sh
s2s add @s2script/basecomm
```

Then move the generated dependency entry to `s2script.optionalPluginDependencies`, as this
example does. The operator installs the provider archive separately.

BaseComm notifications report policy transitions and do not replay history. The public setters
are for trusted plugins: they bypass the permission and immunity checks enforced by BaseComm's
commands and menu. A successful mute setter means the requested policy is stored; it does not
prove audio delivery when the host voice descriptor is degraded.

Build from the repository root:

```sh
node packages/sdk/dist/cli.js build examples/interop-observer
```
