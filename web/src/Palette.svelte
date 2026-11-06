<script lang="ts">
  // Every action of the page with its keys (Ctrl+K), filtered as you type; Enter or a click
  // does it. Ctrl+/ opens it too: the list of keys.
  export type Action = { name: string; keys?: string; run: () => void; when?: () => boolean };
  let { actions, close }: { actions: Action[]; close: () => void } = $props();
  let query = $state("");
  let at = $state(0);
  const shown = $derived(actions.filter((a) => (!a.when || a.when()) && a.name.toLowerCase().includes(query.trim().toLowerCase())));
  $effect(() => { query; at = 0; });
  // the chosen row in view as the arrows move it
  $effect(() => { document.getElementById(`act-${at}`)?.scrollIntoView({ block: "nearest" }); });
  function go(a: Action | undefined) {
    if (!a) return;
    close();
    a.run();
  }
  function keys(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); close(); }
    else if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); at = (at + (e.key === "ArrowDown" ? 1 : shown.length - 1)) % Math.max(shown.length, 1); }
    else if (e.key === "Enter") { e.preventDefault(); go(shown[at]); }
  }
</script>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="finder palette" role="dialog" aria-modal="true" aria-label="Actions and their keys">
  <!-- svelte-ignore a11y_autofocus -->
  <input placeholder="Find an action" bind:value={query} onkeydown={keys} autofocus aria-label="Find an action" role="combobox" aria-expanded="true" aria-controls="actions" aria-activedescendant={shown.length ? `act-${at}` : undefined} />
  <div id="actions" role="listbox" aria-label="Actions">
    {#each shown as a, i (a.name)}
      <div id="act-{i}" role="option" aria-selected={i === at} class="trow" class:sel={i === at} tabindex="-1" onclick={() => go(a)} onkeydown={(e) => e.key === "Enter" && go(a)}><span>{a.name}</span>{#if a.keys}<kbd>{a.keys}</kbd>{/if}</div>
    {:else}
      <p class="quiet small">Nothing like that.</p>
    {/each}
  </div>
</div>
