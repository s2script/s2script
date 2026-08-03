import * as build from "./build.ts";
import * as deploy from "./deploy.ts";
import * as version from "./version.ts";
import * as add from "./add.ts";
import * as install from "./install.ts";
import * as create from "./create.ts";
import * as login from "./login.ts";
import * as config from "./config.ts";
import * as codegen from "./codegen.ts";

export interface Command {
  name: string;
  summary: string;
  run: (argv: string[]) => Promise<void>;
}

/** The command registry — the dispatcher (cli.ts) and the no-arg menu both read this. */
export const COMMANDS: Command[] = [
  { name: "create", summary: "Scaffold a new plugin or workspace", run: create.run },
  { name: "build", summary: "Build a plugin (or a whole workspace) to .s2sp", run: build.run },
  { name: "deploy", summary: "Publish a plugin (or a whole workspace) to the registry", run: deploy.run },
  { name: "version", summary: "Apply pending changesets across a workspace", run: version.run },
  { name: "add", summary: "Add a registry package's types", run: add.run },
  { name: "install", summary: "Download plugins + their deps into a server", run: install.run },
  { name: "login", summary: "Save a registry deploy token", run: login.run },
  { name: "config", summary: "Emit a plugin's default config file(s)", run: config.run },
  { name: "gen-schema", summary: "Regenerate schema accessors", run: (a) => codegen.run("schema", a) },
  { name: "gen-events", summary: "Regenerate the event catalog", run: (a) => codegen.run("events", a) },
  { name: "gen-nav", summary: "Regenerate nav accessors", run: (a) => codegen.run("nav", a) },
  { name: "gen-hooks", summary: "Regenerate the ctx hook augmentation", run: (a) => codegen.run("hooks", a) },
];

/** Resolve a command by name, honoring the `publish` alias for `deploy`. */
export function find(name: string): Command | undefined {
  if (name === "publish") return COMMANDS.find((c) => c.name === "deploy");
  return COMMANDS.find((c) => c.name === name);
}
