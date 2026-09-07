# Typed plugin interop acceptance record

**Recorded September 6, 2026 (local date).** Slices 01–07 and the final implementation fixes are
independently reviewed. The reviewed runtime and service archives are running on the
owned test server. Dispatch isolation, decisions, transforms, malformed-value rejection,
recursion recovery, service APIs, offline ban commands and named-consumer unload have
passed live. The 1,000-cycle provider run, restoration, binding disposal, provider-only
unload/reload, map guard and controller teardown/restoration also pass. The original map
and all 25 plugins are restored. Human-client and real zones engine checks remain pending.
No merge or production deployment is claimed.

This record complements the [implementation plan](2026-09-06-plugin-interop.md),
[design](../specs/2026-09-06-plugin-interop-design.md),
[public contract guide](../../PLUGIN_INTEROP.md), and
[executable runbook](../../../tools/interop-acceptance/README.md).
The [decision appendix](#coordinator-decisions-and-costs) preserves the coordinator's
rulings beyond the ignored working ledger.

## Source, review and artifact identity

| Slice | Reviewed tip | Draft PR |
| --- | --- | --- |
| 01 — typed notifications | `8c0f35f` | [#205](https://github.com/s2script/s2script/pull/205) |
| 02 — hooks/transforms | `405fe23` | [#206](https://github.com/s2script/s2script/pull/206) |
| 03 — provider lifetimes | `d986a31` | [#207](https://github.com/s2script/s2script/pull/207) |
| 04 — zones | `f3575cd` | [#208](https://github.com/s2script/s2script/pull/208) |
| 05 — BaseComm | `e963555` | [#209](https://github.com/s2script/s2script/pull/209) |
| 06 — BaseBans | `1b71ec2` | [#210](https://github.com/s2script/s2script/pull/210) |
| 07 — named bindings | `aeb9ba0` | [#211](https://github.com/s2script/s2script/pull/211) |
| 08 — integrated acceptance | `b8a06f4` (live fixtures); `3e6701b` (runtime) | [#212](https://github.com/s2script/s2script/pull/212) |

The stack extends original PR #202, `e9f4177`; predecessors remained ancestors and
were not rebased during final integration. Full runtime source is
`3e6701b47af59552006a82d443c4c1ebe4351a25`. Final fixture source is
`b8a06f4500889f9521799b0561662bff8f700f26`, which changes three fixture registration
paths, their tests and documentation, without changing native inputs. The final
whole-stack review closed four Important findings and one Minor finding at `3e6701b`:
owned direct-import subscriptions, unsupported contract value exports, method-return
participant liveness, strict UTF-16 copying, and complete churn evidence validation.
A subsequent scoped review approved the fixture lifecycle correction at `b8a06f4`.

The deployable runtime was built using `scripts/build-sniper.sh` in the supported
bullseye environment and packaged with `scripts/package-addon.sh`. Core requires
GLIBC 2.30; shim requires GLIBC 2.17, both below the server ceiling 2.31. The applied
`3e6701b` bundle SHA-256 was
`0541f6481ba871b8dcd1350eae309def74363f93cfeb5bf0c8564a293d605b17`.
The earlier `590335c` staged bundle was never applied. Three corrected fixture
archives were subsequently hot-loaded without another core/server restart.

| Final applied artifact | SHA-256 |
| --- | --- |
| `libs2script_core.so` | `ec892f9bcde1c148840c89eb79201a9348c4f51e3113ea09daa082c9bfd0a2f2` |
| `s2script.so` | `29eea0e204bb7dfd04e3319b2dfcb06c2fb4c54336e47d66ba979e09ee309990` |
| `js/pawn.js` | `e871555439bd3147e8124170f30e186d610fef25df43622638071eec4c83baca` |
| `_s2script_zones.s2sp` | `dc834b5e913eba15261ac9fb0f817cacfd2dd4638aceef6456765ff4f028c3d4` |
| `_s2script_basecomm.s2sp` | `fcf79592fe21957c3e697da10e0bbf8455e013e5330fdb4d55f9428a1c42c07f` |
| `_s2script_basebans.s2sp` | `b5c56720c5f6670a5bc61a5e0cfbe76f67ac2da8b0c28067520800f74fcb46a5` |
| `_interop_controller.s2sp` | `a0c7ae37a0f0b19149c67bd9f70c1e086770a96f3473118dd786b8f1dce93da3` |
| `_interop_named.s2sp` (`b8a06f4`) | `90965226715667d22f164e674964ead294a2d63029f32a3457b051e3dd1ad6b1` |
| `_interop_numeric.s2sp` (`b8a06f4`) | `85891c4d67b369a776f59da7c1bde021c40467580d87468a42da69e59cbdabe9` |
| `_interop_text.s2sp` (`b8a06f4`) | `e6e9a4d195118b2934055256ccf8568757df8ca43f12f17e01fa39289acfc97b` |

Native/service hashes come from the packaged `3e6701b` `SHA256SUMS`; the three
replacement fixture hashes come from the `b8a06f4` fixture `SHA256SUMS`. Remote staged
and applied hash checks passed. Configs, data, translations, unrelated plugin archives
and operator `custom/` gamedata were preserved. The verified pre-final backup was
`.gate/backups/s2script-before-interop-590335c.tar.gz`, SHA-256
`2a5cabac64ac290e6338abcf1dbfbeb9812e13a5d97865e720d3c314e736dc65`.

## Verification results

| Check | Frozen source and result |
| --- | --- |
| Clean Linux `CI=1 bash scripts/ci-js.sh` | `3e6701b`: npm ci, 726 SDK tests, 12 acceptance tests, all remaining JS gates and Docker gate pass |
| Linux `bash scripts/ci-native.sh` | `3e6701b`: 902 core tests pass, 3 existing ignored; three separate stress tests pass; shim selftest and all 47 core entry points pass |
| Linux `bash scripts/test-interop.sh js` | `b8a06f4`: 14 acceptance tests, four strict archive builds and four sibling hash relationships pass |
| GitHub full JS workflow | `b8a06f4`: [run 34068153029](https://github.com/s2script/s2script/actions/runs/34068153029) succeeds on Ubuntu/Node 22 at 23:56:04 UTC |
| GitHub full native workflow | `b8a06f4`: [run 34068153014](https://github.com/s2script/s2script/actions/runs/34068153014) succeeds on Ubuntu at 23:56:25 UTC |
| Sniper/package | `3e6701b`: deployable build succeeds; GLIBC requirements above verified |
| Scoped fixture regression | Both providers and named binding fail outside the mocked load window before the fix; all three actual fixture startup paths pass after it |
| Types-only acquisition | Existing SDK test removes producer source, requests only registry metadata/types, builds without a producer archive, and rejects stale bytes; missing contracts also reject |
| Compatibility | Native/API/archive tests retain API-2/protocol-1 behavior and enforce complete API-3/protocol-2 metadata; earlier `d986a31` live smoke loaded all 21 original archives |

No actual old API-2 host was started for a new protocol-2 archive. Refusal is covered
by the API/version and archive compatibility contract, not claimed as that separate
live experiment. The final `b8a06f4` change is fixture/tests/docs only, so its focused
Linux test/build gate supplements the local full `3e6701b` gates. Separately, both
GitHub workflows execute the complete CI scripts and passed on `b8a06f4`; those are
full workflow results, distinct from the local focused run.

Retained operator logs include `interop-final-3e6701b-linux-js.log`,
`s2script-final-fix-ci-native.log`, `interop-final-3e6701b-sniper.log`, and
`interop-b8a06f4-linux-fixtures.log` under the coordinator's local temporary evidence
area. They are not repository dependencies. Exact compact live results are recorded
below so their meaning survives the temporary working ledger.

## Live server and completed probes

Only `s2script-cs2-hardening` at `nebula.gkh.dev:27016` was modified. Production
port 27015 was outside scope. The server reported Linux dedicated,
CS2 `1.41.7.8/14178 10896`, `de_inferno`, zero humans and two bots. Occupancy was checked
again before applying the runtime and before hot-loading the corrected fixtures.
All 25 plugins were running before the churn sequence. This is an owned test-server
acceptance run, not a production rollout.

### Notifications, decisions, transforms and malformed values

`s2_interop_probe` passed the strict `verify.mjs probe` validator. Numeric and text
interfaces share all three forward names, but preserve distinct payloads. Each normal
notification reached the optional consumer once despite mutations in the named
consumer's private copy. Numeric patches added 1 and 10; text patches each appended
one exclamation mark. Normal HookResult was Handled (2); Stop was 3. Six malformed
payloads were rejected before any delivery. One bounded recursion error was observed,
and the following valid call recovered. Actual reply:

```json
{"a":{"action":2,"final":"{\"mode\":\"normal\",\"value\":12}","original":"{\"value\":1,\"mode\":\"normal\"}","result":1},"b":{"action":2,"final":"{\"mode\":\"normal\",\"text\":\"seed!!\"}","original":"{\"text\":\"seed\",\"mode\":\"normal\"}","result":1},"normal":{"numeric":1,"text":1},"stop":{"numeric":3,"text":3},"malformed":6,"malformedDelivered":0,"recursion":1,"recovery":2,"isolationFailures":0,"resources":{"attachments":4,"callbacks":4,"disposers":15,"ledger":66,"methods":19,"pending":0,"subscriptions":17,"watches":4}}
```

### Service API and server-console operations

`s2_interop_services` passed `verify.mjs services`. Its unoccupied offline domain
fixture identity was `18446744073709551615`; it was never used as authentication or
as a fabricated bot SteamID. It exercised mute/gag policy, invalid identity rejection,
ban cache readback, request/record/removal notifications and cleanup. Actual reply:

```json
{"mute":true,"gag":true,"invalidMute":false,"invalidBan":{"recorded":false,"result":0},"ban":{"recorded":true,"result":0},"unban":true,"events":{"mute":2,"gag":2,"request":1,"recorded":1,"removed":1},"cleaned":true}
```

Then `sm_addban 18446744073709551615 0 interop-command` reached the shared operation;
`s2_interop_service_status` confirmed exact reason, permanent expiry zero and the
additional request/record notifications:

```json
{"id":"18446744073709551615","ban":{"reason":"interop-command","until":0},"muted":false,"gagged":false,"events":{"mute":2,"gag":2,"request":2,"recorded":2,"removed":1}}
```

`sm_unban 18446744073709551615` removed that cache entry and emitted one additional
removal; policy remained clear:

```json
{"id":"18446744073709551615","ban":null,"muted":false,"gagged":false,"events":{"mute":2,"gag":2,"request":2,"recorded":2,"removed":2}}
```

These establish cache/policy and server-console behavior. `Bans.add` has no durable
write acknowledgement, and these outputs do not establish disk durability, human
permission/immunity behavior, audio delivery, authenticated kicks or reconnects.

### Named consumer unload and resource baseline

Before churn, the named consumer reported numeric/text hits 19/2, matching the
controller's numeric hit count. `sm plugins unload @interop/named` then removed six
subscription callbacks and reduced ledger resources from 66 to 57. Other interop
counts stayed unchanged:

| Counter | Both consumers active | Named consumer unloaded | Numeric provider absent (churn baseline) |
| --- | ---: | ---: | ---: |
| Watches | 4 | 4 | 4 |
| Watch callbacks | 4 | 4 | 4 |
| Attachments | 4 | 4 | 3 |
| Owned disposers | 15 | 15 | 11 |
| Pending attachments | 0 | 0 | 0 |
| Subscription callbacks | 17 | 11 | 8 |
| Method callbacks | 19 | 19 | 16 |
| Active ledger resources | 66 | 57 | 51 |

The private diagnostic counts are aggregate host resources; no public SDK diagnostic
API was introduced. Unrelated plugin changes would invalidate exact comparisons.
The original named consumer unload and intermediate provider absence are completed
observations; they do not by themselves prove the full 1,000-cycle run.

### Completed provider churn

`s2_interop_churn` completed 1,000 actual deferred provider load/unload cycles. The final
`s2_interop_status` reply passed the strict `verify.mjs churn` validator: `state="done"`,
1,000 cycles, exactly 1,000 deliveries, 1,000 expired-proxy refusals, and no error.
All eight counters are valid nonnegative safe integers; baseline and final are equal
and pending attachments are zero. Numeric remained unloaded at completion, so the
final resource snapshot is the provider-absent baseline, not the initial all-loaded state.
The run performed one initial baseline unload plus 1,000 cycle unloads and 1,000 loads.
Actual final reply:

```json
{"state":"done","cycles":1000,"deliveries":1000,"staleBlocked":1000,"error":"","baseline":{"attachments":3,"callbacks":4,"disposers":11,"ledger":51,"methods":16,"pending":0,"subscriptions":8,"watches":4},"final":{"attachments":3,"callbacks":4,"disposers":11,"ledger":51,"methods":16,"pending":0,"subscriptions":8,"watches":4},"attached":1002,"detached":1002,"hits":1019,"map":0,"resources":{"attachments":3,"callbacks":4,"disposers":11,"ledger":51,"methods":16,"pending":0,"subscriptions":8,"watches":4}}
```

The total attachment/detachment counters of 1,002 include the two observations before
the cycle run; `hits=1019` includes 19 earlier probe deliveries. The cycle-specific
counters are the acceptance quantities. Intermediate samples were progress evidence
only; the final completed reply establishes this gate.

### Restoration and whole-map disposal

After churn, numeric was loaded and its optional attachment confirmed before named was
reloaded. `probe-restored.json` passed the same complete dispatch validator, with
17 subscriptions and 66 ledger resources restored. Two `s2_interop_dispose` calls
then demonstrated whole-map disposal and idempotence:

| Current resources | Before disposal | After first call | After second call | After named reload |
| --- | ---: | ---: | ---: | ---: |
| Subscription callbacks | 17 | 11 | 11 | 17 |
| Active ledger resources | 66 | 60 | 60 | 66 |
| Attachments | 4 | 4 | 4 | 4 |
| Method callbacks | 19 | 19 | 19 | 19 |

All other interop counters were unchanged. The first and second status objects were
identical; `disposal-second.json` also contained the subsequent reload acknowledgement,
which is not part of that equality comparison. `probe-after-disposal.json` passed after
reload, including notification isolation and the original transform results.

### Manual provider-only unload and explicit hard-consumer reload

`sm plugins unload @interop/numeric` was run while named was active. Numeric became
unloaded; named and text remained running. This is a provider-only unload, not an
automatic dependent cascade. The historical evidence filename `cascade-unload.json`
does not describe the runtime policy. The observed lifecycle was:

| Current resources | Both consumers/providers active | Numeric absent; named running | Numeric restored; optional watch reattached | Explicit named reload |
| --- | ---: | ---: | ---: | ---: |
| Subscription callbacks | 17 | 11 | 14 | 17 |
| Active ledger resources | 66 | 57 | 63 | 66 |
| Attachments | 4 | 3 | 4 | 4 |
| Owned disposers | 15 | 11 | 15 | 15 |
| Method callbacks | 19 | 16 | 19 | 19 |

Watches and watch callbacks stayed at four; pending stayed zero. Three hard numeric
bindings and three optional numeric subscriptions disappeared on provider removal;
text bindings remained. `watchOptional` recreated its three subscriptions on the new
provider generation. An explicit named-consumer reload recreated its hard bindings.
`probe-after-provider-reload.json` then passed the complete dispatch validator.

The existing reverse-dependency order governs teardown of a set, including full
unload. A manual single-plugin unload selects that plugin; it does not promise to
unload its dependents. Hard forward bindings removed with a provider are registered
again by reloading the consumer. `watchOptional` is the supported automatic attachment
path. No new cascade policy or runtime change was introduced during acceptance.

### Map guard and post-map dispatch

On `de_inferno`, `s2_interop_map_capture` returned `{"map":0,"captured":2}`. After
changing to engine-confirmed available `de_dust2`, with zero humans and two bots,
`s2_interop_map_check` returned `{"map":1,"captured":2,"blocked":2}`. Both copied
previous-map actions would therefore be refused by the fixture's map/userId guard.
The guard performs no client actions; this is not an authenticated kick/reconnect
or actual stale-slot action test. `probe-after-map.json` passed every dispatch check,
with all eight resource counters restored to the 17-subscription/66-ledger baseline.
The original `de_inferno` was restored before the final controller and identity checks.

### Controller unload and restoration

The independent `sm_mixed_stats` reader reconstructed each framed three-part diagnostic
reply before assertions. `controller-before.json`, `controller-after.json`, and
`controller-restored.json` show the following exact interop resource transitions:

| Counter | Before unload | Controller unloaded | Controller reloaded |
| --- | ---: | ---: | ---: |
| Watches | 4 | 0 | 4 |
| Watch callbacks | 4 | 0 | 4 |
| Attachments | 4 | 0 | 4 |
| Owned disposers | 15 | 0 | 15 |
| Pending attachments | 0 | 0 | 0 |
| Subscription callbacks | 17 | 6 | 17 |
| Method callbacks | 19 | 19 | 19 |
| Active ledger resources | 66 | 41 | 66 |

All controller watch callbacks, attachments, owned disposers and 11 subscriptions
were removed. The six hard subscriptions in named remained. Reload restored the
entire eight-counter interop snapshot exactly; unrelated timing fields in the full
async diagnostic are not part of that equality assertion. The final dispatch and
service replies passed their strict validators again. Final service state was
`ban:null`, `muted:false`, `gagged:false`; the service probe reported `cleaned:true`.

### Final restored server state

`final-server.log` confirms all 25 plugins running, zero humans and two bots, and the
original `de_inferno` map. Metamod lists s2script and the existing MultiAddonManager.
The test container retained start time `2026-09-06T23:43:26.856332391Z` and reported
restart count zero in `final-identity.log`; fixture reloads, churn and map changes
did not require another container restart. The remote operator checkout remained
clean at `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1`. That checkout identifies the
operator environment, not the applied plugin/native source: the deployed artifacts
are the `3e6701b` runtime/services and three `b8a06f4` replacements identified above.

Final remote hashes matched all ten listed artifacts against the packaged manifest
with the three fixture overrides. The recent final server log contained no matching
panic/fatal/failure/error/exception diagnostics. This observation concerns that
captured final log window, not a claim that the intentionally malformed/recursive
probes or the earlier fixture startup failure never logged errors.

## Remaining live gates

- **Zones engine enter/leave and provider reload: pending separate live evidence.**
  Compiler and plugin-operation tests pass, but fixture numeric/text lifecycle checks
  do not establish real zones boundary behavior.

## Human checks and other limits

Authenticated voice delivery/mute, gagged chat, command permissions and immunity,
visual menu interaction, authenticated ban/kick/reconnect, and actual stale
connection replacement effects remain pending human-client observations. The two
bots have SteamID `0`; offline policy and bot command replies cannot establish those
gates. Existing service VM tests cover operation convergence and identity races,
which is narrower evidence than these engine checks.

The first integrated native gate hit the inherited
`net_same_batch_connect_data_error_close_keeps_subscription_checkpoint` timing
failure. The same focused failure reproduced on unchanged parent `aeb9ba0`; an
unmodified full rerun passed. No assertions were weakened. Initial clean-Linux JS
attempts failed on old Git `--path-format` support and then missing socket-inspection
tools; the corrected bookworm image completed the full gate. Initial SSH signing and
backup-permission failures were resolved before final staging, with backups verified.
Inherited compiler warnings remain deferred. The final npm ci output also reported
one high-severity audit advisory and an install-script warning; no dependency-security
clearance is inferred from the green JS gate.

Live startup found a fixture bug that offline mocks had hidden: numeric/text called
`publish` during CJS evaluation, before the load window. Named bindings had the same
placement. Commit `b8a06f4` moved all three registration paths into `OnPluginStart`,
made the VM authorization phase faithful, and corrected public examples. Red tests
reproduced the live errors; the 14-test/four-build gate and subsequent real dispatch
probe passed. This acceptance correction does not invalidate the unchanged reviewed
native runtime, and no runtime registration authority was widened to accommodate it.

## Coordinator decisions and costs

The following lines are copied from the execution ledger, including the stated
rationale and cost. They preserve decisions made while the stack was being built;
they do not override the final public API documentation or the pending gates above.

### Decision 1

> Ruling: Use explicit protocol 2 opt-in for new strict checks; preserve protocol 1 callers — avoids unplanned migration breakage — cost if wrong is a later migration change.

### Decision 2

> Ruling: Native/live tests use supported Docker Linux/owned test environment when available; do not treat local macOS limitations as green — preserves verification integrity — cost is pending acceptance if infrastructure unavailable.

### Decision 3

> Ruling: host API major 3 accepts legacy major 2 archives explicitly, while protocol2 requires major3 — old hosts must fail closed rather than ignoring metadata — cost is compatibility testing and a public API major migration.

### Decision 4

> Ruling: use one explicitly specified cross-language canonical format with numeric and Unicode edge tests; a focused maintained crate is acceptable — serde_json reserialization is not JS canonicalization — cost is a small dependency and metadata compatibility tests.

### Decision 5

> Ruling: provider removal during a decision dispatch stops remaining listeners and throws InterfaceUnavailable at the return boundary; effects already performed are not rolled back — a removed generation cannot cross back to its producer — cost is explicit producer error handling. Preserve existing named Error.message convention.

### Decision 6

> Ruling: move cookbook legacy econ/workshop recipes into a documented protocol1 companion example while opting the zones cookbook into protocol2 — existing Promise-returning/unverified legacy contracts cannot satisfy the plugin-level strict opt-in, and mixed-protocol runtime support would expand this adoption slice — cost is relocating those example commands/imports and maintaining discoverability. Preserve their behavior and build/typecheck coverage.

### Decision 7

> Ruling: basecomm setter boolean means accepted/current policy equals requested state, including idempotent requests; invalid canonical nonzero u64 SteamID or nonboolean state returnsfalse — separates operation acceptance from change-only notifications and supports offline policy — cost is documenting this return meaning for callers expecting changed-state booleans. Commands must snapshot intended SteamIDs before callbacks, especially multi-target/silence paths.

### Decision 8

> Ruling: basebans public ban(request) snapshots a matching currently connected identity before request callbacks and kicks that same identity only after confirmed record/notification; sm_addban remains record-only, while sm_ban/menu pass copied target identity into the same operation — public ban should perform a ban for an online target, preserving existing offline-command policy — cost is documenting the API's online kick effect. Re-resolve userId+SteamID after both callback boundaries.

### Decision 9

> Ruling: unban(request) takes a self-contained UnbanRequest {steamId:string}; invalid domain requests returnfalse (ban returns recorded:false/result:Continue) before hooks/store/side effects — fills the plan's unspecified unban request shape with the smallest wire contract — cost is committing callers to object-shaped unban.

### Decision 10

> Ruling: recorded confirms immediate Bans.get cache readback, with exact reason and expiry consistent with validated minutes/current call's second bounds; never infer durability from voidBans.add — actual native writes cache before persistence and has no acknowledgement — cost is that identical preexisting cache state cannot prove a fresh durable write, which the API explicitly does not promise.

### Decision 11

> Ruling: permit authoritative SDK HookResultValue type import from events/barrel in protocol2 contracts, resolved to existing literal-union wire schema — BanResult must reuse the existing result type rather than a competing enum — cost is a bounded additional SDK helper in the governed import allowlist; domain/other SDK imports remain rejected.

### Decision 12

> Ruling: include a small Basecomm follow-on actor-reply guard/test in Task6 alongside discovered Basebans guard — forTargets also translates/replies to raw callerSlot after arbitrary notification callbacks, confirmed in source; fixing on the current child avoids rewriting the active stack — cost is a cross-service correctness fix in the ban-service PR. Preserve existing setter behavior.

### Decision 13

> Ruling: bindForwards uses hard-dependency normal-load policy, matchinguse and its always-presentSubscription return; optional integrations bind named local functions with watchOptional scopedservice.on — avoids silent absent-provider binding and widened attachment authorization — cost is no optional-present-at-load bindForwards convenience. Require compiler/runtime agreement and document path.

### Decision 14

> Ruling: Task8 fixture uses two independently reloadable provider archives plus two consumers — required same-name cross-interface isolation and optional-provider generations need separate producer lifetimes — cost is one extra fixture archive versus singularproducer wording in plan.

### Decision 15

> Ruling: final cross-slice correctness repairs land on current interoperability08 child — user requested minimal rebase and all earlier PRs are stacked/published — cost is broader final acceptance PR diff rather than rewriting seven reviewed branch tips.

### Decision 16

> Ruling: reject unpaired UTF16 surrogates in strict wire strings/property names, preserve validBMP/nonBMP — matches canonicalmetadata and avoids silent datamutation — cost is explicit serialization refusal for malformedJavaScriptstrings rather than transporting lonecodeunits.

### Setter return clarification

> Basecomm setter ruling clarification: evaluate final policy equality after synchronous notifications; valid-but-reversed requests returnfalse, distinct from invalid/no-side-effectfalse. Silence must invoke both setters without short-circuiting and query both final policies after both callbacks before reporting combined success. Required focused reentrancy regression sent toTask5 implementer.

This clarification requires two non-short-circuiting policy writes and final readback in
combined silence operations; callers must distinguish invalid requests from valid
requests reversed by synchronous observers. It adds behavioral tests rather than a
new API surface.
