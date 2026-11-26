// Tree -> layout XML + manifest, with the whole-file rules the validator and the runtime impose.
import { ALLOWED, MAX_IDS } from "./rules.mjs";
import { LayoutError } from "./renderer.mjs";

const esc = (s) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const VAR = /^\{s:([A-Za-z_]\w*)\}$/;

export function serialize(fragment, { stylesheet }) {
  const elements = fragment.children.filter((c) => c.type === "element");
  if (elements.length !== 1) throw new LayoutError(`a layout needs exactly one outer panel, found ${elements.length}`);
  const outer = elements[0];
  if (outer.attrs.has("id")) throw new LayoutError("the outer panel may not carry an id");

  const manifest = { ids: [], vars: [], buttons: [] };
  const lines = [];
  const walk = (el, depth) => {
    const pad = "\t".repeat(depth);
    const id = el.attrs.get("id");
    if (id !== undefined) {
      if (!/^[A-Za-z_]\w*$/.test(id)) throw new LayoutError(`id "${id}" is not a plain identifier`);
      if (manifest.ids.includes(id)) throw new LayoutError(`duplicate id ${id}`);
      manifest.ids.push(id);
      if (el.tag === "Button") manifest.buttons.push(id);
    } else if (el.tag === "Button") {
      throw new LayoutError("a Button without an id can never report a click");
    }
    for (const key of ["text", "src"]) {
      const m = VAR.exec(el.attrs.get(key) ?? "");
      if (!m) continue;
      if (m[1] !== id) throw new LayoutError(`dialog variable {s:${m[1]}} must be named after its panel id (${id ?? "none"})`);
      manifest.vars.push(id);
    }
    // Attribute order follows the whitelist, so output is stable whatever order Svelte sets them in.
    const attrs = ALLOWED[el.tag].filter((k) => el.attrs.has(k)).map((k) => ` ${k}="${esc(el.attrs.get(k))}"`).join("");
    const kids = el.children.filter((c) => c.type === "element");
    const stray = el.children.find((c) => c.type === "text" && c.data.trim());
    if (stray) throw new LayoutError(`loose text "${stray.data.trim()}" inside <${el.tag}>. Text belongs in a Label's text.`);
    if (el.tag === "Label" || el.tag === "Image") {
      if (kids.length) throw new LayoutError(`<${el.tag}> cannot have children`);
    }
    if (!kids.length) { lines.push(`${pad}<${el.tag}${attrs} />`); return; }
    lines.push(`${pad}<${el.tag}${attrs}>`);
    for (const k of kids) walk(k, depth + 1);
    lines.push(`${pad}</${el.tag}>`);
  };
  walk(outer, 1);
  if (manifest.ids.length > MAX_IDS) throw new LayoutError(`${manifest.ids.length} ids, over the ${MAX_IDS} one layout entity can intern`);

  const xml = [
    "<root>",
    "\t<styles>",
    `\t\t<include src="file://{resources}/styles/custom_game/${stylesheet}" />`,
    "\t</styles>",
    ...lines,
    "</root>",
    "",
  ].join("\n");
  return { xml, manifest };
}
