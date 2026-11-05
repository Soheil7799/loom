<script lang="ts">
  // A picture of the conversation's: small here, whole in a dialog when clicked.
  import { blobUrl } from "./lib/pictures";

  let { id }: { id: string } = $props();
  let url = $state("");
  let failed = $state(false);
  let whole: HTMLDialogElement | undefined = $state();
  $effect(() => { blobUrl(id).then((u) => (url = u), () => (failed = true)); });
</script>

{#if url}
  <button class="pic" aria-label="Show the picture whole" onclick={() => whole?.showModal()}><img src={url} alt="" /></button>
  <dialog class="whole" bind:this={whole} onclick={() => whole?.close()}><img src={url} alt="" /></dialog>
{:else}
  <span class="pic loading">{failed ? "gone" : "…"}</span>
{/if}
