// A Svelte custom renderer whose "DOM" is a custom_hud_layout tree.
//
// It never runs in the game: CustomHud has no client scripting. It runs once, in Node, at build
// time. A component is mounted into this tree, the tree is checked and written out as XML. A tag
// or attribute the client would reject cannot be created at all: createElement and setAttribute
// throw, naming the component file.
import { createRenderer } from "svelte/renderer";
import { ALLOWED } from "./rules.mjs";

const TAG = { "hud-panel": "Panel", "hud-label": "Label", "hud-image": "Image", "hud-button": "Button" };

export class LayoutError extends Error {}

const node = (type, extra) => ({ type, parent: null, children: [], ...extra });

function detach(n) {
  if (!n.parent) return;
  const i = n.parent.children.indexOf(n);
  if (i >= 0) n.parent.children.splice(i, 1);
  n.parent = null;
}

export default createRenderer({
  createFragment: () => node("fragment"),
  createElement(name) {
    const tag = TAG[name];
    if (!tag) throw new LayoutError(`<${name}> is not a CustomHud panel type. Use Panel, Label, Image or Button.`);
    return node("element", { tag, attrs: new Map() });
  },
  createText: (data) => node("text", { data }),
  createComment: (data) => node("comment", { data }),
  getNodeType: (n) => n.type,
  getText: (n) => n.data,
  setText(n, text) { n.data = text; },
  getAttribute: (el, name) => el.attrs.get(name) ?? null,
  hasAttribute: (el, name) => el.attrs.has(name),
  removeAttribute(el, name) { el.attrs.delete(name); },
  setAttribute(el, name, value) {
    if (!ALLOWED[el.tag].includes(name)) {
      throw new LayoutError(`disallowed attribute ${name} for panel type '${el.tag}' (allowed: ${ALLOWED[el.tag].join(", ")})`);
    }
    if (value === false || value == null) el.attrs.delete(name);
    else el.attrs.set(name, String(value));
  },
  getFirstChild: (n) => n.children[0] ?? null,
  getLastChild: (n) => n.children[n.children.length - 1] ?? null,
  getNextSibling(n) {
    if (!n.parent) return null;
    return n.parent.children[n.parent.children.indexOf(n) + 1] ?? null;
  },
  getParent: (n) => n.parent,
  insert(parent, n, anchor) {
    const list = n.type === "fragment" ? n.children.splice(0) : [n];
    for (const c of list) {
      detach(c);
      const i = anchor ? parent.children.indexOf(anchor) : -1;
      if (i >= 0) parent.children.splice(i, 0, c);
      else parent.children.push(c);
      c.parent = parent;
    }
  },
  remove: detach,
  addEventListener() {
    throw new LayoutError("CustomHud has no client events. A Button click reaches the server by its id.");
  },
  removeEventListener() {},
});
