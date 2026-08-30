# Base-plugin publics cutover — implementation plan

> GitHub-native stacked PR. Base: `cursor/sdk-barrel-a8c9`. Branch: `cursor/plugins-publics-cutover-a8c9`.

**Goal:** Every `plugins/` and `plugins/disabled/` entry uses `OnPluginStart` + free APIs / named publics. No changeset (private plugins).

## Verify

- `bash scripts/check-plugins-typecheck.sh`
- `bash scripts/check-examples-coverage.sh` (cookbook still on subpaths)
- `bash scripts/check-antiflood-test.sh`
- `bash scripts/build-base-plugins.sh` (if the SDK CLI is built)
