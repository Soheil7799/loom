<script lang="ts">
  // A folder picked by walking down from home (`dirs_list`): the page can't ask the desktop.
  import { command } from "./lib/api";
  import Icon from "./Icon.svelte";

  let { pick, close }: { pick: (folder: string) => void; close: () => void } = $props();
  let at = $state<{ folder: string; parent: string | null; dirs: string[] } | null>(null);
  let problem = $state("");
  async function go(folder?: string) {
    try { at = await command("dirs_list", folder ? { folder } : {}); problem = ""; } catch (e) { problem = (e as Error).message; }
  }
  go();
</script>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="finder picker" role="dialog" aria-modal="true" aria-label="Pick a project's folder" tabindex="-1" onkeydown={(e) => e.key === "Escape" && close()}>
  {#if at}
    <p class="crumbs mono">{at.folder}</p>
    <div class="controls">
      {#if at.parent}<button class="btn small" onclick={() => go(at!.parent!)}><Icon name="chev-u" size={12} /> Up</button>{/if}
      <span class="grow"></span>
      <button class="btn small suggested" onclick={() => pick(at!.folder)}>This folder</button>
    </div>
    {#each at.dirs as d (d)}<button class="trow" ondblclick={() => pick(`${at!.folder}/${d}`)} onclick={() => go(`${at!.folder}/${d}`)}><Icon name="folder" size={13} /><span>{d}</span></button>{/each}
  {/if}
  {#if problem}<p class="error">{problem}</p>{/if}
</div>
