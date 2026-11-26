<script lang="ts">
  import type { Snippet } from "svelte";
  // The whole CustomHud whitelist for Panel. Anything else is a type error here and a build error
  // in the renderer.
  let { id, class: cls, hittest = false, children }: { id?: string; class?: string; hittest?: boolean; children?: Snippet } = $props();
</script>

<hud-panel {id} class={cls} hittest={hittest ? "true" : "false"}>{@render children?.()}</hud-panel>
