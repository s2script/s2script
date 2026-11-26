// What CS2's CustomHud validator accepts, as data. One table drives the renderer (build error),
// the component prop types (editor error) and the serialiser.
//
// Read from client.dll of CS2 build 11259529 (2026-11-19). Re-derive after a CS2 update.
export const ALLOWED = {
  Panel: ["id", "class", "hittest"],
  Label: ["id", "class", "hittest", "text"],
  Image: ["id", "class", "hittest", "src", "languages"],
  Button: ["id", "class"],
};

// Each custom_hud_layout entity interns at most this many ids.
export const MAX_IDS = 1024;

// CSS that the resource compiler accepts and the client silently ignores
// (examples/hud-lab/workshop/PANORAMA.md). A web habit, not a Panorama property.
export const CSS_DEAD_PROPERTIES = new Set([
  "display", "flex", "flex-direction", "flex-grow", "flex-shrink", "flex-basis", "flex-wrap", "flex-flow",
  "grid", "grid-template", "grid-template-columns", "grid-template-rows", "grid-area", "grid-column", "grid-row",
  "justify-content", "align-items", "align-content", "align-self", "justify-items", "justify-self", "gap", "row-gap", "column-gap", "order",
  "float", "clear", "top", "right", "bottom", "left", "box-sizing", "content", "list-style", "outline",
  "filter", "object-fit", "aspect-ratio", "clip-path", "mask", "pointer-events", "user-select", "mix-blend-mode",
]);
export const CSS_DEAD_VALUES = [/\bcalc\(/, /\bvar\(/, /\brgba?\(/, /\bhsla?\(/];
export const CSS_DEAD_SELECTORS = [/:not\(/, /:where\(/, /:is\(/, /:has\(/, /::(before|after)/, /\[[^\]]+\]/, /:nth-of-type/];
export const CSS_DEAD_AT_RULES = ["@media", "@supports", "@container"];
