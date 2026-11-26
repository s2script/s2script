<script lang="ts">
  import Panel from "../src/kit/Panel.svelte";
  import Label from "../src/kit/Label.svelte";
  import Button from "../src/kit/Button.svelte";
  import KillFeed from "./KillFeed.svelte";
</script>

<Panel class="root">
  <Panel id="gk_toast" class="toast s2-hide">
    <Label id="gk_toast_t" class="toast-t" />
    <Label id="gk_toast_s" class="toast-s" />
  </Panel>
  <KillFeed rows={6} />
  <Button id="gk_close" class="close">
    <Label class="close-t" text="X" />
  </Button>
</Panel>

<style>
  :global(.root) { width: 100%; height: 100%; }
  :global(.toast) { flow-children: down; horizontal-align: center; margin-top: 130px; }
  :global(.toast-t) { font-size: 22px; font-weight: bold; color: #ffffff; }
  :global(.toast-s) { font-size: 14px; color: #aeb5bc; }
  :global(.close) { width: 28px; height: 28px; horizontal-align: right; }
  :global(.close-t) { horizontal-align: center; vertical-align: center; color: #ffffff; }
</style>
