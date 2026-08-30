# `hook` is game events only — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** `hook.on` / `hook.onPre` for game events; named publics for lifecycle; delete `hook.client` / `entity` / `server` / `events`.

**Architecture:** Prelude flattens `hook` and wires new exports in `__s2_run_factory`. Callers migrate. Cookbook fans out from plugin.ts.

**Tech Stack:** prelude.js, plugin.d.ts, isolate tests in v8host.rs.

## Global Constraints

- 0.x minor `@s2script/sdk`
- Base: `cursor/hook-events-a8c9`. Branch: `cursor/hook-only-events-a8c9`.
- Do not rustfmt all of `v8host.rs`.

---

### Task 1: Prelude, types, callers, tests

- [x] Flatten `hook` to `on` / `onPre`; add named-public wiring + `onOutput`; delete subject objects.
- [x] Migrate plugins/examples/tools/cookbook/fixtures; isolate tests.
- [ ] Changeset + gates.
