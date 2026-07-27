/**
 * `s2s create --workspace <dir>` — scaffold a workspace ROOT (design spec 2026-07-27 §8): a
 * directory holding several plugins that build and publish together. This is deliberately a
 * separate function from `createPlugin` (create.ts) — a root and a plugin are different things
 * with different files — but it reuses create.ts's content generators (`eslintConfig`,
 * `gitignore`, `registryDevDeps`/`fileDevDeps`, `ESLINT_RANGE`, `readCliVersion`,
 * `slugifyBasename`) rather than duplicating them, so the two scaffolds can never drift on the
 * parts they share.
 *
 * The root's `tsconfig.base.json` is built from `sharedCompilerOptionsJson` (tsconfig-shared.ts)
 * — the SAME literal the typecheck gate enforces — never a second copy of it (spec §8's explicit
 * instruction). A workspace member's own tsconfig.json (create.ts's `workspaceTsconfigJson`)
 * then `extends` this file instead of repeating the options.
 */

import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  assertEmptyTarget,
  eslintConfig,
  fileDevDeps,
  findLocalPackagesDir,
  gitignore,
  readCliVersion,
  registryDevDeps,
  slugifyBasename,
  ESLINT_RANGE,
} from "./create.ts";
import type { GameChoice } from "./create.ts";
import { sharedCompilerOptionsJson } from "../tsconfig-shared.ts";
import { findWorkspaceRoot } from "../workspace/workspace.ts";
import * as ui from "../ui/ui.ts";

export interface CreateWorkspaceOptions {
  /** Target directory for the new workspace root. Defaults to the current directory. */
  path?: string;
  /** Root package.json `name`. Defaults to a slug of the target directory's basename. */
  name?: string;
  /** Skip interactive prompts; use defaults / provided flags. */
  yes?: boolean;
}

export interface CreateWorkspaceResult {
  dir: string;
  name: string;
}

/** The single game a fresh workspace root's devDependencies are seeded for. A workspace holds
 *  many plugins that may each pick their own game at `s2s create <name>` time (create.ts's
 *  existing `--game` prompt, unchanged) — the root just needs *a* default so the very first
 *  `npm install` has something to fetch; `cs2` matches every other scaffold default in this SDK. */
const ROOT_DEFAULT_GAME: GameChoice = "cs2";

/** Pinned to the same version the s2script repo's own root package.json carries (§3.6/§7): `s2s
 *  version` dynamically imports `@changesets/*` internals from the WORKSPACE ROOT's own
 *  node_modules, so a fresh workspace needs this installed for that command to have anything to
 *  import. */
const CHANGESETS_CLI_RANGE = "^2.29.5";

function defaultWorkspaceName(dir: string): string {
  return slugifyBasename(dir, "s2script-plugins");
}

function rootDevDependencies(version: string, localPackagesDir: string | undefined): Record<string, string> {
  const fileDeps = localPackagesDir ? fileDevDeps(localPackagesDir, ROOT_DEFAULT_GAME) : undefined;
  return {
    ...(fileDeps ?? registryDevDeps(ROOT_DEFAULT_GAME, version)),
    eslint: ESLINT_RANGE,
    "@changesets/cli": CHANGESETS_CLI_RANGE,
  };
}

function rootPackageJsonContent(
  name: string,
  version: string,
  localPackagesDir: string | undefined,
): string {
  return (
    JSON.stringify(
      {
        name,
        private: true,
        // Standard npm field for what npm models (the standing convention) — this is what makes
        // `plugins/a`, `plugins/b`, `packages/lib` symlink into a shared node_modules with NO
        // dependency declaration required for sibling resolution (spec §3.1).
        workspaces: ["plugins/*", "packages/*"],
        // The ONLY thing that makes this directory a workspace root (spec §4.1) — its presence,
        // not its contents, is what `findWorkspaceRoot` looks for.
        s2script: { workspace: { plugins: ["plugins/*"] } },
        devDependencies: rootDevDependencies(version, localPackagesDir),
      },
      null,
      2,
    ) + "\n"
  );
}

function tsconfigBaseJson(): string {
  return JSON.stringify({ compilerOptions: sharedCompilerOptionsJson }, null, 2) + "\n";
}

function changesetConfigJson(): string {
  return (
    JSON.stringify(
      {
        $schema: "https://unpkg.com/@changesets/config@3.1.1/schema.json",
        changelog: "@changesets/cli/changelog",
        commit: false,
        fixed: [],
        linked: [],
        access: "restricted",
        baseBranch: "main",
        updateInternalDependencies: "patch",
        ignore: [],
      },
      null,
      2,
    ) + "\n"
  );
}

/**
 * Scaffold a workspace root: `package.json` (both `workspaces` and `s2script.workspace`),
 * `tsconfig.base.json`, a root `eslint.config.mjs`, `.changeset/config.json`, `.gitignore`, and
 * empty `plugins/` and `packages/` directories (spec §8). Interactive when stdin is a TTY and
 * `--yes` is not set.
 */
export async function createWorkspace(
  opts: CreateWorkspaceOptions = {},
): Promise<CreateWorkspaceResult> {
  const targetPath = resolve(opts.path ?? ".");
  const interactive = ui.isInteractive({ yes: opts.yes });
  let name = opts.name;

  assertEmptyTarget(targetPath);

  // One level only (spec §14: nested workspaces are out of scope). A directory already inside
  // (or itself carrying) a workspace marker would otherwise silently produce a root
  // `findWorkspaceRoot` can never actually reach, because the ENCLOSING marker wins first — every
  // command would keep resolving to the outer workspace and never see this one.
  const enclosing = findWorkspaceRoot(targetPath);
  if (enclosing !== null) {
    throw new Error(
      `${targetPath} is already inside a workspace rooted at ${enclosing} — nested workspaces are ` +
        `not supported (one level only)`,
    );
  }

  if (interactive) {
    ui.intro("Create an s2script plugin workspace");
    if (!name) {
      name = await ui.text({
        message: "Workspace package name",
        defaultValue: defaultWorkspaceName(targetPath),
        placeholder: defaultWorkspaceName(targetPath),
      });
    }
    const proceed = await ui.confirm({
      message: `Create workspace ${name} in ${targetPath}?`,
      initialValue: true,
    });
    if (!proceed) {
      ui.outro("Cancelled.");
      process.exit(130);
    }
  }

  name = name ?? defaultWorkspaceName(targetPath);

  const version = readCliVersion();
  const localPackagesDir = findLocalPackagesDir();

  mkdirSync(join(targetPath, "plugins"), { recursive: true });
  mkdirSync(join(targetPath, "packages"), { recursive: true });
  mkdirSync(join(targetPath, ".changeset"), { recursive: true });
  writeFileSync(join(targetPath, "package.json"), rootPackageJsonContent(name, version, localPackagesDir));
  writeFileSync(join(targetPath, "tsconfig.base.json"), tsconfigBaseJson());
  writeFileSync(join(targetPath, "eslint.config.mjs"), eslintConfig());
  writeFileSync(join(targetPath, ".changeset", "config.json"), changesetConfigJson());
  writeFileSync(join(targetPath, ".gitignore"), gitignore());

  return { dir: targetPath, name };
}
