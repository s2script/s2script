# Plugin interop acceptance fixtures

These four private archives exercise the real protocol-2 host. `numeric` and `text`
publish independently reloadable interfaces with identical `OnSignal`, `OnRequest`,
and `OnFormat` names but different payloads. `named` binds explicit exported aliases
and both decision forwards as hard dependencies. `controller` watches optional
providers, owns attachment resources, drives deferred reloads, and checks service APIs.
The fixtures are opt-in test tools and are not included in the base-plugin package.

## Offline gates

From the repository root:

```sh
bash scripts/test-interop.sh js
bash scripts/test-interop.sh native # Linux native toolchain
bash scripts/ci-js.sh
bash scripts/ci-native.sh
```

The JS gate executes the fixture state machine, provider/binding/service operation
and evidence-validator tests, then builds all four archives and verifies four sibling
contract hash relationships. The SDK suite already run by `ci-js.sh` retains the
`interop.test.mjs` registry add/build test: it deletes the producer tree, permits only
metadata/types requests, builds without a producer archive, and rejects stale bytes.
Missing contracts also fail compilation. Native archive tests retain API-2/protocol-1
compatibility and require complete API-3/protocol-2 metadata. New SDK archives stamp
API 3, which old API-2 hosts reject; a metadata-only test is not an old server run.

The controller's BaseComm/BaseBans declarations are exact types-only copies. The two
fixture contracts resolve from workspace siblings without copied declarations.
`dist/_interop_*.s2sp` archives and generated `interfaces.d.ts` files are ignored.

## Live operator sequence

Use the authorized test-server workflow in [BUILDING.md](../../docs/BUILDING.md).
Record source SHA, CS2 build/map, native and archive SHA-256 hashes, server identity,
commands, raw replies and server logs. Build native artifacts with
`scripts/build-sniper.sh` and package with `scripts/package-addon.sh`; host-built
binaries are not deployable. Stage all four fixture archives plus the reviewed
BaseComm and BaseBans providers. Check that all are `running` with `sm plugins list`.

Run commands via the test server's RCON connection. Every fixture reply is one JSON
object below 1,800 characters. Capture the JSON object exactly; replies from the
RCON wrapper or unrelated log lines are not part of the evidence file.

1. Run `s2_interop_probe`, save `probe.json`, then validate:
   `node tools/interop-acceptance/verify.mjs probe probe.json`.
   Run `s2_interop_named` to record the named consumer's counters.
2. Run `s2_interop_services`, save `services.json`, then validate:
   `node tools/interop-acceptance/verify.mjs services services.json`.
3. Run `sm plugins unload @interop/named`; wait for `unloaded` in
   `sm plugins list`. Capture `s2_interop_status` resources before/after this
   consumer unload. It removes six subscriptions and its method imports.
4. Run `s2_interop_churn`. Poll `s2_interop_status` every 30 seconds until
   `state` is `done` or `failed`; never accept `running` as success. Capture the
   final JSON in `churn.json` and validate:
   `node tools/interop-acceptance/verify.mjs churn churn.json`.
5. Churn leaves the numeric provider unloaded. Run `s2_interop_restore`, then
   poll `sm plugins list` until numeric is running and `s2_interop_status`
   shows an additional attachment. Run `s2_interop_restore_named` only then,
   wait for named to become running, and repeat the validated normal probe.
6. Run `s2_interop_dispose` twice, recording resources with
   `s2_interop_status` after each call. The first removes six subscriptions;
   the second changes no counts. Run `s2_interop_restore_named`, wait for it
   to run, and repeat the validated probe.
7. Run `s2_interop_map_capture` with bots connected; record `captured > 0`.
   Change to an operator-selected valid map, wait for stable clients/plugins,
   then run `s2_interop_map_check`. The map counter must advance and all copied
   player actions must be blocked (`blocked === captured > 0`). This tests the
   fixture's map/userId guard; it performs no client action and does not prove
   authenticated kick/reconnect behavior. Repeat the validated normal probe.
8. Capture resources, unload the controller, and confirm its watch callbacks,
   attachments, disposers and subscriptions disappear using a second diagnostic
   reader (the existing mixed-live diagnostic fixture can read the same private
   `__s2_async_stats().interop` object). Reload it and repeat the normal probe.
   Preserve raw before/after totals and account for all other loaded plugins.

Do not reload unrelated providers during churn: resource totals are aggregate
host totals, so unrelated changes correctly invalidate the baseline comparison.
The controller refuses churn while the hard consumer is running. Each cycle
waits for deferred unload/load and three settled frames, checks exactly one new
attachment/detachment and delivery, verifies the expired proxy throws
`InterfaceUnavailable`, and compares active and absent resource snapshots. Every
transition has a 600-frame timeout. A map change aborts a running churn. There are
1,001 unloads (one initial baseline unload) and 1,000 loads. No synchronous loop
performs 1,000 reloads, and no saved scope is reused to register callbacks.

Manual provider-only unload removes that provider's subscriptions but leaves hard consumers
running, including their bindings to other providers. After manually restarting a provider,
reload each hard consumer to register its lost bindings again. `watchOptional` reattaches its
subscriptions automatically. Reverse-dependency teardown order applies to unloading a set
(such as full unload); it does not imply an automatic dependent cascade for one manual unload.

## Expected evidence

For a normal probe with both consumers active:

- Numeric: `action=2`, `result=1`, original `{value:1,mode:"normal"}`, final
  `{value:12,mode:"normal"}`. The two patches add 1 and 10 in either order.
- Text: `action=2`, `result=1`, original `{text:"seed",mode:"normal"}`, final
  `{text:"seed!!",mode:"normal"}`. Both patches append one exclamation mark.
- `normal={numeric:1,text:1}`, `stop={numeric:3,text:3}`, `malformed=6`,
  `malformedDelivered=0`, `recursion=1`, `recovery=2`, `isolationFailures=0`.
- `resources.pending=0`. Other totals depend on the full server plugin set.

The named handlers mutate their received notification copy; the other consumer
and producer must still see the original value. Three invalid payloads per
provider must fail before delivery. Nested notification/method calls reach the
shared 32-crossing limit, catch its named error, and then recover for a valid call.
Stop suppresses later listeners according to the current registration order;
counts of stop listeners are not assumed stable across server restarts.

Completed churn requires `state="done"`, `cycles=deliveries=staleBlocked=1000`,
`error=""`, and exact equality of every `baseline` and `final` counter.
The private diagnostic reports aggregate watches, retained watch callbacks,
attachments, owned disposers, pending attachments, subscription callbacks,
method callbacks and active ledger rows. It exposes no public SDK debug API.

The service probe operates only on offline domain fixture identity
`18446744073709551615`, refusing an occupied policy/ban or connected identity.
It expects mute/gag true, invalid mute false, invalid ban
`{recorded:false,result:0}`, valid ban `{recorded:true,result:0}`, unban true,
`events={mute:2,gag:2,request:1,recorded:1,removed:1}`, and cleanup true. Cleanup
runs even if a provider operation throws. This is cache/policy acceptance, not
Steam authentication or persistence acknowledgement.

Server-console command checks are separate evidence: `sm_addban 18446744073709551615 0 interop-command`, inspect
`s2_interop_service_status` (ban reason, expiry zero and transition counters), then
`sm_unban 18446744073709551615` and verify `s2_interop_service_status` reports `ban:null` and one removal. Record
BaseComm command replies for the existing bots without treating SteamID `0` as
authenticated identity. Do not change or replace a real player's ban to run a test.

## Acceptance status and limits

The [durable acceptance record](../../docs/superpowers/plans/2026-09-06-plugin-interop-acceptance.md)
records reviewed source and applied artifact hashes, collected results, pending gates and
[coordinator decisions](../../docs/superpowers/plans/2026-09-06-plugin-interop-acceptance.md#coordinator-decisions-and-costs).

Fixture implementation and offline checks do not establish live acceptance.
The linked coordinator record contains the exact frozen-artifact results for the
completed sequence above and retains separate outstanding checks. Authenticated mute/audio delivery, gag chat suppression, command
permission/immunity behavior, visual menus, authenticated ban/kick and reconnect,
and stale connection replacement effects require a human client and remain pending
until separately observed. Existing service VM tests cover command/menu/API
convergence and identity races; those are not a substitute for human engine checks.
