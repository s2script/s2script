<script lang="ts">
  import Panel from "../src/kit/Panel.svelte";
  import Label from "../src/kit/Label.svelte";
  import Image from "../src/kit/Image.svelte";
  // The pool size is a prop: the server cannot create panels, so every row exists up front.
  let { rows = 6 }: { rows?: number } = $props();
</script>

<Panel id="gk_feed" class="feed">
  {#each { length: rows }, i}
    <Panel id="gk_kf{i}" class="kill s2-hide">
      <Label id="gk_kf{i}_a" class="name" />
      <Image id="gk_kf{i}_wi" class="icon" />
      <Panel id="gk_kf{i}_hs" class="hs s2-hide" />
      <Label id="gk_kf{i}_v" class="name" />
    </Panel>
  {/each}
</Panel>

<style>
  :global(.feed) { flow-children: down; horizontal-align: right; margin: 96px 20px 0px 0px; }
  :global(.kill) { flow-children: right; horizontal-align: right; padding: 5px 9px; background-color: #090b0dcc; }
  :global(.name) { font-size: 15px; font-weight: bold; color: #eef1f3; }
  :global(.icon) { height: 16px; width: fit-children; margin: 0px 7px; }
  :global(.hs) { width: 14px; height: 14px; background-color: #e5484d; }
  :global(.s2-hide) { visibility: collapse; }
</style>
