import {
  mkdirSync,
  writeFileSync,
  existsSync,
  readdirSync,
  readFileSync,
} from "node:fs";
import { dirname, join, resolve, basename } from "node:path";
import { createInterface } from "node:readline/promises";
import { stdin as input, stdout as output } from "node:process";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { isPackagesDir } from "../packages-resolve.ts";
import { sharedCompilerOptionsJson } from "../tsconfig-shared.ts";

export type GameChoice = "cs2" | "none";
export type TemplateChoice = "minimal";
export type InstallChoice = "npm" | "pnpm" | "yarn" | "bun" | "none";

export interface CreateOptions {
  path?: string;
  name?: string;
  game?: GameChoice;
  template?: TemplateChoice;
  install?: InstallChoice;
  noInstall?: boolean;
  /** Skip interactive prompts; use defaults / provided flags. */
  yes?: boolean;
}

export interface CreateResult {
  dir: string;
  name: string;
  game: GameChoice;
  installed: boolean;
  skippedInstall: boolean;
  packageManager?: InstallChoice;
}

function readCliVersion(): string {
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

/** Locate monorepo packages/ when create runs from an in-tree CLI build. */
function findLocalPackagesDir(): string | undefined {
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

function defaultNameFromPath(dir: string): string {
  const base = basename(resolve(dir));
  const slug =
    base
      .replace(/[^a-zA-Z0-9._-]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .toLowerCase() || "plugin";
  return `@plugin/${slug}`;
}

async function promptSelect(
  rl: ReturnType<typeof createInterface>,
  question: string,
  choices: { value: string; label: string }[],
  defaultValue: string,
): Promise<string> {
  console.log(question);
  choices.forEach((c, i) => {
    const mark = c.value === defaultValue ? "*" : " ";
    console.log(`  ${mark} ${i + 1}) ${c.label}`);
  });
  const raw = (await rl.question(`  (default ${defaultValue}): `)).trim();
  if (!raw) return defaultValue;
  const asNum = Number(raw);
  if (Number.isInteger(asNum) && asNum >= 1 && asNum <= choices.length) {
    return choices[asNum - 1]!.value;
  }
  const hit = choices.find(
    (c) => c.value === raw || c.label.toLowerCase() === raw.toLowerCase(),
  );
  return hit?.value ?? defaultValue;
}

async function promptText(
  rl: ReturnType<typeof createInterface>,
  question: string,
  defaultValue: string,
): Promise<string> {
  const raw = (await rl.question(`${question} (${defaultValue}): `)).trim();
  return raw || defaultValue;
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
 *  build CLI (bin `s2s`); the game types are the separate `@s2script/cs2`. */
function createPackageNames(game: GameChoice): string[] {
  if (game === "cs2") {
    return ["sdk", "cs2"];
  }
  return ["sdk"];
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

function fileDevDeps(packagesDir: string, game: GameChoice): Record<string, string> | undefined {
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
    return `import { plugin } from "@s2script/sdk/plugin";
import { Chat } from "@s2script/sdk/chat";

export default plugin((ctx) => {
  ctx.commands.register("hello", (cmd) => {
    cmd.reply("hello from s2script");
    if (cmd.callerSlot >= 0) {
      Chat.toSlot(cmd.callerSlot, "hello from s2script");
    }
  });
});
`;
  }
  return `import { plugin } from "@s2script/sdk/plugin";
import { delay } from "@s2script/sdk/timers";

export default plugin((ctx) => {
  let n = 0;
  ctx.server.onGameFrame(() => {
    n += 1;
  });
  void delay(1000).then(() => console.log("s2script plugin alive; frames so far:", n));
});
`;
}

function tsconfigJson(): string {
  return (
    JSON.stringify(
      {
        compilerOptions: sharedCompilerOptionsJson,
        include: ["src", "node_modules/@s2script/sdk/globals.d.ts"],
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
  const devDependencies = fileDeps ?? registryDevDeps(game, version);
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

function gitignore(): string {
  return `node_modules/
dist/
*.s2sp
.DS_Store
`;
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
 * Scaffold a new plugin project. Interactive when stdin is a TTY and `--yes` is not set.
 *
 * When run from an in-tree CLI (monorepo packages/ present), devDependencies use
 * `file:` links so install works before the first npm publish.
 */
export async function createPlugin(opts: CreateOptions = {}): Promise<CreateResult> {
  const targetPath = resolve(opts.path ?? ".");
  const interactive = Boolean(input.isTTY && output.isTTY && !opts.yes);
  let game = assertGame(opts.game);
  let name = opts.name;
  let install = opts.noInstall ? ("none" as InstallChoice) : assertInstall(opts.install);
  const template: TemplateChoice = opts.template ?? "minimal";
  if (template !== "minimal") {
    throw new Error(`unknown template ${JSON.stringify(template)} (v1 supports: minimal)`);
  }

  if (existsSync(targetPath)) {
    const kids = readdirSync(targetPath);
    const meaningful = kids.filter((k) => k !== ".git");
    if (meaningful.length) {
      throw new Error(`target directory is not empty: ${targetPath}`);
    }
  }

  if (interactive) {
    const rl = createInterface({ input, output });
    try {
      if (!game) {
        game = (await promptSelect(
          rl,
          "Which game?",
          [
            { value: "cs2", label: "Counter-Strike 2" },
            { value: "none", label: "Engine-generic only (no game package)" },
          ],
          "cs2",
        )) as GameChoice;
      }
      if (!name) {
        name = await promptText(rl, "Plugin package name", defaultNameFromPath(targetPath));
      }
      if (!install) {
        install = (await promptSelect(
          rl,
          "Install dependencies?",
          [
            { value: "npm", label: "npm" },
            { value: "pnpm", label: "pnpm" },
            { value: "yarn", label: "yarn" },
            { value: "bun", label: "bun" },
            { value: "none", label: "skip" },
          ],
          "npm",
        )) as InstallChoice;
      }
    } finally {
      rl.close();
    }
  }

  game = game ?? "cs2";
  name = name ?? defaultNameFromPath(targetPath);
  install = install ?? (opts.noInstall ? "none" : "npm");

  const version = readCliVersion();
  const localPackagesDir = findLocalPackagesDir();

  mkdirSync(join(targetPath, "src"), { recursive: true });
  writeFileSync(
    join(targetPath, "package.json"),
    packageJsonContent(name, game, version, localPackagesDir),
  );
  writeFileSync(join(targetPath, "tsconfig.json"), tsconfigJson());
  writeFileSync(join(targetPath, "src", "plugin.ts"), pluginSource(game));
  writeFileSync(join(targetPath, ".gitignore"), gitignore());

  let installed = false;
  if (install !== "none") {
    runInstall(targetPath, install);
    installed = true;
  }

  return {
    dir: targetPath,
    name,
    game,
    installed,
    skippedInstall: install === "none",
    packageManager: install,
  };
}
