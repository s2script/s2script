<script lang="ts">
  // `text` is fixed text. Without it the Label is driven by the server: its dialog variable is
  // named after its id, which the runtime relies on.
  let { id, class: cls, text, hittest = false }: { id?: string; class?: string; text?: string; hittest?: boolean } = $props();
</script>

<hud-label {id} class={cls} hittest={hittest ? "true" : "false"} text={text ?? `{s:${id}}`}></hud-label>
