<script lang="ts">
  // A project's page, as the window's: start a new conversation in it, or pick up a past one
  // (or delete one, for good).
  import { command } from "./lib/api";
  import type { ProjectsItem } from "./lib/api-types";
  import { ago } from "./lib/state";
  import Icon from "./Icon.svelte";

  let { project, act }: { project: ProjectsItem; act: (p: Promise<{ agent?: string }>) => Promise<void> } = $props();
  type Past = { session: string; title: string; folder: string; updated: string };
  let past = $state.raw<Past[] | null>(null);
  let find = $state("");
  let problem = $state("");
  async function load() {
    try { past = (await command("past_conversations", { folder: project.id })).conversations ?? []; } catch (e) { problem = (e as Error).message; past = []; }
  }
  $effect(() => { project.id; past = null; load(); });
  const shown = $derived((past ?? []).filter((p) => !find || `${p.title}`.toLowerCase().includes(find.toLowerCase())));
  async function remove(p: Past) {
    if (!confirm(`Delete "${p.title || "this conversation"}" for good? It can't be undone.`)) return;
    try { await command("delete_conversation", { session: p.session }); load(); } catch (e) { problem = (e as Error).message; }
  }
</script>

<section class="project-page" aria-label="{project.name}: its conversations">
  <header class="header">
    <span class="headline"><span class="title">{project.name}</span><span class="place mono">{project.id}</span></span>
    <span class="grow"></span>
    <button class="btn suggested" onclick={() => act(command("new_agent", { folder: project.id }))}><Icon name="plus" size={13} /> New agent here</button>
  </header>
  {#if problem}<p class="error">{problem}</p>{/if}
  <div class="past-list">
    <input class="find" placeholder="Find a past conversation" aria-label="Find a past conversation" bind:value={find} />
    {#each shown as p (p.session)}
      <div class="change-row">
        <span><b>{p.title || "Untitled"}</b><small class="quiet">{ago(p.updated)}</small></span>
        <button class="btn small" onclick={() => act(command("resume", { session: p.session, folder: p.folder, title: p.title }))}>Pick it up</button>
        <button class="tiny" aria-label="Delete it for good" title="Delete it for good" onclick={() => remove(p)}><Icon name="x" size={13} /></button>
      </div>
    {:else}
      <p class="empty">{past === null ? "Reading…" : "No past conversations here."}</p>
    {/each}
  </div>
</section>
