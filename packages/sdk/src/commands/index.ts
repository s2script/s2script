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

/** The command registry — the dispatcher (cli.ts) and the no-arg menu both read this. */
export const COMMANDS: Command[] = [
  { name: "create", summary: "Scaffold a new plugin", run: create.run },
  { name: "build", summary: "Build a plugin to a .s2sp", run: build.run },
  { name: "deploy", summary: "Publish a plugin to the registry", run: deploy.run },
  { name: "add", summary: "Add a registry package's types", run: add.run },
  { name: "login", summary: "Save a registry deploy token", run: login.run },
  { name: "config", summary: "Emit a plugin's default config file(s)", run: config.run },
  { name: "gen-schema", summary: "Regenerate schema accessors", run: (a) => codegen.run("schema", a) },
  { name: "gen-events", summary: "Regenerate the event catalog", run: (a) => codegen.run("events", a) },
  { name: "gen-nav", summary: "Regenerate nav accessors", run: (a) => codegen.run("nav", a) },
];

/** Resolve a command by name, honoring the `publish` alias for `deploy`. */
export function find(name: string): Command | undefined {
  if (name === "publish") return COMMANDS.find((c) => c.name === "deploy");
  return COMMANDS.find((c) => c.name === name);
}
