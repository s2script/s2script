import { readFileSync } from "node:fs";
import { resolve, sep, basename } from "node:path";
import { stripJsonComments } from "./jsonc.ts";
import { validatePluginGamedata } from "./validate.ts";
import type { PluginGamedata } from "./types.ts";

/** Shared validation for build and non-emitting standalone typecheck. */
export function loadPluginGamedata(absDir: string, pkg: {
  name: string; s2script?: { gamedata?: string; permissions?: string[] };
}): PluginGamedata | undefined {
  const s2 = pkg.s2script ?? {};
  if (typeof s2.gamedata !== "string") return undefined;
  const permissions = Array.isArray(s2.permissions) ? s2.permissions : [];
  let gamedata: PluginGamedata;
  const gdPath = resolve(absDir, s2.gamedata);
  // Containment: the gamedata must live inside the plugin directory. `s2script.gamedata` is an
  // author-supplied path, and a build must never read (and then PACK) a file from outside the
  // package — the same class of thing commit 7f45a70 hardened for install.
  const dirPrefix = absDir.endsWith(sep) ? absDir : absDir + sep;
  if (gdPath !== absDir && !gdPath.startsWith(dirPrefix)) {
    throw new Error(
      `s2script.gamedata escapes the plugin directory: ${JSON.stringify(s2.gamedata)} resolves to ${gdPath}`
    );
  }
  // Naming convention: `<plugin-name-without-scope>.gamedata.jsonc` — a plugin's one gamedata file
  // is named for its OWNER (the plugin), the same way `gamedata/core/` and `gamedata/cs2/` are
  // owner-named DIRECTORIES in the framework's own tree. (The files inside those directories are
  // named for their TARGET instead — common/engine.<engine>/game.<mod> — a distinction a plugin's
  // single file has no need of, since a plugin has exactly one owner and no target axis.)
  // Enforced rather than documented so the convention actually holds: `@me/burn` -> `burn.gamedata.jsonc`.
  const unscoped = pkg.name.includes("/") ? pkg.name.slice(pkg.name.lastIndexOf("/") + 1) : pkg.name;
  const actualBase = basename(gdPath);
  const allowedBases = [`${unscoped}.gamedata.jsonc`, `${unscoped}.gamedata.json`];
  if (!allowedBases.includes(actualBase)) {
    throw new Error(
      `s2script.gamedata must be named '${unscoped}.gamedata.jsonc' (after the plugin name without ` +
        `its scope — a plugin's gamedata is named for its owner, the plugin itself), but is '${actualBase}'`
    );
  }
  // Full JSONC: trailing `// …` and `/* … */` too, string-aware. gamedata/core/*.jsonc —
  // which authors are told to imitate — uses both freely, so a line-only stripper rejected the
  // house style. See gamedata/jsonc.ts.
  const raw = stripJsonComments(readFileSync(gdPath, "utf8"));
  try {
    gamedata = JSON.parse(raw) as PluginGamedata;
  } catch (e) {
    throw new Error(`invalid gamedata ${gdPath}: ${(e as Error).message}`);
  }
  const gdErrs = validatePluginGamedata(gamedata, { permissions });
  if (gdErrs.length) throw new Error(`invalid gamedata:\n  ${gdErrs.join("\n  ")}`);

  return gamedata;
}
