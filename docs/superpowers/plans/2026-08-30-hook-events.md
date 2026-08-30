# `hook.events.on` / `onPre` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace `hook.event(name, handler, phase?)` with `hook.events.on` / `hook.events.onPre`, and remove `hook.topmenu`.

**Architecture:** Prelude + `plugin.d.ts`; migrate in-repo callers; isolate test uses nested `hook.events.on` and free `topmenu`.

**Tech Stack:** prelude.js, plugin.d.ts, v8host isolate tests.

## Global Constraints

- 0.x minor `@s2script/sdk`
- Base: `cursor/hook-subjects-a8c9`. Branch: `cursor/hook-events-a8c9`.
- Do not rustfmt all of `v8host.rs`.

---

### Task 1: Prelude, types, callers, tests

- [x] `hook.events.on` / `onPre` in prelude + plugin.d.ts; delete `hook.event` and `hook.topmenu`.
- [x] Migrate plugins/examples/tools/fixtures; isolate test.
- [x] Changeset + gates.
