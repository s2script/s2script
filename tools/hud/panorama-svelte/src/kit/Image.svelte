<script lang="ts">
  let { id, class: cls, src, hittest = false }: { id?: string; class?: string; src?: string; hittest?: boolean } = $props();
</script>

<hud-image {id} class={cls} hittest={hittest ? "true" : "false"} src={src ?? `{s:${id}}`}></hud-image>
