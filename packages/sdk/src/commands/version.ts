import { resolve } from "node:path";
import { parseFlag, positionals } from "../cli/args.ts";
import { resolveVersionRoot, versionWorkspace } from "../version/version.ts";
import * as ui from "../ui/ui.ts";

/**
 * `s2s version [dir] [--since <ref>]` — apply the pending changesets across a workspace, cascading
 * bumps through `s2script.pluginDependencies` (design spec 2026-07-27 §7).
 *
 * Workspace-only by design: versioning is a statement about a SET of packages, so unlike `build`
 * and `deploy` there is no single-plugin mode to fall back to — outside a workspace this refuses
 * by name (§14's rule: a named refusal, never a silent no-op).
 */
export async function run(argv: string[]): Promise<void> {
  const interactive = ui.isInteractive();
  const [dirArg] = positionals(argv, ["--since"]);
  const from = resolve(dirArg ?? ".");

  try {
    const root = resolveVersionRoot(from);
    const result = await ui.task(
      `Versioning ${root}`,
      () => versionWorkspace(root, { sinceRef: parseFlag(argv, "--since") }),
      { interactive, done: (r) => `Versioned ${r.releases.length} package${r.releases.length === 1 ? "" : "s"}` },
    );

    if (result.changesetCount === 0) {
      // A named no-op, not a surprise: `changeset version` is equally quiet here, and a release
      // pipeline that ran this by mistake should be able to see that nothing happened.
      const msg = "No changesets found — nothing to version.";
      if (interactive) ui.outro(msg);
      else console.log(msg);
      return;
    }

    const lines: string[] = [];
    for (const r of result.releases) {
      lines.push(`${r.name} ${r.oldVersion} -> ${r.newVersion} (${r.type})`);
    }
    for (const rw of result.rewrites) {
      lines.push(`${rw.consumer} pluginDependencies[${rw.iface}] ${rw.from} -> ${rw.to}`);
    }
    // Skips are the ones that will fail the §5.2 range gate at the next build if they matter, so
    // they are printed rather than buried in the return value.
    for (const s of result.skips) {
      lines.push(`WARN: ${s.consumer} pluginDependencies[${s.iface}] = ${s.range} left unchanged — ${s.reason}`);
    }
    // Non-interactive stays plain-line on stdout: this is the scripting invariant every other
    // command holds to.
    if (interactive) {
      ui.note(lines.join("\n"), "Released");
      ui.outro(`${result.touchedFiles.length} file(s) updated`);
    } else {
      for (const line of lines) console.log(line);
    }
  } catch (e) {
    console.error(String(e instanceof Error ? e.message : e));
    process.exit(1);
  }
}
