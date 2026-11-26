#!/usr/bin/env node
// Build one Svelte component into a custom_hud_layout: <name>.xml, <name>.css, <name>.manifest.json.
//
//   node --conditions=svelte-custom-renderer src/build.mjs <Entry.svelte> <outDir>
//
// Steps: compile every .svelte file it reaches with this package's renderer, mount the entry once
// in Node, serialise the tree. Any rule broken on the way is a build error, not a blank HUD.
import { compile } from "svelte/compiler";
import { mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { basename, dirname, join, resolve, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { checkCss } from "./css.mjs";
import { serialize } from "./serialize.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const RENDERER = join(HERE, "renderer.mjs");

export async function build(entry, { props = {} } = {}) {
  entry = resolve(entry);
  const name = basename(entry, ".svelte").toLowerCase();
  // Compiled modules sit beside this package so `svelte` resolves from its node_modules.
  const work = join(HERE, "..", ".build", `${name}-${process.pid}-${Date.now()}`);
  const root = commonRoot(entry, HERE);
  const css = [];
  const seen = new Map();

  const compileFile = (file) => {
    if (seen.has(file)) return seen.get(file);
    const out = join(work, relative(root, file) + ".mjs");
    seen.set(file, out);
    const result = compile(readFileSync(file, "utf8"), {
      filename: file,
      generate: "client",
      css: "external",
      experimental: { customRenderer: RENDERER },
    });
    for (const w of result.warnings) console.warn(`warning: ${relative(process.cwd(), file)}: ${w.message}`);
    // Plain .js imports keep pointing at their source; .svelte imports point at compiled output.
    let code = result.js.code.replace(/(from\s+|import\s+)(["'])(\.{1,2}\/[^"']+\.m?js)\2/g, (_all, kw, q, spec) =>
      `${kw}${q}${pathToFileURL(resolve(dirname(file), spec)).href}${q}`);
    code = code.replace(/(from\s+|import\s+)(["'])(\.{1,2}\/[^"']+\.svelte)\2/g, (_all, kw, q, spec) =>
      `${kw}${q}${pathToFileURL(compileFile(resolve(dirname(file), spec))).href}${q}`);
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, code);
    // Svelte scopes every selector after the first with :where(.svelte-hash) to keep specificity
    // flat. Panorama has no :where(), so write the plain compound selector it stands for.
    if (result.css?.code) css.push({ file, code: result.css.code.replace(/:where\((\.svelte-[\w-]+)\)/g, "$1") });
    return out;
  };

  try {
    const compiled = compileFile(entry);
    const { mount, flushSync } = await import("svelte");
    const renderer = (await import(pathToFileURL(RENDERER).href)).default;
    const component = (await import(pathToFileURL(compiled).href)).default;
    const target = renderer.createFragment();
    mount(component, { target, renderer, props });
    flushSync();

    const stylesheet = `${name}.css`;
    const { xml, manifest } = serialize(target, { stylesheet });
    const cssText = css.map((c) => `/* ${relative(root, c.file).replace(/\\/g, "/")} */\n${c.code}\n`).join("\n");
    const problems = css.flatMap((c) => checkCss(c.code, relative(process.cwd(), c.file)));
    if (problems.length) throw new Error("CSS the client would ignore:\n  " + problems.join("\n  "));
    return { name, xml, css: cssText, manifest };
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

function commonRoot(a, b) {
  const pa = a.split(/[\\/]/), pb = b.split(/[\\/]/);
  let i = 0;
  while (i < pa.length && pa[i] === pb[i]) i++;
  return pa.slice(0, i).join("/") || "/";
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  const [entry, outDir = "out"] = process.argv.slice(2);
  if (!entry) { console.error("usage: build.mjs <Entry.svelte> [outDir]"); process.exit(2); }
  try {
    const r = await build(entry);
    mkdirSync(outDir, { recursive: true });
    writeFileSync(join(outDir, `${r.name}.xml`), r.xml);
    writeFileSync(join(outDir, `${r.name}.css`), r.css);
    writeFileSync(join(outDir, `${r.name}.manifest.json`), JSON.stringify(r.manifest, null, 2) + "\n");
    console.log(`built ${r.name}: ${r.manifest.ids.length} ids, ${r.manifest.vars.length} dialog variables, ${r.manifest.buttons.length} buttons`);
  } catch (e) {
    console.error(`FAIL: ${entry}: ${e.message}`);
    process.exit(1);
  }
}
