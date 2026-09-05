import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");

test("published hudkit example and previous-renderer delegation typecheck", () => {
  const docs = readFileSync(join(root, "packages/cs2/ui.d.ts"), "utf8");
  const example = docs.slice(docs.lastIndexOf(" * @example"))
    .split("\n").slice(1).filter(line => line.startsWith(" *"))
    .filter(line => line !== " */").map(line => line.replace(/^ \* ?/, "")).join("\n");
  const source = example + `
import { Menu, type MenuRenderer } from "@s2script/sdk/menu";
import type { Dashboard, HudKitPlayer, MotdHandle } from "@s2script/cs2";
declare const renderer: MenuRenderer;
const previous = Menu.registerRenderer("center", renderer);
const expected: MenuRenderer | undefined = previous;
if (expected) {
  const replacement: MenuRenderer = {
    open(session) { expected.open(session); },
    update(session) { expected.update(session); },
    close(slot) { expected.close(slot); },
  };
  Menu.registerRenderer("center", replacement);
}
// Called by the host after initialization; these handles are nonnullable.
export function OnMapStart(): void {
  const player: HudKitPlayer = hudkit.forSlot(1);
  const dashboard: Dashboard = hudkit.dashboard({ title: "Dashboard", tabs: [], rows: () => [] });
  const motd: MotdHandle = player.motd({ title: "Rules" });
  dashboard.close(1);
  motd.close();
}
`;
  const filename = join(root, "hud-api-contract-example.ts");
  const configFile = ts.readConfigFile(join(root, "tsconfig.base.json"), ts.sys.readFile);
  const config = ts.parseJsonConfigFileContent(configFile.config, ts.sys, root);
  const host = ts.createCompilerHost(config.options);
  const getSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (name, languageVersion, ...args) => name === filename
    ? ts.createSourceFile(name, source, languageVersion, true)
    : getSourceFile(name, languageVersion, ...args);
  const program = ts.createProgram([filename, join(root, "packages/sdk/globals.d.ts")], config.options, host);
  const diagnostics = ts.getPreEmitDiagnostics(program);
  assert.equal(diagnostics.length, 0, ts.formatDiagnostics(diagnostics, {
    getCurrentDirectory: () => root,
    getCanonicalFileName: name => name,
    getNewLine: () => "\n",
  }));
});
