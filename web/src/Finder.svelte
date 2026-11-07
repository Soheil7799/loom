<script lang="ts">
  // Ctrl+P: a file of the project found by its name, opened beside the chat.
  import { command } from "./lib/api";

  let { project, pick, close }: { project: string; pick: (path: string) => void; close: () => void } = $props();
  let query = $state("");
  let found = $state<string[]>([]);
  let problem = $state("");
  async function find() {
    try { found = query.trim() ? (await command("files_find", { project, query, limit: 30 })).files ?? [] : []; problem = ""; } catch (e) { problem = (e as Error).message; }
  }
  const go = (f: string) => { close(); pick(`${project}/${f}`); };
</script>

<div class="scrim top" role="presentation" onclick={close}></div>
<div class="finder" role="dialog" aria-modal="true" aria-label="Find a file">
  <!-- svelte-ignore a11y_autofocus -->
  <input placeholder="A file's name…" bind:value={query} oninput={find} autofocus onkeydown={(e) => { if (e.key === "Enter" && found[0]) go(found[0]); if (e.key === "Escape") close(); }} />
  {#if problem}<p class="error">{problem}</p>{/if}
  {#each found as f (f)}<button class="trow" onclick={() => go(f)}>{f}</button>{/each}
</div>
