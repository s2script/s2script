/** `s2s config gen` — emit an operator's default config file for a plugin, at package/release time.
 *
 *  Input is a STAGED .s2sp (its baked manifest.json, post-validation), never the source package.json.
 *  Output is a commented JSONC file whose bytes match the core's `generate_default_jsonc`
 *  (core/src/config.rs): sorted keys, a `// <type>[ — <description>]` comment per decl, defaults at
 *  their declared value, sections nested as `{ … }` blocks, dotted keys skipped. The filename matches
 *  the runtime's ConfigPath (see config-path.ts).
 *
 *  Plugin-scoped by design: this knows nothing about the framework templates (admins/databases/…) —
 *  those are shipped by the release script, not by the published CLI. */

import { unzipSync } from "fflate";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { configFileName } from "./config-path.ts";

/** A config entry is a value DECL iff it has a string-valued `type` key; else it is a SECTION.
 *  (Shared verbatim with the core's untagged ConfigEntry + the SDK validator.) */
function isDecl(v: Record<string, unknown>): boolean {
  return typeof v.type === "string";
}

/** Produce the commented JSONC for a config block. Mirror of core's `generate_default_jsonc`. */
export function generateDefaultJsonc(config: Record<string, unknown>): string {
  return "{\n" + genEntries(config, 1) + "}\n";
}

function genEntries(entries: Record<string, unknown>, indent: number): string {
  const pad = "  ".repeat(indent);
  // Skip dotted keys BEFORE computing comma positions so the emitted JSONC stays valid, then sort.
  const keys = Object.keys(entries).filter((k) => !k.includes(".")).sort();
  let out = "";
  keys.forEach((key, i) => {
    const comma = i + 1 < keys.length ? "," : "";
    const raw = entries[key];
    if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return; // malformed post-validation — skip
    const obj = raw as Record<string, unknown>;
    if (isDecl(obj)) {
      const desc = typeof obj.description === "string" ? obj.description : "";
      out += `${pad}// ${String(obj.type)}${desc ? ` — ${desc}` : ""}\n`;
      out += `${pad}${JSON.stringify(key)}: ${JSON.stringify(obj.default)}${comma}\n`;
    } else {
      out += `${pad}${JSON.stringify(key)}: {\n`;
      out += genEntries(obj, indent + 1);
      out += `${pad}}${comma}\n`;
    }
  });
  return out;
}

/** Given a parsed manifest, write its default config file into `outDir`. Returns the written path, or
 *  null if the manifest declares no (non-empty) config. Throws if a config-bearing manifest has no id. */
export function genConfigFromManifest(manifest: { id?: unknown; config?: unknown }, outDir: string): string | null {
  const config = manifest.config;
  if (config == null || typeof config !== "object" || Array.isArray(config) || Object.keys(config as object).length === 0) {
    return null;
  }
  const id = typeof manifest.id === "string" ? manifest.id : "";
  if (!id) throw new Error("config gen: manifest declares config but has no id");
  mkdirSync(outDir, { recursive: true });
  const path = join(outDir, configFileName(id));
  writeFileSync(path, generateDefaultJsonc(config as Record<string, unknown>));
  return path;
}

/** Read a staged .s2sp, extract its manifest.json, and write its default config file into `outDir`. */
export function genConfigForS2sp(s2spPath: string, outDir: string): string | null {
  const entry = unzipSync(readFileSync(s2spPath))["manifest.json"];
  if (!entry) throw new Error(`config gen: ${s2spPath} has no manifest.json`);
  const manifest = JSON.parse(Buffer.from(entry).toString("utf8"));
  return genConfigFromManifest(manifest, outDir);
}

/** Run config gen over a list of staged .s2sp paths, writing into `outDir`. */
export function runConfigGen(s2sps: string[], outDir: string): { written: string[]; skipped: string[] } {
  const written: string[] = [];
  const skipped: string[] = [];
  for (const p of s2sps) {
    const r = genConfigForS2sp(p, outDir);
    if (r) written.push(r);
    else skipped.push(p);
  }
  return { written, skipped };
}
