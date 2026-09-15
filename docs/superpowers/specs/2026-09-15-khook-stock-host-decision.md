# KHook PR A — stock Metamod requirement

**Status:** Current user requirement: stock Metamod, resident native s2script,
and `.s2sp` hot reload managed inside s2script.

## Decision and precedence

s2script must run on unmodified upstream Metamod and its bundled KHook. Do not
ship a private host build requirement, modify either dependency, or disguise an
upstream change as a plugin-side write into host internals. This supersedes the
host-patch prescriptions in the remediation design and both remediation plans,
including the later narrowed bookkeeping patch. No patch-dependent implementation
or build identity is an accepted deliverable under this decision.

The independent shim, controller and fixture corrections remain relevant. Public
JavaScript APIs and the PR A/B boundary remain unchanged. PR A is still draft;
this decision does not establish runtime acceptance or authorize merging.

## Evidence and unresolved capability

The pinned official `samples/s2_sample_mm/src/plugin.cpp` calls `Virtual.Remove`
in `Unload()` and returns true. That method removes an object routing filter; it
does not physically remove the native hook. Normal Metamod unload honors a false
plugin result, retaining the plugin and its provider for a later retry. Neither
normal KHook use nor JavaScript plugin reload has been shown to require a patch.

The audit identified a separate native-library release limitation in the pinned
stock source: virtual IDs remain in the host ownership set after explicit removal,
and a later asynchronous removal of an absent ID receives no completion. The stock
unloader can therefore retain its library reference. This is a source-level and
sanitizer-harness result, not live CS2 evidence. It does not establish that s2script's
overall integration is fundamentally wrong, or justify requiring a modified host.

The user clarified that only `.s2sp` hot reload is required. s2script owns this
lifecycle; Metamod remains unaware of individual JavaScript plugin reloads. The
native shim and host provider remain loaded during gameplay. Native shim hot
unload/reload and actual native library unmapping are not PR A acceptance
requirements. Native updates use a server restart. Failed-load cleanup and safe
process shutdown remain relevant and must be tested separately. A refusal from
ordinary `Unload()` does not, by itself, make forced process shutdown safe.

For `.s2sp` reload, retire the outgoing plugin's subscriptions and plugin-owned resources,
prevent stale callbacks, then activate the replacement. Shared native hooks can
remain installed for the resident shim's lifetime. Removing one plugin's routing
or subscription must not retire the native host or disturb another plugin.

## Revised worker handoff

1. **Stock integration audit — lifecycle owner.** Compare our registration,
   subscription removal and teardown against the pinned public API and official
   sample. Identify unnecessary shim machinery and prove proposed simplifications
   using actual production paths. No dependency edits or replacement hook engine.
   Deliver `.s2sp` reload and server-shutdown behavior with tests and explicit
   limitations. Remove machinery needed only to support native shim hot reload.
2. **Independent corrections — existing controller and fixture owners.** Preserve
   accepted command dispatch, checked-helper cleanup and evidence fixes. Resolve
   remaining controller identity-validation and fixture resume findings with scoped
   real-code regressions. Do not add a patched-host dependency to these changes.
3. **Stock delivery — build/installer owner.** Remove the patch series and its
   application path, custom-host installation requirement and patch-digest identity
   contract. Update installation, verification, CI and notices together. A
   reproducible unmodified source build may be a test tool; operators must not need
   our custom Metamod build. Keep binary validation and stale-artifact rejection.
4. **Integration — coordinator.** Replace patch-specific host tests with stock
   compatibility, `.s2sp` reload and server-shutdown tests. Replace the native
   unload/reload acceptance case with actual JavaScript plugin reload evidence;
   do not merely relabel old native observations. Update the
   acceptance schema and producer/controller bindings together; preserve missing
   evidence as pending and observed failures as failures. Keep native and JS reload
   evidence separate. Do not push the currently patch-dependent integration as a
   completed stock-host solution.
5. **Review and validation — independent reviewer.** Confirm no upstream patch or
   host-internals workaround remains; review lifetime behavior on the agreed stock
   host. Run applicable local tests and Linux/sniper builds, then live CS2 with both
   plugin orders and process shutdown. Report unavailable live/human evidence
   explicitly. Local harness results alone do not complete acceptance.

The [current remediation spec](2026-09-14-khook-pr-a-remediation-design.md) and
[dynamic plan](../plans/2026-09-15-khook-pr-a-review-fixes.md) now incorporate this
confirmed requirement. Earlier host-patch and native-reload prescriptions are
historical and must not be dispatched. The dynamic plan records the separate,
still-unverified stock-host shutdown prerequisite.
