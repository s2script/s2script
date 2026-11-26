// The CustomHud whitelist as element types. Mirrors ALLOWED in ../rules.mjs.
type HitTest = "true" | "false";
declare namespace svelteHTML {
  interface IntrinsicElements {
    "hud-panel": { id?: string; class?: string; hittest?: HitTest; children?: import("svelte").Snippet };
    "hud-label": { id?: string; class?: string; hittest?: HitTest; text: string };
    "hud-image": { id?: string; class?: string; hittest?: HitTest; src: string; languages?: string };
    "hud-button": { id: string; class?: string; children?: import("svelte").Snippet };
  }
}
