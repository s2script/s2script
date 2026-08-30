# Nest `hook` by subject — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Nest the load-window `hook` object as `hook.client` / `hook.entity` / `hook.server` and delete the flat aliases.

**Architecture:** Prelude rebuilds the `hook` object; types in `plugin.d.ts` match; in-repo callers and isolate tests migrate. Runtime still forwards to the existing load ctx.

**Tech Stack:** `core/js/prelude.js`, `packages/sdk/plugin.d.ts`, isolate tests in `core/src/v8host.rs`.

## Global Constraints

- 0.x **minor** changeset (`@s2script/sdk`).
- No flat aliases.
- Do not rustfmt all of `v8host.rs`. Isolate tests: `cargo +stable test -p s2script-core <substring>`.
- Stacked PR base: `cursor/drop-plugin-factory-a8c9`. Branch: `cursor/hook-subjects-a8c9`.

---

### Task 1: Prelude + types + isolate tests

- Modify: `core/js/prelude.js`, `packages/sdk/plugin.d.ts`, `core/src/v8host.rs` (the three hook isolate tests only)

- [x] Nested `hook.client` / `entity` / `server`; keep `hook.event` + `hook.topmenu`.
- [x] Isolate tests call nested paths; throw copy includes the nested name.

### Task 2: In-repo callers + changeset

- Modify: `plugins/`, `examples/`, `tools/`, SDK fixtures, READMEs that show `hook.*`
- Create: `.changeset/hook-subjects.md`

- [ ] Mechanical mapping in the spec table.
- [ ] `check-plugins-typecheck.sh` + `packages/sdk && npm test` + isolate substring tests.
