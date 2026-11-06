<script lang="ts">
  // The notes vault, read here: its folders (`vault/<folder>`), a note rendered (`note/<path>`)
  // with what it links to and what links to it. Writing stays Scribe's and yours elsewhere.
  import { follow, type Item } from "./lib/api";
  import { markdown } from "./lib/markdown";
  import Icon from "./Icon.svelte";

  let folder = $state("");
  let notePath = $state<string | null>(null);
  let entries = $state.raw<Item[]>([]);
  let note = $state.raw<Item | null>(null);

  $effect(() => follow(`vault/${folder}`, (f) => (entries = f.items)));
  $effect(() => { if (notePath) return follow(`note/${notePath}`, (f) => (note = f.items[0] ?? null)); note = null; });
  const crumbs = $derived(folder.split("/").filter(Boolean));
  const up = (n: number) => { folder = crumbs.slice(0, n).join("/"); notePath = null; };
  type Link = { target?: string; title?: string; path?: string | null };
  const open = (l: Link) => { if (l.path) notePath = l.path; };
  // shown as read: the front matter left out (its tags show above), [[links]] as their names
  const body = (text: string) => text.replace(/^---\n[\s\S]*?\n---\n/, "").replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_, target, alias) => `**${alias ?? target}**`);
</script>

<div class="vault">
  <nav class="vault-list" aria-label="The vault's notes">
    <p class="crumbs"><button class="link" onclick={() => up(0)}>vault</button>{#each crumbs as c, i (i)} / <button class="link" onclick={() => up(i + 1)}>{c}</button>{/each}</p>
    {#each entries as e (e.id)}
      {#if e.kind === "error"}<p class="error">{e.message}</p>
      {:else}
        <button class="trow" class:sel={notePath === e.path} onclick={() => (e.kind === "folder" ? ((folder = e.path), (notePath = null)) : (notePath = e.path))}>
          <Icon name={e.kind === "folder" ? "folder" : "notes"} size={13} /><span>{e.kind === "note" ? (e.title || e.name) : e.name}</span>{#if e.kind === "folder"}<small class="quiet">{e.count}</small>{/if}
        </button>
      {/if}
    {:else}<p class="quiet small">Empty.</p>{/each}
  </nav>
  <article class="note">
    {#if note?.id === "error"}<p class="error">{note.message}</p>
    {:else if note}
      <p class="quiet small mono">{note.path}</p>
      {#if note.tags?.length}<p class="tags">{#each note.tags as t (t)}<span>#{t}</span>{/each}</p>{/if}
      <div class="md">{@html markdown(body(note.text ?? ""))}</div>
      {#if note.links?.length}<h3>Links to</h3><p class="links">{#each note.links as l, i (i)}<button class="link" disabled={!l.path} title={l.path ? l.path : "no such note yet"} onclick={() => open(l)}>{l.target ?? l.title ?? l.path}</button>{/each}</p>{/if}
      {#if note.backlinks?.length}<h3>Linked from</h3><p class="links">{#each note.backlinks as l, i (i)}<button class="link" onclick={() => open(l)}>{l.title ?? l.target ?? l.path}</button>{/each}</p>{/if}
    {:else}<p class="empty">Pick a note to read it.</p>{/if}
  </article>
</div>
