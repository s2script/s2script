// Browser side: measure text with the real font stack, then paint each computed box as an
// absolutely positioned element. No element here takes part in web layout.
const FONT_STACK = `"Stratum2", "Stratum2 Web", "Roboto Condensed", "Arial Narrow", "Helvetica Neue", Arial, sans-serif`;

function textCss(s) {
  return {
    fontFamily: s["font-family"] ? `${s["font-family"]}, ${FONT_STACK}` : FONT_STACK,
    fontSize: s["font-size"] ?? "18px",
    fontWeight: { bold: "700", black: "900", medium: "500", semi_bold: "600", "semi-bold": "600", light: "300", thin: "200", normal: "400" }[s["font-weight"]] ?? s["font-weight"] ?? "400",
    fontStyle: s["font-style"] ?? "normal",
    letterSpacing: s["letter-spacing"] ?? "0px",
    lineHeight: s["line-height"] ?? "normal",
  };
}

const probe = document.createElement("span");
probe.style.cssText = "position:absolute;left:-99999px;top:0;visibility:hidden;";
document.documentElement.appendChild(probe);

function measure(text, s, maxW) {
  Object.assign(probe.style, textCss(s));
  const wrap = maxW != null && s["white-space"] !== "nowrap";
  probe.style.whiteSpace = wrap ? "pre-wrap" : "pre";
  probe.style.display = "inline-block";
  probe.style.maxWidth = wrap ? `${maxW}px` : "none";
  probe.textContent = text || "​";
  const r = probe.getBoundingClientRect();
  return { w: text ? Math.ceil(r.width * 100) / 100 : 0, h: r.height };
}

// Panorama text-shadow: hOff vOff blur strength color. The web has no strength, so it is dropped.
const shadow = (v) => { const p = v.trim().split(/\s+/); return p.length === 5 ? `${p[0]} ${p[1]} ${p[2]} ${p[4]}` : v; };
const transform = (v) => v.replace(/rotateZ/g, "rotate").replace(/\(\s+/g, "(").replace(/\s+\)/g, ")");

function paint(root, stage, state, opts) {
  stage.replaceChildren();
  const draw = (n, parentEl) => {
    if (n.hidden) return;
    const s = n.style, el = document.createElement("div");
    const st = el.style;
    st.position = "absolute"; st.boxSizing = "border-box";
    st.left = `${n.x ?? 0}px`; st.top = `${n.y ?? 0}px`; st.width = `${n.w}px`; st.height = `${n.h}px`;
    if (s["background-color"]) st.backgroundColor = s["background-color"];
    for (const side of ["top", "right", "bottom", "left"]) if (s[`border-${side}`]) st[`border${side[0].toUpperCase()}${side.slice(1)}`] = s[`border-${side}`];
    if (s["border-radius"]) st.borderRadius = s["border-radius"];
    if (s.opacity) st.opacity = s.opacity;
    if (s["z-index"]) st.zIndex = s["z-index"];
    if (s["box-shadow"]) st.boxShadow = s["box-shadow"];
    if (s.transform) st.transform = transform(s.transform);
    if (s.overflow && /clip|squish|scroll/.test(s.overflow)) st.overflow = "hidden";
    if (n.id) el.dataset.id = n.id;
    el.dataset.cls = [...n.classes].join(" ");
    if (opts.outline) st.outline = "1px solid #ff00ff55";
    if (n.tag === "Label") {
      const t = document.createElement("span");
      Object.assign(t.style, textCss(s));
      t.style.position = "absolute";
      t.style.left = `${n.e.pl - parseFloat(st.borderLeftWidth || 0)}px`; t.style.top = `${n.e.pt - parseFloat(st.borderTopWidth || 0)}px`;
      t.style.width = `${n.w - n.e.pl - n.e.pr}px`;
      t.style.whiteSpace = s["white-space"] === "nowrap" ? "pre" : "pre-wrap";
      t.style.textAlign = s["text-align"] ?? "left";
      t.style.color = s.color ?? "#ffffff";
      if (s["text-shadow"]) t.style.textShadow = shadow(s["text-shadow"]);
      t.textContent = displayText(n, state);
      el.appendChild(t);
    } else if (n.tag === "Image" && n.w > 0 && n.h > 0) {
      st.backgroundImage = "repeating-linear-gradient(45deg,#ffffff22 0 4px,#ffffff08 4px 8px)";
    }
    parentEl.appendChild(el);
    n.children.forEach((c) => draw(c, el));
  };
  draw(root, stage);
}

function render(data, stage, opts = {}) {
  const root = parseLayout(data.xml);
  const state = applyScenario(root, data.scenario);
  resolveStyles(root, parseCss(data.css), state);
  layoutTree(root, { width: 1920, height: 1080, measure, state });
  paint(root, stage, state, opts);
  return root;
}
