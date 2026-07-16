# `s2s create` — live per-package version resolution

- **Date:** 2026-07-16
- **Status:** Design approved; ready for implementation plan
- **Scope:** `packages/sdk` CLI (`s2s create` scaffold path) — one source file + its tests + a changeset
- **Grounded against:** `origin/main` @ `f30c4cd`

## Problem

Scaffolding a CS2 plugin on the registry path writes an **unsatisfiable dependency pin** for `@s2script/cs2`, so the subsequent install fails outright.

`packages/sdk/src/create/create.ts` computes both dev-dependency pins from a single sdk-derived version:

- `readCliVersion()` (L39–55) reads **only** `packages/sdk/package.json` → `0.1.0`.
- `registryDevDeps(game, version)` (L134–140) pins **every** package — `@s2script/sdk` *and* `@s2script/cs2` — to the same `^${version}` string.
- `runInstall` (L245–254) shells out to a bare `<pm> install` against those pre-written pins.

`@s2script/sdk` and `@s2script/cs2` version **independently** (currently sdk `0.1.0`, cs2 `0.5.0`). The registry path therefore emits `"@s2script/cs2": "^0.1.0"` for a package whose real version is `0.5.0` — and under `^0.x` semver rules `0.5.0` does not satisfy `^0.1.0`, so `npm install` fails. `@s2script/sdk: "^0.1.0"` is correct only by coincidence.

Root cause: **any value derived from the sdk's version is wrong for cs2 by construction**, because the two packages version independently. The only correct source for "the current cs2 version" is the registry, queried at scaffold time.

## Goal

Stop computing non-sdk versions in the CLI. Resolve each non-sdk package's version live from the registry at scaffold time, writing a correct concrete pin into the generated `package.json`. The sdk pin stays self-read (it is correct by construction). This must never again go stale when either package versions on its own.

## Non-goals

- The hardcoded `"0.1.0"` fallback in `readCliVersion` when the CLI cannot find its own `package.json` — separate edge, rarely hit, left as-is.
- Any change to `runInstall`, the package-manager surface (npm/pnpm/yarn/bun/none), or the install mechanism.
- Any change to the local monorepo (`file:`) path — it is already correct.

## Design

### The rule (generalized, not cs2-specific)

For each package name returned by `createPackageNames(game)`:

- **`sdk`** → `^${readCliVersion()}`. Unchanged. The running CLI *is* the `@s2script/sdk` artifact, so its own version is exactly the installable version by construction — pinning to it is the right contract (not "latest", not stale).
- **anything else** (`cs2` today, `@s2script/<game>` tomorrow) → **resolved live from the registry**, never derived from the sdk version.

Generalizing on "is this `sdk`?" rather than hardcoding cs2 means future per-game packages are covered with no further edit.

### New resolver, split for testability

A thin network seam delegating to a pure formatter:

```ts
// Network seam — thin, not directly unit-tested.
function resolvePublishedVersion(pkg: string): string {
  const r = spawnSync("npm", ["view", pkg, "version"],
                      { encoding: "utf8", timeout: 5000 });
  return versionSpecFrom(r.status, r.stdout);
}

// Pure — fully unit-testable, no network.
function versionSpecFrom(status: number | null, stdout: string): string {
  const v = (stdout ?? "").trim();
  return status === 0 && /^\d+\.\d+\.\d+/.test(v) ? `^${v}` : "latest";
}
```

Rationale:

- **`npm view`** (not a hardcoded registry URL) respects `.npmrc` and private/enterprise registries.
- **`timeout: 5000`** bounds the worst case: an offline scaffold fails fast to the `latest` fallback instead of hanging.
- **Every failure collapses to `"latest"`** — non-zero exit, empty/garbage stdout, `npm` absent from PATH, or the package unpublished (404). `latest` is a floating spec, chosen deliberately over any hardcoded number so the author's eventual install picks up the current version. It is the documented degraded fallback, not the happy path.

### `registryDevDeps` gains an injectable resolver

Default argument keeps existing call sites unchanged; tests pass a stub.

```ts
function registryDevDeps(
  game: GameChoice,
  sdkVersion: string,
  resolve: (pkg: string) => string = resolvePublishedVersion,
): Record<string, string> {
  const deps: Record<string, string> = {};
  for (const n of createPackageNames(game)) {
    deps[`@s2script/${n}`] = n === "sdk" ? `^${sdkVersion}` : resolve(`@s2script/${n}`);
  }
  return deps;
}
```

Both branches return a complete version-spec string (`^x.y.z` or `latest`), so the generated `package.json` is always complete and installable via the existing bare `<pm> install`.

The call site at `packageJsonContent` (L215) stays `registryDevDeps(game, version)` — the resolver defaults in.

### Untouched by design

- **`fileDevDeps`** (L142–150) — the local monorepo `file:` path. Already correct; it is what in-repo authoring and the existing scaffold test exercise.
- **`runInstall`** (L245–254) — stays a bare `<pm> install`. A single install mechanism across all four PMs; the `package.json` we write is already complete and correctly pinned, so there is no need for per-PM install-by-name (`npm i -D` / `pnpm add -D` / `yarn add -D` / `bun add -d`).
- **The `localPackagesDir ? fileDevDeps : registryDevDeps` branch** in `packageJsonContent` (L214–215).

### Behavior after the fix

| Scenario | `@s2script/cs2` pin written |
|---|---|
| registry path, online | `^<live cs2 version>` (e.g. `^0.5.0`) |
| registry path, `--install none`, online | `^<live cs2 version>` — still concrete and correct |
| registry path, offline / no `npm` / cs2 unpublished | `latest` |
| local monorepo path | `file:<packagesDir>/cs2` (unchanged) |
| `@s2script/sdk`, every path | `^<readCliVersion()>` (unchanged) |

Note two deliberate improvements over the original framing: resolution happens at **package.json-write time**, independent of the install choice — so a `--install none` scaffold still gets a concrete pin whenever the registry is reachable — and the `latest` fallback is reached only on genuine failure, not merely because install was skipped.

## Testing

In `packages/sdk/test/create-resolve.test.mjs` (or a focused sibling):

1. **`versionSpecFrom` (pure, no network):**
   - `(0, "0.5.0\n")` → `^0.5.0`
   - `(0, "")` → `latest`
   - `(1, "0.5.0")` → `latest`
   - `(0, "garbage")` → `latest`
2. **`registryDevDeps` with a stub resolver:**
   - `registryDevDeps("cs2", "0.1.0", () => "^0.5.0")` → `{ "@s2script/sdk": "^0.1.0", "@s2script/cs2": "^0.5.0" }`. This is the direct regression for the bug: **cs2 is not tied to the sdk version.**
   - `registryDevDeps("none", "0.1.0", stub)` → `{ "@s2script/sdk": "^0.1.0" }` only.
3. The existing `file:` scaffold test (`"create --yes scaffolds a CS2 plugin ..."`) stays green — the local path is unchanged.

The network seam (`resolvePublishedVersion`) is intentionally thin and covered only through the pure `versionSpecFrom` formatter, so the suite stays deterministic and offline-safe. (The 13 pre-existing CLI failures in `schema-runtime.test.mjs` + `player-identity.test.mjs` are unrelated to this change and remain out of scope.)

## Acceptance criteria

- Scaffolding a CS2 plugin on the registry path never writes a cs2 pin derived from the sdk version; it writes the live cs2 version (or `latest` on failure).
- `@s2script/sdk` continues to pin to the running CLI's own version.
- The local monorepo path and `runInstall` are byte-for-byte unchanged in behavior.
- New unit tests above pass; existing scaffold test stays green.

## Delivery

Ships as **PR 3 (top) of a 3-PR Graphite stack**, all branched off current `main` (`f30c4cd`). The stack grew from 2 to 3 PRs mid-execution: pruning the stale `@s2script/cli` lock entry surfaced that the consolidation had left the *whole* lockfile stale, which earned its own reconcile PR rather than being folded into the fixture cleanup.

- **PR 1 (bottom) — migrate the stranded test fixture.** The CLI-into-`@s2script/sdk` consolidation moved every test fixture into `packages/sdk/test/fixtures/` *except* `publisher-mapform-typeerror`, whose only copy was left under the otherwise-dead `packages/cli/`. That stranded a live test: `packages/sdk/test/build.test.mjs`'s "build rejects a RANGE BEFORE the typecheck gate" resolves the fixture via `join(here, "fixtures", …)` (now the sdk dir), hit `ENOENT`, and failed — a real regression, distinct from the 13 known `schema-runtime`/`player-identity` failures. PR 1 migrates the 3 fixture files into `packages/sdk/test/fixtures/` (turns that test green) and deletes the dead `packages/cli/` shell.
- **PR 2 (middle) — reconcile `package-lock.json` with the post-consolidation workspace.** The consolidation (#58/#59) never regenerated the lockfile or `node_modules`: `main`'s lockfile still carried 30+ dead pre-consolidation workspace entries (`admin`, `bans`, …, `sound`, `usercmd`) and their dangling `node_modules/@s2script/*` symlinks. PR 2 removes the dangling symlinks and regenerates the lockfile from scratch so it describes only the two real workspaces (`@s2script/sdk` + `@s2script/cs2`) — 9 insertions / 893 deletions, no transitive version churn.
- **PR 3 (top) — this version-resolution fix**: `packages/sdk/src/create/create.ts` + its test file, plus a **changeset** (patch bump for `@s2script/sdk`).

The two cleanups sit at the bottom so PR 3 lands on a green, reconciled base. Run the gate suite **per PR** (each must pass on its own); `./scripts/check-plugins-typecheck.sh` is the relevant gate, and `npm test` in `packages/sdk` must show only the 13 known failures.
