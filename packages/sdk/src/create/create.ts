import {
  mkdirSync,
  writeFileSync,
  existsSync,
  readdirSync,
  readFileSync,
  symlinkSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, basename } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { isPackagesDir } from "../packages-resolve.ts";
import { sharedCompilerOptionsJson } from "../tsconfig-shared.ts";
import * as ui from "../ui/ui.ts";
import type { PackageKind } from "../build-library.ts";

export type GameChoice = "cs2" | "none";
export type TemplateChoice = "minimal";
export type InstallChoice = "npm" | "pnpm" | "yarn" | "bun" | "none";
export type { PackageKind };

export interface CreateOptions {
  path?: string;
  name?: string;
  game?: GameChoice;
  template?: TemplateChoice;
  install?: InstallChoice;
  noInstall?: boolean;
  /**
   * What to scaffold. Absent means `"plugin"` — the same default `packageKind` (libraries.ts)
   * applies to a package.json with no `s2script.kind` at all, so a plugin scaffold needs no new
   * field to keep meaning what it always meant.
   */
  kind?: PackageKind;
  /** Skip interactive prompts; use defaults / provided flags. */
  yes?: boolean;
  /**
   * Set by the CLI (spec §8) when `s2s create <name>` runs inside a detected workspace: `path`,
   * if given, is then a bare plugin SLUG under `plugins/` — never a path relative to cwd — and the
   * scaffold is minimal (no devDependencies, no per-plugin eslint config, a tsconfig extending the
   * root's). `createPlugin` never derives this from `process.cwd()` itself: the CLI is the only
   * caller with a real ambient cwd, so every other caller (tests, library use) stays fully
   * deterministic over its explicit options.
   */
  workspaceRoot?: string;
}

export interface CreateResult {
  dir: string;
  name: string;
  game: GameChoice;
  /** Echoes opts.kind (defaulted). "library" is only reachable outside workspace mode — see
   *  createPlugin's workspace guard. */
  kind: PackageKind;
  installed: boolean;
  skippedInstall: boolean;
  packageManager?: InstallChoice;
  /** Set when this plugin was scaffolded as a workspace member — echoes opts.workspaceRoot. */
  workspaceRoot?: string;
}

/** Exported so `create/workspace.ts` can stamp the SDK's own running version into a fresh
 *  workspace root's devDependencies without re-deriving it. */
export function readCliVersion(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  const candidates = [
    join(here, "..", "package.json"), // dist/cli.js → packages/sdk/package.json
    join(here, "..", "..", "package.json"), // src/create → packages/sdk/package.json
  ];
  for (const p of candidates) {
    if (!existsSync(p)) continue;
    try {
      const pkg = JSON.parse(readFileSync(p, "utf8")) as { name?: string; version?: string };
      if (pkg.name === "@s2script/sdk" && pkg.version) return pkg.version;
    } catch {
      /* try next */
    }
  }
  return "0.1.0";
}

/** Locate monorepo packages/ when create runs from an in-tree CLI build. Exported for
 *  `create/workspace.ts`, which needs the same in-tree `file:` vs registry decision at the root. */
export function findLocalPackagesDir(): string | undefined {
  const here = dirname(fileURLToPath(import.meta.url));
  const candidates = [
    join(here, "..", ".."), // dist/cli.js → packages
    join(here, "..", "..", ".."), // src/create → packages
  ];
  for (const c of candidates) {
    if (isPackagesDir(c)) return resolve(c);
  }
  return undefined;
}

/** Sanitize a directory's basename into a filesystem/package-name-safe slug, falling back to
 *  `fallback` when that leaves nothing (root path, or a name that is all-punctuation). Shared by
 *  `defaultNameFromPath` (prefixes `@plugin/` or `@library/`) and `create/workspace.ts`'s root name default
 *  (a different fallback — a workspace root package is never itself an npm dependency). */
export function slugifyBasename(dir: string, fallback = "plugin"): string {
  const base = basename(resolve(dir));
  return (
    base
      .replace(/[^a-zA-Z0-9._-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .toLowerCase() || fallback
  );
}

function defaultNameFromPath(dir: string, kind: PackageKind = "plugin"): string {
  return `@${kind}/${slugifyBasename(dir)}`;
}

/** The "target directory is not empty" guard (spec §8): applies to the root dir in
 *  `create --workspace <dir>` and to the new `plugins/<name>` subdirectory when adding a plugin
 *  to a workspace — a `.git` already there (e.g. `git init` ran first) does not count as "not
 *  empty". Exported so `create/workspace.ts` runs the identical check on the workspace root. */
export function assertEmptyTarget(targetPath: string): void {
  if (!existsSync(targetPath)) return;
  const kids = readdirSync(targetPath);
  const meaningful = kids.filter((k) => k !== ".git");
  if (meaningful.length) {
    throw new Error(`target directory is not empty: ${targetPath}`);
  }
}

/** Inside a workspace, `s2s create <name>` names a NEW SIBLING under `plugins/` (spec §8) — `raw`
 *  must be a single path segment, never a path relative to cwd. A workspace member's location is
 *  always `plugins/<name>`, so there is nothing else a path could mean here. */
function assertBareSlug(raw: string): string {
  if (raw === "" || raw === "." || raw === ".." || isAbsolute(raw) || raw.includes("/") || raw.includes("\\")) {
    throw new Error(
      `inside a workspace, "s2s create" takes a plugin NAME, not a path (got ${JSON.stringify(raw)}) ` +
        `— it always writes plugins/<name>/`,
    );
  }
  return raw;
}

function assertGame(v: string | undefined): GameChoice | undefined {
  if (v === undefined) return undefined;
  if (v === "cs2" || v === "none") return v;
  throw new Error(`invalid --game ${JSON.stringify(v)} (expected cs2|none)`);
}

function assertInstall(v: string | undefined): InstallChoice | undefined {
  if (v === undefined) return undefined;
  if (v === "npm" || v === "pnpm" || v === "yarn" || v === "bun" || v === "none") return v;
  throw new Error(`invalid --install ${JSON.stringify(v)} (expected npm|pnpm|yarn|bun|none)`);
}

/** Direct create deps needed for a clean typecheck. Post-consolidation the builtins all ship in
 *  the single `@s2script/sdk` package (subpaths `@s2script/sdk/<cap>`), which also carries the
 *  build CLI (bin `s2s`); the game types are the separate `@s2script/cs2`. Exported so
 *  `create/workspace.ts` can compute the same set for a workspace root's devDependencies. */
export function createPackageNames(game: GameChoice): string[] {
  if (game === "cs2") {
    return ["sdk", "cs2", "eslint-plugin"];
  }
  return ["sdk", "eslint-plugin"];
}

/** Resolve a published package's current version from the registry, as a caret range.
 *  `npm view` respects .npmrc / private registries. Any failure — non-zero exit, empty or
 *  malformed output, npm absent, package unpublished — degrades to the floating `latest` spec. */
function resolvePublishedVersion(pkg: string): string {
  const r = spawnSync("npm", ["view", pkg, "version"], { encoding: "utf8", timeout: 5000 });
  return versionSpecFrom(r.status, r.stdout);
}

/** Pure formatter for a `npm view <pkg> version` result: a caret range on a clean semver,
 *  else the floating `latest`. Split out so the fallback logic is unit-testable without a network. */
export function versionSpecFrom(status: number | null, stdout: string | null): string {
  const v = (stdout ?? "").trim();
  return status === 0 && /^\d+\.\d+\.\d+/.test(v) ? `^${v}` : "latest";
}

/** Registry-path dev deps. `@s2script/sdk` pins to the running CLI's own version (the CLI *is*
 *  that artifact, so its version is installable by construction); every other package versions
 *  independently and must be resolved live. `resolve` is injectable so tests avoid the network. */
export function registryDevDeps(
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

/** Exported so `create/workspace.ts` makes the identical in-tree-vs-registry decision at the root. */
export function fileDevDeps(packagesDir: string, game: GameChoice): Record<string, string> | undefined {
  const deps: Record<string, string> = {};
  for (const n of createPackageNames(game)) {
    const abs = join(packagesDir, n);
    if (!existsSync(join(abs, "package.json"))) return undefined;
    deps[`@s2script/${n}`] = `file:${abs}`;
  }
  return deps;
}

function pluginSource(game: GameChoice): string {
  if (game === "cs2") {
    return `import { command, Chat } from "@s2script/sdk";
import type { Command } from "@s2script/sdk";

export function OnPluginStart(): void {
  command("hello", hello);
}

function hello(cmd: Command): void {
  cmd.reply("hello from s2script");
  if (cmd.callerSlot >= 0) {
    Chat.toSlot(cmd.callerSlot, "hello from s2script");
  }
}
`;
  }
  return `import { delay } from "@s2script/sdk";

let n = 0;

export function OnPluginStart(): void {
  void delay(1000).then(() => console.log("s2script plugin alive; frames so far:", n));
}

export function OnGameFrame(): void {
  n += 1;
}
`;
}

/** A library's entry point is a plain module, not a `plugin(...)`-wrapped one — build-time code
 *  has no load-scoped context to receive. Replace the body; keep exporting real, typed functions. */
const LIBRARY_ENTRY = `/** Encode a string. Replace this with the library's real surface. */
export function encode(input: string): string {
  return input;
}
`;

const LIBRARY_TYPES = `export declare function encode(input: string): string;
`;

/** Aligned with packages/sdk's own eslint dependency — bump the two together. */
export const ESLINT_RANGE = "^10.7.0";

/** A standalone plugin's own eslint.config.mjs. A workspace member writes NONE of this — one root
 *  config covers every plugin (lint.ts searches upward to the workspace root, spec §5.3 item 3) —
 *  so `create/workspace.ts` reuses this same content for the ROOT's config instead of duplicating it. */
export function eslintConfig(): string {
  return `import s2script from "@s2script/eslint-plugin";

// The SAME pinned rules \`s2s build\` enforces — the editor's ESLint extension picks this up,
// so a violation is a red squiggle before you ever build (green editor => green build).
export default s2script.configs.recommended({ tsconfigRootDir: import.meta.dirname });
`;
}

function tsconfigJson(): string {
  return (
    JSON.stringify(
      {
        compilerOptions: sharedCompilerOptionsJson,
        // `.s2script/gamedata.d.ts` is generated by `s2s build` from a plugin's own gamedata and
        // augments EngineCalls. It must be in `include` or the EDITOR sees `keyof EngineCalls` as
        // `never` — every Engine.call() shows as an error while `s2s build` passes, because the build
        // adds the file to its own program (typecheck.ts) but tsconfig never did. Harmless when the
        // plugin ships no gamedata: the path simply matches nothing.
        include: ["src", ".s2script/gamedata.d.ts", "node_modules/@s2script/sdk/globals.d.ts"],
      },
      null,
      2,
    ) + "\n"
  );
}

/** A workspace member's tsconfig.json (spec §8): extends the root's `tsconfig.base.json` instead
 *  of repeating `sharedCompilerOptionsJson` — the root's own file is already that single literal
 *  (`create/workspace.ts`'s `tsconfigBaseJson`), so a per-plugin copy would be exactly the
 *  duplication the design avoids. `include` still needs stating per-plugin: tsconfig `extends`
 *  does not merge `include`, only `compilerOptions`. */
function workspaceTsconfigJson(targetPath: string, workspaceRoot: string): string {
  const rel = relative(targetPath, join(workspaceRoot, "tsconfig.base.json")).replace(/\\/g, "/");
  return (
    JSON.stringify(
      {
        extends: rel.startsWith(".") ? rel : `./${rel}`,
        include: ["src", ".s2script/gamedata.d.ts", "node_modules/@s2script/sdk/globals.d.ts"],
      },
      null,
      2,
    ) + "\n"
  );
}

/** A workspace member's MINIMAL package.json (spec §8): no devDependencies at all — they live at
 *  the root and are hoisted to a single `node_modules` npm workspaces already share — and no
 *  eslint field, since the root's `eslint.config.mjs` covers every member (§5.3 item 3). */
function minimalPackageJsonContent(name: string): string {
  return (
    JSON.stringify(
      {
        name,
        version: "0.1.0",
        private: true,
        main: "src/plugin.ts",
        scripts: {
          build: "s2s build .",
        },
      },
      null,
      2,
    ) + "\n"
  );
}

function packageJsonContent(
  name: string,
  game: GameChoice,
  version: string,
  localPackagesDir: string | undefined,
): string {
  const fileDeps = localPackagesDir ? fileDevDeps(localPackagesDir, game) : undefined;
  const devDependencies: Record<string, string> = {
    ...(fileDeps ?? registryDevDeps(game, version)),
    eslint: ESLINT_RANGE,
  };
  return (
    JSON.stringify(
      {
        name,
        version: "0.1.0",
        private: true,
        main: "src/plugin.ts",
        scripts: {
          build: "s2s build .",
        },
        devDependencies,
      },
      null,
      2,
    ) + "\n"
  );
}

/** A standalone LIBRARY's package.json: `main`/`types` point at `src/index.ts`/`src/index.d.ts`
 *  (build-time code, no `plugin.ts` load entry) and `s2script.kind: "library"` is what
 *  `packageKind` (libraries.ts) dispatches `s2s build` on. Deliberately no
 *  `pluginDependencies`/`optionalPluginDependencies`/`publishes` field: `buildLibrary` refuses all
 *  three outright (build-time code has no ledgered runtime lifecycle to hang them on), so the
 *  scaffold never writes a field the build would immediately reject.
 *
 *  Deliberately no `private: true` either — unlike the plugin scaffold below, whose own next-step
 *  hint is `npm run build` and never touches deploy. `s2s create --library`'s own hint (see
 *  commands/create.ts's `libraryHint`) tells the author to run `s2s deploy` next, and
 *  `assertDeployable` (registry/deploy.ts) refuses `private: true` before login or build even
 *  happens — the two would flatly contradict each other. A freshly scaffolded library is also
 *  more likely to be published than a base plugin is: publishing IS the point of a library that
 *  isn't a workspace sibling (a plugin far more often stays local, `private` or not). */
function libraryPackageJsonContent(
  name: string,
  game: GameChoice,
  version: string,
  localPackagesDir: string | undefined,
): string {
  const fileDeps = localPackagesDir ? fileDevDeps(localPackagesDir, game) : undefined;
  const devDependencies: Record<string, string> = {
    ...(fileDeps ?? registryDevDeps(game, version)),
    eslint: ESLINT_RANGE,
  };
  return (
    JSON.stringify(
      {
        name,
        version: "0.1.0",
        main: "src/index.ts",
        types: "src/index.d.ts",
        scripts: {
          build: "s2s build .",
        },
        s2script: { kind: "library" },
        devDependencies,
      },
      null,
      2,
    ) + "\n"
  );
}

/** None of these patterns has a slash in the middle, so git does NOT anchor them to this
 *  directory — they match at any depth, which is exactly what a workspace root's nested
 *  `plugins/<name>/dist/` needs too. Exported for `create/workspace.ts` to reuse verbatim. */
export function gitignore(): string {
  return `node_modules/
dist/
*.s2sp
.DS_Store
`;
}

/** In-tree dev without a real install (`file:` devDeps + `--no-install`): the scaffold's own
 *  `eslint.config.mjs` is a real ESM module that Node resolves with genuine module resolution
 *  (unlike the typecheck gate's in-memory `paths` override), so it needs a real
 *  `node_modules/@s2script` to find `@s2script/eslint-plugin` even without running `npm install`.
 *  A single directory symlink to the monorepo `packages/` mirrors what an install would produce. */
function linkLocalPackagesForNoInstall(targetPath: string, localPackagesDir: string): void {
  const nm = join(targetPath, "node_modules");
  mkdirSync(nm, { recursive: true });
  const dest = join(nm, "@s2script");
  if (!existsSync(dest)) symlinkSync(localPackagesDir, dest);
}

function runInstall(dir: string, pm: InstallChoice): void {
  if (pm === "none") return;
  const r = spawnSync(pm, ["install"], {
    cwd: dir,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`${pm} install failed (exit ${r.status})`);
}

/**
 * Scaffold a new plugin OR library project (`opts.kind`, default `"plugin"`). Interactive when
 * stdin is a TTY and `--yes` is not set.
 *
 * When run from an in-tree CLI (monorepo packages/ present), devDependencies use
 * `file:` links so install works before the first npm publish.
 *
 * Workspace mode (`opts.workspaceRoot` set, spec §8): `opts.path`, if given, is a bare plugin
 * SLUG under `plugins/` rather than a directory relative to cwd, and the scaffold written is
 * minimal — no devDependencies (they live at the root), no per-plugin eslint config, a
 * tsconfig.json extending the root's `tsconfig.base.json`. This function never inspects
 * `process.cwd()` to decide it is "inside" a workspace; the CLI does that (the only caller with a
 * real ambient cwd) and passes the root down, so every other caller stays fully deterministic.
 * `kind: "library"` is refused in workspace mode (see the guard just inside this function) — a
 * workspace library needs to sit outside the `plugins/` glob, which this codepath cannot express.
 */
export async function createPlugin(opts: CreateOptions = {}): Promise<CreateResult> {
  const interactive = ui.isInteractive({ yes: opts.yes });
  let game = assertGame(opts.game);
  let name = opts.name;
  let kind: PackageKind = opts.kind ?? "plugin";
  let install = opts.noInstall ? ("none" as InstallChoice) : assertInstall(opts.install);
  const template: TemplateChoice = opts.template ?? "minimal";
  if (template !== "minimal") {
    throw new Error(`unknown template ${JSON.stringify(template)} (v1 supports: minimal)`);
  }

  const workspaceRoot = opts.workspaceRoot;
  if (kind === "library" && workspaceRoot !== undefined) {
    // A workspace member is classified plugin-vs-library STRUCTURALLY (workspace.ts: whatever
    // matches s2script.workspace.plugins is a plugin, everything else npm covers is a library) —
    // never by its own s2script.kind. createPlugin's workspace mode always writes plugins/<slug>,
    // which the default glob matches, so scaffolding a "library" there would silently misclassify
    // it as ws.plugins (see examples/workspace-library/libs/greeter for a real
    // s2script.kind:"library" + s2script.libraries workspace member and its consuming plugin).
    // Refuse rather than produce a package the workspace tooling would never treat as a library.
    throw new Error(
      `s2s create --library does not support workspace mode yet (workspace detected at ` +
        `${workspaceRoot}) — a workspace library needs to sit outside the plugins/ glob (e.g. a ` +
        `sibling directory like libs/<name>, matched by npm's own "workspaces" field but not by ` +
        `s2script.workspace.plugins — see examples/workspace-library for a worked one); add it ` +
        `by hand for now`,
    );
  }
  let slug = workspaceRoot !== undefined && opts.path !== undefined ? assertBareSlug(opts.path) : opts.path;
  // Outside a workspace, targetPath is always resolvable up front (as it always was). Inside one,
  // it depends on the slug — which interactively may not exist yet (no positional arg given).
  let targetPath: string | undefined =
    workspaceRoot === undefined
      ? resolve(opts.path ?? ".")
      : slug === undefined
        ? undefined
        : join(workspaceRoot, "plugins", slug);

  if (targetPath !== undefined) assertEmptyTarget(targetPath);

  if (interactive) {
    ui.intro("Create an s2script plugin");
    if (workspaceRoot !== undefined) {
      // "the wizard announces the detected workspace and offers to add a plugin to it" (spec §8)
      // — the announcement here, the offer is the confirm prompt below naming the workspace.
      ui.log.info(`Detected workspace at ${workspaceRoot} — adding a plugin under plugins/`);
      if (targetPath === undefined) {
        slug = assertBareSlug(
          await ui.text({ message: "Plugin name (directory under plugins/)", placeholder: "myplugin" }),
        );
        targetPath = join(workspaceRoot, "plugins", slug);
        assertEmptyTarget(targetPath);
      }
    }
    if (opts.kind === undefined && workspaceRoot === undefined) {
      // Workspace mode never offers this choice: the guard above already refuses "library" there,
      // so asking would just be a dead end.
      kind = await ui.select<PackageKind>({
        message: "What are you creating?",
        options: [
          { value: "plugin", label: "Plugin — loads on a server" },
          { value: "library", label: "Library — build-time code other plugins bundle" },
        ],
        initialValue: "plugin",
      });
    }
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
      // targetPath is always resolved by here: standalone it was set up front; in workspace mode
      // the block above either had it already or just prompted for the slug that produces it.
      const known = targetPath as string;
      name = await ui.text({
        message: kind === "library" ? "Library package name" : "Plugin package name",
        defaultValue: defaultNameFromPath(known, kind),
        placeholder: defaultNameFromPath(known, kind),
      });
    }
    if (workspaceRoot === undefined && !install) {
      // A workspace member has nothing per-plugin to install — see the forced "none" below —
      // so this prompt only makes sense standalone.
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
      message:
        workspaceRoot !== undefined
          ? `Add ${name} to the workspace at ${workspaceRoot}?`
          : kind === "library"
            ? `Create library ${name} in ${targetPath}?`
            : `Create ${name} (${game}) in ${targetPath}?`,
      initialValue: true,
    });
    if (!proceed) {
      ui.outro("Cancelled.");
      process.exit(130);
    }
  } else if (workspaceRoot !== undefined && targetPath === undefined) {
    throw new Error(
      `s2s create: a plugin name is required inside a workspace (e.g. "s2s create shop") — pass it ` +
        `as the argument or --name, or run interactively`,
    );
  }

  game = game ?? "cs2";
  name = name ?? defaultNameFromPath(targetPath as string, kind);
  // A workspace member's package.json carries no devDependencies at all (they are hoisted at the
  // root, already shared by npm workspaces) — there is nothing per-plugin to install. `npm
  // install` belongs at the workspace root, once, after this adds the new member.
  install = workspaceRoot !== undefined ? "none" : (install ?? (opts.noInstall ? "none" : "npm"));

  const finalTargetPath = targetPath as string;
  const localPackagesDir = workspaceRoot === undefined ? findLocalPackagesDir() : undefined;

  mkdirSync(join(finalTargetPath, "src"), { recursive: true });
  if (workspaceRoot !== undefined) {
    writeFileSync(join(finalTargetPath, "package.json"), minimalPackageJsonContent(name));
    writeFileSync(
      join(finalTargetPath, "tsconfig.json"),
      workspaceTsconfigJson(finalTargetPath, workspaceRoot),
    );
    // No eslint.config.mjs here: the root's own covers every member (spec §5.3 item 3).
  } else if (kind === "library") {
    const version = readCliVersion();
    writeFileSync(
      join(finalTargetPath, "package.json"),
      libraryPackageJsonContent(name, game, version, localPackagesDir),
    );
    writeFileSync(join(finalTargetPath, "tsconfig.json"), tsconfigJson());
    writeFileSync(join(finalTargetPath, "eslint.config.mjs"), eslintConfig());
  } else {
    const version = readCliVersion();
    writeFileSync(
      join(finalTargetPath, "package.json"),
      packageJsonContent(name, game, version, localPackagesDir),
    );
    writeFileSync(join(finalTargetPath, "tsconfig.json"), tsconfigJson());
    writeFileSync(join(finalTargetPath, "eslint.config.mjs"), eslintConfig());
  }
  if (kind === "library") {
    writeFileSync(join(finalTargetPath, "src", "index.ts"), LIBRARY_ENTRY);
    writeFileSync(join(finalTargetPath, "src", "index.d.ts"), LIBRARY_TYPES);
  } else {
    writeFileSync(join(finalTargetPath, "src", "plugin.ts"), pluginSource(game));
  }
  writeFileSync(join(finalTargetPath, ".gitignore"), gitignore());

  let installed = false;
  if (workspaceRoot === undefined) {
    if (install !== "none") {
      const pm = install;
      await ui.task(`Installing dependencies (${pm})`, async () => runInstall(finalTargetPath, pm), {
        interactive,
        done: () => `Installed dependencies (${pm})`,
      });
      installed = true;
    } else if (localPackagesDir) {
      linkLocalPackagesForNoInstall(finalTargetPath, localPackagesDir);
    }
  }

  return {
    dir: finalTargetPath,
    name,
    game,
    kind,
    installed,
    skippedInstall: install === "none",
    packageManager: install,
    workspaceRoot,
  };
}
