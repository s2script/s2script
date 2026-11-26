// Reject CSS the Panorama client silently ignores. The resource compiler validates none of it.
import { CSS_DEAD_PROPERTIES, CSS_DEAD_VALUES, CSS_DEAD_SELECTORS, CSS_DEAD_AT_RULES } from "./rules.mjs";

/** Problems in a stylesheet, as strings. */
export function checkCss(css, file = "") {
  const problems = [];
  const src = css.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));
  const at = (i) => `${file}:${src.slice(0, i).split("\n").length}`;
  for (const rule of CSS_DEAD_AT_RULES) {
    for (const m of src.matchAll(new RegExp(rule + "\\b", "g"))) problems.push(`${at(m.index)}: ${rule} does not exist in Panorama`);
  }
  for (const m of src.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selector = m[1].trim();
    if (!selector.startsWith("@") && !/^(from|to|[\d.]+%)/.test(selector)) {
      for (const re of CSS_DEAD_SELECTORS) if (re.test(selector)) problems.push(`${at(m.index)}: selector "${selector}" uses syntax Panorama ignores`);
    }
    let offset = m.index + m[1].length + 1;
    for (const decl of m[2].split(";")) {
      const c = decl.indexOf(":");
      if (c > 0) {
        const prop = decl.slice(0, c).trim().toLowerCase();
        const value = decl.slice(c + 1).trim();
        if (CSS_DEAD_PROPERTIES.has(prop)) problems.push(`${at(offset)}: property "${prop}" does not exist in Panorama`);
        for (const re of CSS_DEAD_VALUES) if (re.test(value)) problems.push(`${at(offset)}: value "${value}" uses a function Panorama ignores`);
      }
      offset += decl.length + 1;
    }
  }
  return problems;
}
