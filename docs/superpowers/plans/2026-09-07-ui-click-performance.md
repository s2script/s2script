# UI click performance fix

B counter clicks consistently coincided with slow server frames. Live profiling isolated repeated entity discovery inside binding validation: one modal refresh ran 520 full entity scans, spending 20–25 ms scanning within a 25–37 ms repaint. The native class lookup walks the entity table.

## Change

Use the existing per-resource cached EntityRef while its host-backed isValid check passes. Rediscover and cache the layout when that handle is absent or dead. Preserve all client, component, entity epoch, focus, and reentrant validity checks. Keep synchronous click repaint semantics.

## Workflow and evidence

The primary agent profiled and implemented the fix. A Sol subagent independently examined the lookup amplification and reviewed the final patch without correctness findings. The regression test first failed with 100 world scans for 100 binding validations, then passed with zero; silent entity replacement still invalidates the component binding and resends unchanged text to the new entity.

Live before: B refresh paint 25, 26, 37 ms; 520 scans each (20, 21, 25 ms scanning). Live after: 4, 3, 3, 3 ms; zero scans each. Initial B open went from 38 ms / 679 scans to 7 ms / zero scans. Timings use temporary Date.now instrumentation on the same server and map, driving the normal modal repaint through fixture commands on a bot. These small samples measure repaint duration, not whole-frame percentiles or human click latency. Instrumentation is excluded from the final bundle and commit.

Validation: regression red/green; 181 UI/component/prelude tests; 38 shipped-bundle UI tests; full scripts/ci-js.sh including Docker gate. No native code changed. Live native binaries from the interop continuation were preserved. Human click retest remains necessary to confirm the original warning symptom is gone. The separate intermittent reconnect blank-text observation remains unresolved.
