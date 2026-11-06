<script lang="ts">
  // Start an agent, as the window's New agent: in a project, in another folder, a
  // conversation closed lately (reopen) or one of Claude Code's past ones (resume).
  import { command, follow, type Item } from "./lib/api";
  import type { ProjectsItem } from "./lib/api-types";
  import { ago } from "./lib/state";
  import Icon from "./Icon.svelte";
  import FolderPicker from "./FolderPicker.svelte";

  let { projects, close, act }: { projects: ProjectsItem[]; close: () => void; act: (p: Promise<{ agent?: string }>) => Promise<void> } = $props();

  type Past = { session: string; title: string; folder: string; updated: string };
  let folder = $state("");
  let closed = $state.raw<Item[]>([]);
  let past = $state.raw<Past[] | null>(null);
  let looking = $state(false);
  let find = $state("");
  let problem = $state("");
  let browsing = $state(false);

  $effect(() => follow("closed", (f) => (closed = f.items)));

  const name = (path: string) => path.split("/").filter(Boolean).pop() ?? path;
  const found = $derived((past ?? []).filter((p) => !find || `${p.title} ${p.folder}`.toLowerCase().includes(find.toLowerCase())).slice(0, 8));

  async function lookBack() {
    looking = true;
    problem = "";
    try { past = (await command("past_conversations", {})).conversations ?? []; } catch (e) { problem = (e as Error).message; }
    looking = false;
  }
  function inFolder(e: SubmitEvent) {
    e.preventDefault();
    if (folder.trim()) act(command("new_agent", { folder: folder.trim() }));
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && close()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="start" role="dialog" aria-label="Start an agent" tabindex="-1" onclick={(e) => e.stopPropagation()}>
  <div class="sh"><b>Start an agent</b><button class="tiny" aria-label="Close" onclick={close}><Icon name="x" size={14} /></button></div>

  {#if projects.length}
    <h3>In a project</h3>
    {#each projects as p (p.id)}
      <button class="it" title={p.id} onclick={() => act(command("new_agent", { folder: p.id }))}><Icon name="plus" size={13} /><span>{p.name}</span></button>
    {/each}
  {/if}

  <h3>In a folder</h3>
  <form class="adding" onsubmit={inFolder}>
    <!-- svelte-ignore a11y_autofocus -->
    <input aria-label="Project folder" placeholder="/path/to/project" bind:value={folder} autofocus={!projects.length} />
    <button type="button" class="btn small" onclick={() => (browsing = true)}>Browse…</button>
    <button class="go" disabled={!folder.trim()}>Start</button>
  </form>

  {#if closed.length}
    <h3>Closed lately</h3>
    {#each closed.slice(0, 5) as c (c.id)}
      <button class="it" onclick={() => act(command("reopen", { session: c.id }))}><Icon name="undo" size={13} /><span>{c.title || "Untitled"}</span><small>{name(c.folder)} · {ago(c.used)}</small></button>
    {/each}
  {/if}

  <h3>Past conversations</h3>
  {#if past === null}
    <button class="it" disabled={looking} onclick={lookBack}><Icon name="back" size={13} /><span>{looking ? "Looking…" : "Show past conversations"}</span></button>
  {:else}
    <input class="find" aria-label="Find a past conversation" placeholder="Find by title or folder" bind:value={find} />
    {#each found as p (p.session)}
      <button class="it" title={p.folder} onclick={() => act(command("resume", { session: p.session, folder: p.folder, title: p.title }))}><Icon name="back" size={13} /><span>{p.title || "Untitled"}</span><small>{name(p.folder)} · {ago(p.updated)}</small></button>
    {:else}
      <p class="quiet small">{past.length ? "None matches." : "None found."}</p>
    {/each}
  {/if}
  {#if problem}<p class="error">{problem}</p>{/if}
</div>
{#if browsing}<FolderPicker pick={(f) => { browsing = false; folder = f; }} close={() => (browsing = false)} />{/if}
