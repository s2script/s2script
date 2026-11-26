<script lang="ts">
  import type { Snippet } from "svelte";
  // CustomHud's Button takes id and class only. The id is how the click reaches the server.
  let { id, class: cls, children }: { id: string; class?: string; children?: Snippet } = $props();
</script>

<hud-button {id} class={cls}>{@render children?.()}</hud-button>
