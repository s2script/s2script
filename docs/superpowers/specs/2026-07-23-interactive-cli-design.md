# Interactive `s2s` CLI — design

**Status:** Approved — ready for planning.
**Audience:** plugin authors who run `s2s` (create/build/deploy/add/login), and CLI maintainers.
**Builds on:** the current `packages/sdk/src/cli.ts` command dispatcher and the fflate bundling
pattern established when `adm-zip` was dropped (a build-time dep inlined into `dist/cli.js`).

---

## 1. Goal

Make `s2s` a modern, interactive CLI — arrow-key selects, text/confirm/password prompts, spinners,
and consistent color — **without breaking scriptability**. Every non-interactive context behaves
exactly as it does today: same exit codes, same plain stdout, no ANSI escapes.

"Non-interactive" is any one of: no TTY on stdout, `CI` env set, `--ci` flag, `-y`/`--yes` flag, or
the value in question already supplied by a flag.

## 2. Dependency

`@clack/prompts` (which pulls in `picocolors` + `sisteransi`) is added as a **devDependency** and
**bundled** into `dist/cli.js` by esbuild — removed from the `external` list in `build.mjs`, exactly
like `fflate`. Consumers therefore gain **zero new runtime dependencies**; the prompt code ships
inlined in the built CLI. `npm audit` on an installed `@s2script/sdk` sees nothing new.

## 3. The UI module — `src/ui/ui.ts`

A single thin wrapper over clack. No command touches `@clack/prompts` directly; they all go through
this module, so styling and the interactivity gate live in one place.

Surface:
- `isInteractive(flags): boolean` = `process.stdout.isTTY && !process.env.CI && !flags.ci && !flags.yes`.
- `intro(msg)`, `outro(msg)`, `note(body, title?)`, `log.{success,info,warn,error,step}(msg)`.
- `select`, `text`, `password`, `confirm` — wrappers around clack's equivalents that detect
  `clack.isCancel` (Ctrl-C) and exit cleanly with code `130` (never a stack trace).
- `task(label, fn)` — runs `await fn()` under a clack spinner when interactive; when non-interactive,
  just runs `fn()` (optionally emitting a single plain status line). Returns `fn`'s result.
- `pc` — re-exported `picocolors` for accent coloring.

**Contract:** the module never prompts when non-interactive. A command that needs a value it wasn't
given resolves it *only* by prompting in interactive mode; in non-interactive mode it uses the flag,
the existing default, or errors with today's usage — no prompt is reached.

## 4. Command flows

- **`create`** — the flagship wizard. For each option not supplied by a flag, prompt in order:
  path (`text`, cwd-relative default), game (`select`: `cs2` / `none`), package manager (`select`:
  `npm`/`pnpm`/`yarn`/`bun`/`none`, default auto-detected from the environment), then a `confirm`
  summary. `install` runs under a `task` spinner. Any flag skips its prompt; `-y` or non-TTY uses the
  same defaults `createPlugin` applies today. `template` is single-valued today, so it is skipped
  until a second template exists (no dead one-option prompt). The package `name` is **not** prompted
  — it stays derived from the path (or `--name`), as today — to keep the wizard short.
- **`login`** — styled `intro`; a masked `password` prompt for the token when `--token` isn't passed;
  the `s2s_` prefix check and save path are unchanged; success `outro`.
- **`build` / `deploy` / `add`** — the async op runs under a `task` spinner with a success/fail log;
  on error the spinner stops and the message prints, exit 1 as today. `build` still writes the plain
  `.s2sp` path to stdout in non-interactive mode.
- **`config gen` / `gen-schema|events|nav`** — styled per-item summaries when interactive; plain lines
  and the exact same exit-code behavior non-interactively. The `--check` gates keep byte-for-byte
  behavior (they run under `CI`/non-TTY, so they never see styling).
- **No-arg `s2s`** — interactive: a `select` main menu listing the commands
  (create/build/deploy/add/login/config gen/gen-schema/gen-events/gen-nav) that dispatches into the
  chosen command's flow. Non-interactive: today's usage text + exit 1 (unchanged).

## 5. Structure (Approach A — extract handlers)

Light refactor of `cli.ts`:
- Each command moves into `src/commands/<cmd>.ts` exporting a handler that takes the parsed flags and
  performs its own interactive/non-interactive resolution via the `ui` module.
- The flag parsers (`parseFlag`/`hasFlag`/`positionals`) move to a shared `src/cli/args.ts`.
- `cli.ts` shrinks to: parse argv → if no command and interactive, run the main menu → otherwise
  dispatch to the command handler.

Rejected alternative (**Approach B**): sprinkle clack calls inline into the existing 213-line
if/else. Less churn now, but it bloats an already-dense file and tangles parse + prompt + dispatch,
making the CI seam hard to test. Approach A keeps units small and the non-interactive path a
first-class, unit-testable boundary.

## 6. Invariants (the scripting contract)

1. **Non-interactive == today**, byte-for-byte on exit codes and plain stdout. clack's spinner and
   `picocolors` auto-disable ANSI off-TTY and honor `NO_COLOR`/`CI`, so piped/CI output stays plain.
2. **`build`'s plain path line stays** on stdout non-interactively. (Verified: `build-base-plugins.sh`,
   `package-release.sh`, and `check-*-generated.sh` consume the CLI via **exit codes**, not stdout
   parsing.)
3. **Ctrl-C anywhere → exit 130, clean** — no stack trace, no half-written prompt.

## 7. Testing

- Unit-test `isInteractive()` across the flag/env matrix (TTY×CI×`--ci`×`-y`).
- Unit-test each command's flag-resolver: with all flags supplied the prompt path is never taken; with
  a flag missing in non-interactive mode the behavior/error matches today. These run with no TTY.
- The full existing SDK suite stays green (build/deploy/config-gen non-interactive paths unchanged).
- Prompt UIs (actual TTY rendering) are verified manually; the resolver seam is what's unit-tested.

## 8. Out of scope (YAGNI)

Autocomplete, persistent command history, a full curses/full-screen TUI, and an interactive
config-editor UI. None are needed for "arrow keys + modern prompts" and each is a separate effort.

## 9. Success criteria

- Running `s2s create` (or bare `s2s`) in a terminal presents arrow-key prompts and a styled summary;
  the created plugin is identical to the flag-driven path.
- `s2s create <path> --game cs2 -y`, `s2s build <dir>`, and all `gen-* --check` behave exactly as
  before (exit codes + plain output), and `make ci-js` stays green.
- `dist/cli.js` has no new runtime dependency — `@clack/prompts` is bundled, not required at runtime.
