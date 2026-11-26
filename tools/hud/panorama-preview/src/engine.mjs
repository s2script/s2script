// A Panorama layout engine for the CustomHud subset: parse a layout XML and its stylesheet, resolve
// styles, and compute every panel's box the way the game does. It owns layout entirely. A browser
// is only asked to measure text and to paint boxes at the positions computed here, so web layout
// (block, flex, line boxes) never gets a say.
//
// One file, no imports: Node runs it in tests with a fake text measurer, and build-preview.mjs
// inlines it into a self-contained page.
//
// Every rule below that is not yet confirmed against an in-game capture is listed in ASSUMPTIONS.
// The preview is only as true as that list is short.

export const ASSUMPTIONS = [
  ["centre-margins", "a centre-aligned panel is centred in the space its margins leave (seen in game for one case)"],
  ["box-model", "width and height include padding and border; margin is outside"],
  ["default-size", "a panel with no width or height is fit-children on that axis; a Label fits its text"],
  ["percent-of", "a percentage size is a share of the parent's content box (inside padding and border)"],
  ["percent-in-fit", "a percentage or fill child adds nothing to a fit-children parent's size on that axis"],
  ["flow-axis-align", "in a flowing parent, alignment applies on the cross axis only"],
  ["fill-share", "fill-parent-flow(w) children split the space left after fixed siblings, by weight"],
  ["cascade", "specificity then source order, as on the web"],
  ["text-inherit", "color and font properties are inherited by child panels"],
  ["line-height", "a Label with no line-height is the font's own line height"],
  ["font", "the default face is Stratum2; the preview falls back to a condensed system face if it is not installed"],
  ["canvas", "layout units are pixels of a 1080-high screen; other resolutions scale uniformly"],
];

// ---- XML --------------------------------------------------------------------------------------
export function parseLayout(xml) {
  const src = xml.replace(/<!--[\s\S]*?-->/g, "");
  const tagRe = /<\s*(\/?)\s*([A-Za-z_][\w.-]*)([^>]*?)(\/?)\s*>/g;
  const root = { tag: "#root", attrs: {}, children: [], parent: null };
  let cur = root, m;
  while ((m = tagRe.exec(src))) {
    const [, closing, tag, rest, selfClose] = m;
    if (closing) { cur = cur.parent ?? root; continue; }
    const attrs = {};
    for (const a of rest.matchAll(/([A-Za-z_:][\w:.-]*)\s*=\s*"([^"]*)"/g)) attrs[a[1]] = a[2];
    const node = { tag, attrs, children: [], parent: cur };
    cur.children.push(node);
    if (!selfClose) cur = node;
  }
  const find = (n, t) => n.tag === t ? n : n.children.map((c) => find(c, t)).find(Boolean);
  const top = find(root, "root");
  const outer = top?.children.find((c) => ["Panel", "Label", "Image", "Button"].includes(c.tag));
  if (!outer) throw new Error("layout has no outer panel");
  const prep = (n, parent) => {
    n.parent = parent;
    n.id = n.attrs.id ?? null;
    n.classes = new Set((n.attrs.class ?? "").split(/\s+/).filter(Boolean));
    n.children.forEach((c) => prep(c, n));
  };
  prep(outer, null);
  return outer;
}

// ---- CSS --------------------------------------------------------------------------------------
export function parseCss(css) {
  const src = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const rules = [];
  let i = 0, order = 0;
  while (i < src.length) {
    const open = src.indexOf("{", i);
    if (open < 0) break;
    const prelude = src.slice(i, open).trim();
    // Balanced close, so @keyframes blocks are skipped whole.
    let depth = 1, j = open + 1;
    while (j < src.length && depth) { if (src[j] === "{") depth++; else if (src[j] === "}") depth--; j++; }
    const body = src.slice(open + 1, j - 1);
    i = j;
    if (prelude.startsWith("@")) continue;
    const decls = {};
    for (const d of body.split(";")) {
      const c = d.indexOf(":");
      if (c > 0) expand(d.slice(0, c).trim().toLowerCase(), d.slice(c + 1).trim(), decls);
    }
    for (const sel of prelude.split(",")) {
      const parsed = parseSelector(sel.trim());
      if (parsed) rules.push({ ...parsed, decls, order: order++ });
    }
  }
  return rules;
}

const SIDES = ["top", "right", "bottom", "left"];
function expand(prop, value, out) {
  if (prop === "margin" || prop === "padding") {
    const v = value.split(/\s+/);
    const four = v.length === 1 ? [v[0], v[0], v[0], v[0]] : v.length === 2 ? [v[0], v[1], v[0], v[1]] : v.length === 3 ? [v[0], v[1], v[2], v[1]] : v;
    SIDES.forEach((s, k) => { out[`${prop}-${s}`] = four[k]; });
  } else if (prop === "border") {
    SIDES.forEach((s) => { out[`border-${s}`] = value; });
  } else if (prop === "align") {
    const [h, v] = value.split(/\s+/);
    out["horizontal-align"] = h; if (v) out["vertical-align"] = v;
  } else out[prop] = value;
}

function parseSelector(text) {
  if (!text) return null;
  const parts = [];
  let spec = 0;
  for (const tok of text.replace(/\s*>\s*/g, " > ").split(/\s+/)) {
    if (tok === ">") { parts.push({ child: true }); continue; }
    const c = { tag: null, id: null, classes: [], pseudo: [] };
    for (const m of tok.matchAll(/([.#:]?)([\w-]+)(\([^)]*\))?/g)) {
      if (m[1] === ".") { c.classes.push(m[2]); spec += 100; }
      else if (m[1] === "#") { c.id = m[2]; spec += 10000; }
      else if (m[1] === ":") { c.pseudo.push(m[2] + (m[3] ?? "")); spec += 100; }
      else { c.tag = m[2]; spec += 1; }
    }
    parts.push(c);
  }
  return { parts, spec };
}

function matchCompound(node, c, state) {
  if (c.tag && c.tag !== node.tag) return false;
  if (c.id && c.id !== node.id) return false;
  for (const k of c.classes) if (!node.classes.has(k)) return false;
  for (const p of c.pseudo) {
    if (p === "root") { if (node.parent) return false; }
    else if (p === "first-child") { if (!node.parent || node.parent.children[0] !== node) return false; }
    else if (p === "last-child") { if (!node.parent || node.parent.children.at(-1) !== node) return false; }
    else if (!(state.pseudo?.[node.id] ?? []).includes(p)) return false; // :hover and friends: off unless the scenario sets them
  }
  return true;
}

function matches(node, parts, state) {
  let k = parts.length - 1;
  if (!matchCompound(node, parts[k], state)) return false;
  let cur = node;
  for (k--; k >= 0; k--) {
    if (parts[k].child) {
      k--; cur = cur.parent;
      if (!cur || !matchCompound(cur, parts[k], state)) return false;
      continue;
    }
    cur = cur.parent;
    while (cur && !matchCompound(cur, parts[k], state)) cur = cur.parent;
    if (!cur) {
      // The engine puts HUD_* state classes on an ancestor outside the layout.
      if (parts[k].classes.length && parts[k].classes.every((c) => state.ancestorClasses?.includes(c)) && !parts[k].id && !parts[k].tag) continue;
      return false;
    }
  }
  return true;
}

const INHERITED = ["color", "font-size", "font-weight", "font-family", "font-style", "letter-spacing", "text-transform", "text-align", "line-height", "text-shadow", "white-space"];

export function resolveStyles(root, rules, state = {}) {
  const sorted = [...rules].sort((a, b) => a.spec - b.spec || a.order - b.order);
  const walk = (n) => {
    const style = {};
    if (n.parent) for (const p of INHERITED) if (n.parent.style[p] !== undefined) style[p] = n.parent.style[p];
    for (const r of sorted) if (matches(n, r.parts, state)) Object.assign(style, r.decls);
    n.style = style;
    n.children.forEach(walk);
  };
  walk(root);
}

// ---- values -----------------------------------------------------------------------------------
const px = (v) => { const n = parseFloat(v); return Number.isFinite(n) ? n : 0; };
function sizeSpec(v) {
  if (v === undefined || v === "fit-children") return { kind: "fit" };
  let m;
  if ((m = /^fill-parent-flow\(\s*([\d.]+)\s*\)$/.exec(v))) return { kind: "fill", n: parseFloat(m[1]) };
  if ((m = /^([\d.]+)%$/.exec(v))) return { kind: "pct", n: parseFloat(m[1]) };
  if ((m = /^(?:width|height)-percentage\(\s*([\d.]+)%?\s*\)$/.exec(v))) return { kind: "aspect", n: parseFloat(m[1]) };
  return { kind: "px", n: px(v) };
}
const borderWidth = (v) => (v ? px(v.split(/\s+/)[0]) : 0);
function edges(s) {
  const e = {};
  for (const side of SIDES) {
    e["m" + side[0]] = px(s[`margin-${side}`]);
    e["p" + side[0]] = px(s[`padding-${side}`]) + borderWidth(s[`border-${side}`]);
  }
  return e;
}
const clampDim = (v, s, axis) => {
  const mn = s[`min-${axis}`], mx = s[`max-${axis}`];
  if (mx !== undefined && /px$|^[\d.]+$/.test(mx)) v = Math.min(v, px(mx));
  if (mn !== undefined && /px$|^[\d.]+$/.test(mn)) v = Math.max(v, px(mn));
  return v;
};

export function displayText(n, state) {
  const raw = n.attrs.text ?? "";
  let t = raw.replace(/\{s:(\w+)\}/g, (_a, k) => state.vars?.[k] ?? "");
  const tt = n.style["text-transform"];
  if (tt === "uppercase") t = t.toUpperCase(); else if (tt === "lowercase") t = t.toLowerCase();
  return t;
}

// ---- layout -----------------------------------------------------------------------------------
// layout(node, availW, availH, forceW, forceH) sizes a node and places its children inside it.
// avail* is the parent's content box on that axis, or null while the parent is still working out a
// fit-children size. force* is a size the parent assigns (a fill-parent-flow share).
export function layoutTree(root, { width = 1920, height = 1080, measure, state = {} }) {
  const ctx = { measure, state };
  layout(root, width, height, null, null, ctx);
  root.x = 0; root.y = 0;
  return root;
}

const collapsed = (n) => n.style.visibility === "collapse";

function layout(n, availW, availH, forceW, forceH, ctx) {
  const s = n.style;
  if (collapsed(n)) { n.w = 0; n.h = 0; n.e = edges({}); n.hidden = true; return; }
  n.hidden = false;
  const e = (n.e = edges(s));
  const ws = sizeSpec(s.width), hs = sizeSpec(s.height);
  const fixed = (spec, avail, force) =>
    force != null ? force : spec.kind === "px" ? spec.n : spec.kind === "pct" && avail != null ? (avail * spec.n) / 100 : null;
  let w = fixed(ws, availW, forceW), h = fixed(hs, availH, forceH);
  // width: height-percentage(n) and height: width-percentage(n) tie one axis to the other.
  if (ws.kind === "aspect" && h != null) w = (h * ws.n) / 100;
  if (hs.kind === "aspect" && w != null) h = (w * hs.n) / 100;
  if (w != null) w = clampDim(w, s, "width");
  if (h != null) h = clampDim(h, s, "height");
  const padW = e.pl + e.pr, padH = e.pt + e.pb;

  let contentW = 0, contentH = 0;
  if (n.tag === "Label") {
    const maxW = w != null ? Math.max(0, w - padW) : s["max-width"] ? px(s["max-width"]) - padW : availW != null ? Math.max(0, availW - e.ml - e.mr - padW) : null;
    const t = ctx.measure(displayText(n, ctx.state), s, maxW);
    contentW = t.w; contentH = t.h; n.textBox = t;
  } else if (n.tag === "Image") {
    const im = ctx.state.images?.[n.id];
    if (h != null && im) contentW = ((h - padH) * im.w) / im.h; else if (im) { contentW = im.w; contentH = im.h; }
  } else {
    const inner = flowChildren(n, w != null ? w - padW : null, h != null ? h - padH : null, ctx);
    contentW = inner.w; contentH = inner.h;
  }
  if (w == null) w = clampDim(contentW + padW, s, "width");
  if (h == null) h = clampDim(hs.kind === "aspect" ? (w * hs.n) / 100 : contentH + padH, s, "height");
  if (ws.kind === "aspect" && forceW == null) w = clampDim((h * ws.n) / 100, s, "width");
  n.w = w; n.h = h;
  // Second pass now that both sizes are known: percentage and fill children can resolve.
  if (n.tag !== "Label" && n.tag !== "Image") flowChildren(n, w - padW, h - padH, ctx, true);
}

function flowChildren(n, cw, ch, ctx, place = false) {
  const flow = (n.style["flow-children"] ?? "none").trim();
  const kids = n.children.filter((c) => { if (collapsed(c)) { c.hidden = true; c.w = c.h = 0; return false; } return true; });
  const free = kids.filter((c) => flow === "none" || c.style["ignore-parent-flow"] === "true");
  const inFlow = kids.filter((c) => !free.includes(c));
  let fitW = 0, fitH = 0;
  const outerW = (c) => c.w + c.e.ml + c.e.mr, outerH = (c) => c.h + c.e.mt + c.e.mb;
  const counts = (c, axis) => { const k = sizeSpec(c.style[axis]).kind; return k !== "pct" && k !== "fill"; };

  for (const c of free) {
    layout(c, cw, ch, null, null, ctx);
    if (counts(c, "width")) fitW = Math.max(fitW, outerW(c));
    if (counts(c, "height")) fitH = Math.max(fitH, outerH(c));
    if (place) { c.x = n.e.pl + alignPos(c.style["horizontal-align"], cw, c.w, c.e.ml, c.e.mr, "left", "right"); c.y = n.e.pt + alignPos(c.style["vertical-align"], ch, c.h, c.e.mt, c.e.mb, "top", "bottom"); }
  }

  if (inFlow.length) {
    const horizontal = flow !== "down";
    const main = horizontal ? "width" : "height";
    const fills = inFlow.filter((c) => sizeSpec(c.style[main]).kind === "fill");
    const mainAvail = horizontal ? cw : ch;
    let used = 0;
    for (const c of inFlow) {
      if (fills.includes(c)) { used += horizontal ? px(c.style["margin-left"]) + px(c.style["margin-right"]) : px(c.style["margin-top"]) + px(c.style["margin-bottom"]); continue; }
      layout(c, cw, ch, null, null, ctx);
      used += horizontal ? outerW(c) : outerH(c);
    }
    const weight = fills.reduce((a, c) => a + sizeSpec(c.style[main]).n, 0);
    const spare = mainAvail != null ? Math.max(0, mainAvail - used) : null;
    for (const c of fills) {
      const share = spare != null ? (spare * sizeSpec(c.style[main]).n) / weight : null;
      if (horizontal) layout(c, cw, ch, share, null, ctx); else layout(c, cw, ch, null, share, ctx);
    }

    if (flow === "right-wrap" && cw != null) {
      let x = 0, y = 0, lineH = 0, maxX = 0;
      for (const c of inFlow) {
        if (x > 0 && x + outerW(c) > cw + 0.01) { x = 0; y += lineH; lineH = 0; }
        if (place) { c.x = n.e.pl + x + c.e.ml; c.y = n.e.pt + y + c.e.mt; }
        x += outerW(c); lineH = Math.max(lineH, outerH(c)); maxX = Math.max(maxX, x);
      }
      fitW = Math.max(fitW, maxX); fitH = Math.max(fitH, y + lineH);
    } else {
      let cursor = 0, cross = 0;
      for (const c of inFlow) {
        const m = horizontal ? outerW(c) : outerH(c);
        if (place) {
          if (horizontal) {
            c.x = flow === "left" ? n.e.pl + cw - cursor - m + c.e.ml : n.e.pl + cursor + c.e.ml;
            c.y = n.e.pt + alignPos(c.style["vertical-align"], ch, c.h, c.e.mt, c.e.mb, "top", "bottom");
          } else {
            c.y = n.e.pt + cursor + c.e.mt;
            c.x = n.e.pl + alignPos(c.style["horizontal-align"], cw, c.w, c.e.ml, c.e.mr, "left", "right");
          }
        }
        if (!fills.includes(c) || mainAvail != null) cursor += m;
        if (counts(c, horizontal ? "height" : "width")) cross = Math.max(cross, horizontal ? outerH(c) : outerW(c));
      }
      if (horizontal) { fitW = Math.max(fitW, cursor); fitH = Math.max(fitH, cross); }
      else { fitH = Math.max(fitH, cursor); fitW = Math.max(fitW, cross); }
    }
  }
  return { w: fitW, h: fitH };
}

function alignPos(mode, avail, size, before, after, _start, end) {
  // Centred inside what the margins leave, so each margin moves the panel by half its size
  // (seen in game: the target label's margin-top 120px sits it 60px below centre).
  if (mode === "center" || mode === "middle") return before + (avail - before - after - size) / 2;
  if (mode === end) return avail - size - after;
  return before;
}

// ---- flatten for painting and for tests -------------------------------------------------------
export function boxes(root) {
  const out = [];
  const walk = (n, ox, oy, depth) => {
    if (n.hidden) return;
    const x = ox + (n.x ?? 0), y = oy + (n.y ?? 0);
    out.push({ node: n, id: n.id, tag: n.tag, x, y, w: n.w, h: n.h, depth });
    n.children.forEach((c) => walk(c, x, y, depth + 1));
  };
  walk(root, 0, 0, 0);
  return out;
}

export function applyScenario(root, scenario = {}) {
  const byId = new Map();
  const walk = (n) => { if (n.id) byId.set(n.id, n); n.children.forEach(walk); };
  walk(root);
  for (const [id, change] of Object.entries(scenario.classes ?? {})) {
    const n = byId.get(id);
    if (!n) throw new Error(`scenario names unknown id ${id}`);
    for (const c of change.remove ?? []) n.classes.delete(c);
    for (const c of change.add ?? []) n.classes.add(c);
  }
  for (const id of scenario.show ?? []) {
    const n = byId.get(id);
    if (!n) throw new Error(`scenario names unknown id ${id}`);
    n.classes.delete("s2-hide");
  }
  return { vars: scenario.vars ?? {}, images: scenario.images ?? {}, pseudo: scenario.pseudo ?? {}, ancestorClasses: scenario.ancestorClasses ?? [] };
}
