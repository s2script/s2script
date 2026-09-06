# UI multi-plugin acceptance fixture A

This test-only plugin drives three public hudkit surfaces in one plugin context: a
focus-aware keyed modal, an existing pooled corner badge, and an explicitly owned
banner. Plugin B is a separate package and context; the fixtures share no JavaScript
state.

Build from the repository root after UI hardening Tasks 3–6 are integrated:

```sh
node packages/sdk/dist/cli.js build examples/ui-multiplugin-a --packages-dir packages
```

Commands accept an optional zero-based slot and otherwise use the in-game caller:

| Command | Action |
| --- | --- |
| `sm_ui_a_open [slot]` | Open A's exclusive modal, show its pooled badge, and acquire its owned banner. |
| `sm_ui_a_reorder` | Rotate and mutate the backing domain records without provider evaluation or repaint. |
| `sm_ui_a_refresh [slot]` | Repaint synchronously with `tryRefresh`. |
| `sm_ui_a_invalidate [slot]` | Queue a coalesced repaint; the reply reports the immediate provider-call delta. |
| `sm_ui_a_banner_busy [slot]` | Attempt a second owned banner while A's first handle is live; expect `Busy`. |
| `sm_ui_a_close [slot]` | Close A's modal, hide its badge, and dispose its banner handle. |
| `sm_ui_a_release` | Close all views, dispose all banner handles, and release the modal and badge pool claims. |
| `sm_ui_a_status [slot]` | Print source, provider, click, ownership, handle, and last-update state as JSON. |

After `sm_ui_a_open`, run `sm_ui_a_reorder` without refreshing and click the first
visible row. `sm_ui_a_status` reports the painted stable ID, the now-different source
ID at that absolute index, and the current domain record found by stable ID. This is
domain revalidation evidence; it does not authorize a stale domain action.

The commands and counters provide server-side evidence only. A human still must prove
rendering, click delivery, focus coverage/restoration, cursor release, reconnect into
the same slot, plugin reload cleanup, and pool reclaim on the integrated live server.
