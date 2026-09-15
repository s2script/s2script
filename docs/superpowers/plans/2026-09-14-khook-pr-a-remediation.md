# KHook PR A remediation implementation plan

This earlier work-package plan is superseded by the [current dynamic implementation
plan](2026-09-15-khook-pr-a-review-fixes.md) and [current remediation
spec](../specs/2026-09-14-khook-pr-a-remediation-design.md).

The user confirmed stock Metamod and only `.s2sp` hot reload inside resident
s2script. The previous host patch/build tasks and native shim reload acceptance
were outside that requirement and must not be dispatched. Their historical review
evidence remains in Git history; independent lifetime, observer, artifact and
fixture corrections are carried into the current plan.

Coordinators and subagents must use the current plan's file ownership, dependency
barriers and evidence rules. PR #221 remains draft until the amended stock-host
acceptance passes. This handoff does not authorize PR B, merge or release.
