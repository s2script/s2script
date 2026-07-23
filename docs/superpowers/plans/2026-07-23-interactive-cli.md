# Interactive `s2s` CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `s2s` CLI interactive — arrow-key prompts, a no-arg command menu, spinners, and styled output — while every non-interactive path stays byte-for-byte as it is today.

**Architecture:** A single `src/ui/ui.ts` wraps `@clack/prompts` and owns the interactivity gate; no command touches clack directly. The existing hand-rolled readline prompts in `create.ts` and `login.ts` are swapped for clack via that module. Each command becomes a callable handler in `src/commands/` so the no-arg main menu can dispatch into it. `cli.ts` shrinks to parse → (no command & interactive ? menu : dispatch).

**Tech Stack:** TypeScript (Node ESM, run via esbuild bundle + `node --experimental-strip-types` for tests), `@clack/prompts` (bundled into `dist/cli.js`, not a runtime dep).

## Global Constraints

- **Zero new runtime deps.** `@clack/prompts` is a **devDependency**, bundled into `dist/cli.js` by esbuild (it is NOT in `build.mjs`'s `external` list, so it is inlined automatically — `build.mjs` needs no change). Verified: it bundles to ~46KB.
- **Non-interactive == today.** Interactive iff `process.stdout.isTTY && !process.env.CI && !flags.ci && !flags.yes`. When non-interactive: no prompts, plain stdout, identical exit codes. `build` still prints the plain `.s2sp` path to stdout.
- **Ctrl-C → exit 130**, cleanly (clack returns a cancel symbol; the ui layer catches it).
- **No new npm dep beyond `@clack/prompts`** — do not add `picocolors` etc.; use clack's own styling.
- **Keep the full existing SDK suite green** (`build`/`create`/`config-gen`/`publish-scan`/`compiled-against` tests cover the non-interactive paths).
- Naming: PascalCase types, camelCase functions (repo convention).

---

### Task 1: The `ui` module + interactivity gate

**Files:**
- Create: `packages/sdk/src/ui/ui.ts`
- Test: `packages/sdk/test/ui.test.mjs`

**Interfaces:**
- Produces:
  - `isInteractive(flags?: { ci?: boolean; yes?: boolean }): boolean`
  - `intro(msg): void`, `outro(msg): void`, `note(body, title?): void`
  - `log: { info, success, warn, error, step, message }` (each `(msg: string) => void`)
  - `select<T extends string>({ message, options: {value:T,label,hint?}[], initialValue? }): Promise<T>`
  - `text({ message, placeholder?, defaultValue?, initialValue?, validate? }): Promise<string>`
  - `password({ message, validate? }): Promise<string>`
  - `confirm({ message, initialValue? }): Promise<boolean>`
  - `task<T>(label, fn: () => Promise<T>, opts: { interactive: boolean; done?: (r:T)=>string }): Promise<T>`

- [ ] **Step 1: Write the failing test** — `packages/sdk/test/ui.test.mjs`

```js
import { test } from "node:test";
import assert from "node:assert";
import { isInteractive } from "../src/ui/ui.ts";

function withEnv({ tty, ci }, fn) {
  const origTTY = Object.getOwnPropertyDescriptor(process.stdout, "isTTY");
  const origCI = process.env.CI;
  try {
    Object.defineProperty(process.stdout, "isTTY", { value: tty, configurable: true });
    if (ci === undefined) delete process.env.CI; else process.env.CI = ci;
    fn();
  } finally {
    if (origTTY) Object.defineProperty(process.stdout, "isTTY", origTTY);
    else delete process.stdout.isTTY;
    if (origCI === undefined) delete process.env.CI; else process.env.CI = origCI;
  }
}

test("isInteractive: true only on a TTY, outside CI, with no --ci/-y", () => {
  withEnv({ tty: true, ci: undefined }, () => {
    assert.equal(isInteractive(), true);
    assert.equal(isInteractive({}), true);
    assert.equal(isInteractive({ yes: true }), false, "-y forces non-interactive");
    assert.equal(isInteractive({ ci: true }), false, "--ci forces non-interactive");
  });
  withEnv({ tty: true, ci: "1" }, () => {
    assert.equal(isInteractive(), false, "CI env forces non-interactive");
  });
  withEnv({ tty: false, ci: undefined }, () => {
    assert.equal(isInteractive(), false, "no TTY -> non-interactive");
  });
});
```

- [ ] **Step 2: Run it, verify it fails** — `Run: cd packages/sdk && node --experimental-strip-types --no-warnings --test test/ui.test.mjs` → FAIL (`Cannot find module ../src/ui/ui.ts`).

- [ ] **Step 3: Implement `src/ui/ui.ts`**

```ts
// The single styling + interactivity layer over @clack/prompts. No command imports clack directly,
// so the gate and the look live in one place. clack is bundled into dist/cli.js (not a runtime dep).
import * as clack from "@clack/prompts";

export interface InteractivityFlags {
  ci?: boolean;
  yes?: boolean;
}

/** Interactive iff stdout is a TTY, we are not in CI, and neither --ci nor -y was passed. */
export function isInteractive(flags: InteractivityFlags = {}): boolean {
  return Boolean(process.stdout.isTTY) && !process.env.CI && !flags.ci && !flags.yes;
}

/** Ctrl-C from any prompt returns a cancel symbol → exit cleanly, never a stack trace. */
function guard<T>(value: T | symbol): T {
  if (clack.isCancel(value)) {
    clack.cancel("Cancelled.");
    process.exit(130);
  }
  return value as T;
}

export const intro = (msg: string): void => clack.intro(msg);
export const outro = (msg: string): void => clack.outro(msg);
export const note = (body: string, title?: string): void => clack.note(body, title);
export const log = {
  info: (m: string): void => clack.log.info(m),
  success: (m: string): void => clack.log.success(m),
  warn: (m: string): void => clack.log.warn(m),
  error: (m: string): void => clack.log.error(m),
  step: (m: string): void => clack.log.step(m),
  message: (m: string): void => clack.log.message(m),
};

export async function select<T extends string>(opts: {
  message: string;
  options: { value: T; label: string; hint?: string }[];
  initialValue?: T;
}): Promise<T> {
  return guard(await clack.select(opts)) as T;
}

export async function text(opts: {
  message: string;
  placeholder?: string;
  defaultValue?: string;
  initialValue?: string;
  validate?: (v: string) => string | undefined;
}): Promise<string> {
  return guard(await clack.text(opts));
}

export async function password(opts: {
  message: string;
  validate?: (v: string) => string | undefined;
}): Promise<string> {
  return guard(await clack.password(opts));
}

export async function confirm(opts: { message: string; initialValue?: boolean }): Promise<boolean> {
  return guard(await clack.confirm(opts));
}

/** Run an async op under a spinner when interactive; otherwise just run it. Returns fn's result. */
export async function task<T>(
  label: string,
  fn: () => Promise<T>,
  opts: { interactive: boolean; done?: (r: T) => string },
): Promise<T> {
  if (!opts.interactive) return fn();
  const s = clack.spinner();
  s.start(label);
  try {
    const r = await fn();
    s.stop(opts.done ? opts.done(r) : label);
    return r;
  } catch (e) {
    s.stop(`${label} — failed`, 1);
    throw e;
  }
}
```

- [ ] **Step 4: Run the test + verify clack bundles**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/ui.test.mjs` → PASS.
Run: `cd packages/sdk && node build.mjs` → prints `built dist/cli.js` (proves `@clack/prompts` bundles via the existing config).

> If importing `@clack/prompts` at module top makes the test error (import-time side effects), fall back to lazy `await import("@clack/prompts")` **inside** each prompt/`task` function and keep `isInteractive` clack-free. `isInteractive`/`log` shape stays identical.

- [ ] **Step 5: Commit** — `git add packages/sdk/src/ui/ui.ts packages/sdk/test/ui.test.mjs && git commit -m "sdk(cli): add the clack-backed ui module + interactivity gate"`

---

### Task 2: Extract flag parsing to `src/cli/args.ts`

**Files:**
- Create: `packages/sdk/src/cli/args.ts`
- Modify: `packages/sdk/src/cli.ts` (import from args.ts instead of local defs)
- Test: `packages/sdk/test/args.test.mjs`

**Interfaces:**
- Produces: `parseFlag(args: string[], name: string): string | undefined`, `hasFlag(args: string[], name: string): boolean`, `positionals(args: string[], flagsWithValue: string[]): string[]`

- [ ] **Step 1: Write the failing test** — `packages/sdk/test/args.test.mjs`

```js
import { test } from "node:test";
import assert from "node:assert";
import { parseFlag, hasFlag, positionals } from "../src/cli/args.ts";

test("parseFlag: --k v and --k=v forms; missing -> undefined", () => {
  assert.equal(parseFlag(["--game", "cs2"], "--game"), "cs2");
  assert.equal(parseFlag(["--game=cs2"], "--game"), "cs2");
  assert.equal(parseFlag(["build"], "--game"), undefined);
  assert.equal(parseFlag(["--game", "--next"], "--game"), undefined, "next flag is not the value");
});

test("hasFlag", () => {
  assert.equal(hasFlag(["--ci"], "--ci"), true);
  assert.equal(hasFlag(["deploy"], "--ci"), false);
});

test("positionals: skips flags and the value after a value-flag", () => {
  assert.deepEqual(positionals(["deploy", "dir", "--registry", "u"], ["--registry"]), ["deploy", "dir"]);
  assert.deepEqual(positionals(["a", "--registry=u", "b"], ["--registry"]), ["a", "b"]);
  assert.deepEqual(positionals(["-y", "x"], []), ["x"]);
});
```

- [ ] **Step 2: Run it, verify it fails** — `Run: cd packages/sdk && node --experimental-strip-types --no-warnings --test test/args.test.mjs` → FAIL (module missing).

- [ ] **Step 3: Create `src/cli/args.ts`** — move the three functions **verbatim** from `cli.ts` (lines 23–50 today): `parseFlag`, `hasFlag`, `positionals`. Export each.

- [ ] **Step 4: Update `cli.ts`** — delete the three local definitions; add `import { parseFlag, hasFlag, positionals } from "./cli/args.ts";`

- [ ] **Step 5: Run tests** — `Run: cd packages/sdk && npm test` → all pass (args.test new; the rest unchanged).

- [ ] **Step 6: Commit** — `git add packages/sdk/src/cli/args.ts packages/sdk/src/cli.ts packages/sdk/test/args.test.mjs && git commit -m "sdk(cli): extract flag parsing to cli/args.ts"`

---

### Task 3: `create` wizard — swap readline for clack

**Files:**
- Modify: `packages/sdk/src/create/create.ts` (interactive block, lines ~82–348)

**Interfaces:**
- Consumes: `ui.isInteractive`, `ui.select`, `ui.text`, `ui.confirm`, `ui.task`, `ui.intro`, `ui.outro`, `ui.log` (Task 1).
- Produces: `createPlugin` keeps its exact signature (`CreateOptions → Promise<CreateResult>`) and non-interactive behavior.

- [ ] **Step 1: Replace the interactivity gate and prompts.** In `create.ts`:
  - Add `import * as ui from "../ui/ui.ts";`
  - Remove the `createInterface`/`stdin`/`stdout`/readline imports and the `promptSelect`/`promptText` helper functions (they become dead).
  - Change `const interactive = Boolean(input.isTTY && output.isTTY && !opts.yes);` → `const interactive = ui.isInteractive({ yes: opts.yes });`
  - Replace the `if (interactive) { const rl = ... }` block with clack prompts (no `rl`):

```ts
  if (interactive) {
    ui.intro("Create an s2script plugin");
    if (!game) {
      game = await ui.select<GameChoice>({
        message: "Which game?",
        options: [
          { value: "cs2", label: "Counter-Strike 2" },
          { value: "none", label: "Engine-generic only (no game package)" },
        ],
        initialValue: "cs2",
      });
    }
    if (!name) {
      name = await ui.text({
        message: "Plugin package name",
        defaultValue: defaultNameFromPath(targetPath),
        placeholder: defaultNameFromPath(targetPath),
      });
    }
    if (!install) {
      install = await ui.select<InstallChoice>({
        message: "Install dependencies?",
        options: [
          { value: "npm", label: "npm" },
          { value: "pnpm", label: "pnpm" },
          { value: "yarn", label: "yarn" },
          { value: "bun", label: "bun" },
          { value: "none", label: "skip" },
        ],
        initialValue: "npm",
      });
    }
    const proceed = await ui.confirm({
      message: `Create ${name} (${game}) in ${targetPath}?`,
      initialValue: true,
    });
    if (!proceed) { ui.outro("Cancelled."); process.exit(130); }
  }
```

- [ ] **Step 2: Put install under a spinner.** Replace the `if (install !== "none") { runInstall(...); installed = true; }` branch so that in interactive mode it runs through `ui.task`:

```ts
  let installed = false;
  if (install !== "none") {
    await ui.task(`Installing dependencies (${install})`, async () => runInstall(targetPath, install), {
      interactive,
      done: () => `Installed dependencies (${install})`,
    });
    installed = true;
  } else if (localPackagesDir) {
    linkLocalPackagesForNoInstall(targetPath, localPackagesDir);
  }
```

> Note: `runInstall` uses `stdio: "inherit"`; leave that. The spinner wraps the call — the child's own output still shows. Acceptable for v1.

- [ ] **Step 3: Run the create tests** — `Run: cd packages/sdk && node --experimental-strip-types --no-warnings --test test/create-resolve.test.mjs` → PASS (these exercise the pure `registryDevDeps`/`versionSpecFrom`, unaffected).

- [ ] **Step 4: Manual TTY check** — `Run: node packages/sdk/dist/cli.js create /tmp/s2s-wiz-demo` (after `node packages/sdk/build.mjs`); arrow-key through game / name / install / confirm; then `rm -rf /tmp/s2s-wiz-demo`. And confirm non-interactive is unchanged: `node packages/sdk/dist/cli.js create /tmp/s2s-y-demo --game cs2 -y --no-install && rm -rf /tmp/s2s-y-demo`.

- [ ] **Step 5: Commit** — `git add packages/sdk/src/create/create.ts && git commit -m "sdk(cli): create wizard uses arrow-key clack prompts"`

---

### Task 4: `login` — masked clack password prompt

**Files:**
- Modify: `packages/sdk/src/registry/login.ts`

**Interfaces:**
- Consumes: `ui.isInteractive`, `ui.intro`, `ui.password`, `ui.outro` (Task 1).
- Produces: `loginInteractive` keeps its signature and CI behavior (throws when no token + non-interactive).

- [ ] **Step 1: Swap readline for clack.** In `login.ts`:
  - Remove the `createInterface`/`stdin`/`stdout` readline imports; add `import * as ui from "../ui/ui.ts";`
  - Replace the interactive branch (`if (opts?.ci || !input.isTTY) throw …; const rl = …`) with:

```ts
  if (!token) {
    if (!ui.isInteractive({ ci: opts?.ci })) {
      throw new Error("no deploy token: set S2SCRIPT_TOKEN or run `s2s login` interactively");
    }
    ui.intro("s2script login");
    ui.log.info(`Registry: ${registryUrl}`);
    ui.log.info(`Sign in (or create an account), then mint a deploy token at:\n  ${registryUrl}/account/tokens`);
    token = (
      await ui.password({
        message: "Paste your deploy token",
        validate: (v) => (v.trim().startsWith("s2s_") ? undefined : 'token must start with "s2s_"'),
      })
    ).trim();
  }
```
  - Keep the existing post-prompt `if (!token || !token.startsWith("s2s_")) throw …` and `saveCredentials`.

- [ ] **Step 2: Run the suite** — `Run: cd packages/sdk && npm test` → all pass (login has no unit test; nothing regresses).

- [ ] **Step 3: Manual check (non-interactive)** — `Run: printf '' | node packages/sdk/dist/cli.js login --ci 2>&1` → prints the "no deploy token" error, exit 1 (unchanged CI behavior).

- [ ] **Step 4: Commit** — `git add packages/sdk/src/registry/login.ts && git commit -m "sdk(cli): login uses a masked clack password prompt"`

---

### Task 5: Command handlers + spinners + styled output

**Files:**
- Create: `packages/sdk/src/commands/build.ts`, `deploy.ts`, `add.ts`, `create.ts`, `login.ts`, `config.ts`, `codegen.ts`
- Modify: `packages/sdk/src/cli.ts` (dispatch to handlers)

**Interfaces:**
- Each handler exports `export async function run(argv: string[]): Promise<void>` where `argv` is the args **after** the command word. Each parses its own flags (via `cli/args.ts`), resolves interactive vs not (via `ui`), does the work, prints results, and `process.exit(1)` on error (matching today).
- Produces: a `COMMANDS` registry `{ name, summary, run }[]` (in `src/commands/index.ts`) that Task 6's menu consumes.

- [ ] **Step 1: Create `src/commands/index.ts`** — the registry the menu and dispatcher share:

```ts
import * as build from "./build.ts";
import * as deploy from "./deploy.ts";
import * as add from "./add.ts";
import * as create from "./create.ts";
import * as login from "./login.ts";
import * as config from "./config.ts";
import * as codegen from "./codegen.ts";

export interface Command {
  name: string;
  summary: string;
  run: (argv: string[]) => Promise<void>;
}

export const COMMANDS: Command[] = [
  { name: "create", summary: "Scaffold a new plugin", run: create.run },
  { name: "build", summary: "Build a plugin to a .s2sp", run: build.run },
  { name: "deploy", summary: "Publish a plugin to the registry", run: deploy.run },
  { name: "add", summary: "Add a registry package's types", run: add.run },
  { name: "login", summary: "Save a registry deploy token", run: login.run },
  { name: "config", summary: "config gen — emit default config files", run: config.run },
  { name: "gen-schema", summary: "Regenerate schema accessors", run: (a) => codegen.run("schema", a) },
  { name: "gen-events", summary: "Regenerate the event catalog", run: (a) => codegen.run("events", a) },
  { name: "gen-nav", summary: "Regenerate nav accessors", run: (a) => codegen.run("nav", a) },
];

export function find(name: string): Command | undefined {
  if (name === "publish") return COMMANDS.find((c) => c.name === "deploy");
  return COMMANDS.find((c) => c.name === name);
}
```

- [ ] **Step 2: Move each command's body out of `cli.ts` into its handler**, verbatim logic, wrapping long ops in `ui.task` when interactive. Example — `src/commands/build.ts`:

```ts
import { resolve } from "node:path";
import { buildPlugin } from "../build.ts";
import { resolvePackagesDir } from "../packages-resolve.ts";
import { parseFlag } from "../cli/args.ts";
import * as ui from "../ui/ui.ts";

export async function run(argv: string[]): Promise<void> {
  const dir = argv.find((a) => !a.startsWith("-"));
  if (!dir) { console.error("Usage: s2s build <dir> [--packages-dir <path>]"); process.exit(1); }
  const interactive = ui.isInteractive();
  try {
    const packagesDir = resolvePackagesDir({
      explicit: parseFlag(argv, "--packages-dir"),
      pluginDir: resolve(dir),
      fromCliUrl: import.meta.url,
    });
    const out = await ui.task(`Building ${dir}`, () => buildPlugin(dir, packagesDir), {
      interactive,
      done: (p) => `Built ${p}`,
    });
    // Non-interactive: keep the machine-readable plain path on stdout (invariant #2).
    if (!interactive) console.log(out);
    else ui.outro(out);
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}
```

  Apply the same shape to `deploy.ts`, `add.ts` (spinner around `deployPlugin`/`addPackage`, styled success via `ui.log.success` interactively, the exact same `console.log` lines non-interactively so nothing scripted changes). `config.ts` wraps `runConfigGen`; `codegen.ts` exports `run(kind: "schema"|"events"|"nav", argv)` wrapping the three `runGen*` with the current `--check` output + exit codes UNCHANGED. `create.ts`/`login.ts` handlers just parse flags and call `createPlugin`/`loginInteractive` (the prompting already lives in those, Tasks 3–4).

- [ ] **Step 3: Verify each command's non-interactive output is unchanged** — diff the `console.log`/`console.error` strings against today's `cli.ts` for `build`, `deploy`, `add`, `config gen`, `gen-*`. The `gen-* --check` branches must keep identical messages + `process.exit(1)` on drift.

- [ ] **Step 4: Run the full suite + gate** — `Run: cd packages/sdk && npm test` → all pass. `Run: cd /home/gkh/projects/s2script && node packages/sdk/build.mjs && CI=1 make ci-js` → green (proves gen-*/build via the CLI still behave in CI).

- [ ] **Step 5: Commit** — `git add packages/sdk/src/commands packages/sdk/src/cli.ts && git commit -m "sdk(cli): extract command handlers with spinners + styled output"`

---

### Task 6: No-arg main menu + final `cli.ts`

**Files:**
- Create: `packages/sdk/src/commands/menu.ts`
- Modify: `packages/sdk/src/cli.ts` (final shape)

**Interfaces:**
- Consumes: `COMMANDS`/`find` (Task 5), `ui.isInteractive`/`ui.intro`/`ui.select` (Task 1).

- [ ] **Step 1: Create `src/commands/menu.ts`**

```ts
import * as ui from "../ui/ui.ts";
import { COMMANDS } from "./index.ts";

export async function run(): Promise<void> {
  ui.intro("s2script");
  const name = await ui.select({
    message: "What would you like to do?",
    options: COMMANDS.map((c) => ({ value: c.name, label: c.name, hint: c.summary })),
  });
  const cmd = COMMANDS.find((c) => c.name === name)!;
  await cmd.run([]); // the handler prompts for whatever it needs
}
```

- [ ] **Step 2: Rewrite `cli.ts` to the final dispatcher**

```ts
import { COMMANDS, find } from "./commands/index.ts";
import * as menu from "./commands/menu.ts";
import * as ui from "./ui/ui.ts";

const argv = process.argv.slice(2);
const command = argv[0];

function usage(): void {
  console.error(
    "Usage:\n" +
      COMMANDS.map((c) => `  s2s ${c.name} — ${c.summary}`).join("\n") +
      "\n\nEnv: S2SCRIPT_REGISTRY_URL  S2SCRIPT_TOKEN (CI deploy)",
  );
}

if (!command) {
  if (ui.isInteractive()) { await menu.run(); }
  else { usage(); process.exit(1); }
} else {
  const cmd = find(command);
  if (!cmd) { usage(); process.exit(1); }
  await cmd.run(argv.slice(1));
}
```

- [ ] **Step 3: Verify non-interactive no-arg is unchanged** — `Run: node packages/sdk/dist/cli.js < /dev/null 2>&1 | head` (after build) → prints usage, exit 1. `Run: node packages/sdk/dist/cli.js bogus 2>&1` → usage, exit 1.

- [ ] **Step 4: Manual TTY check** — `Run: node packages/sdk/dist/cli.js` in a terminal → arrow-key menu appears; selecting `create` enters the wizard.

- [ ] **Step 5: Commit** — `git add packages/sdk/src/cli.ts packages/sdk/src/commands/menu.ts && git commit -m "sdk(cli): no-arg interactive main menu + slim dispatcher"`

---

### Task 7: Final gate + changeset

**Files:**
- Create: `.changeset/sdk-interactive-cli.md`

- [ ] **Step 1: Rebuild + full suite** — `Run: cd /home/gkh/projects/s2script && node packages/sdk/build.mjs && cd packages/sdk && npm test` → `built dist/cli.js`, all tests pass.
- [ ] **Step 2: Full JS gate** — `Run: cd /home/gkh/projects/s2script && CI=1 make ci-js` → `ci-js: all JS gates passed`.
- [ ] **Step 3: Confirm zero new runtime dep** — `Run: node -e "const p=require('./packages/sdk/package.json'); console.log('deps', Object.keys(p.dependencies)); console.log('clack in dev', !!p.devDependencies['@clack/prompts'])"` → clack is only in devDependencies; `dependencies` has no clack.
- [ ] **Step 4: Add changeset** — `.changeset/sdk-interactive-cli.md`:

```markdown
---
"@s2script/sdk": minor
---

The `s2s` CLI is now interactive: arrow-key prompts for `create` and `login`, a no-arg command menu, and spinners on `build`/`deploy`/`add`, with consistent styling. Every non-interactive path (flags, `-y`, `--ci`, no TTY, CI) behaves exactly as before — same output and exit codes. Powered by `@clack/prompts`, bundled into the CLI, so there is no new runtime dependency.
```

- [ ] **Step 5: Commit** — `git add .changeset/sdk-interactive-cli.md && git commit -m "sdk(cli): changeset for the interactive CLI (minor)"`

---

## Notes

- **Spec doc:** `docs/superpowers/specs/2026-07-23-interactive-cli-design.md` — commit it alongside Task 1 (`git add` it in the first commit).
- **Branch:** work is on `sdk/interactive-cli`, based on the `sdk/fflate-and-tests` (PR #6) branch so it has the login refresh + bundling pattern. After #6 merges, `git rebase origin/main` so the eventual PR contains only the interactive-CLI commits (one PR per slice).
- **Deviation from spec:** no `picocolors` — clack's own `log`/`note`/`spinner` styling is enough, and it keeps the dep surface minimal (aligned with the just-completed adm-zip removal).
