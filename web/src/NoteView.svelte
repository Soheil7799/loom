<script lang="ts">
  // A note of the vault, beside the Notes chat (`note/<path>`): rendered, with what it links
  // to and what links to it (a click opens that one here). Writing stays Scribe's and yours.
  import { follow, type Item } from "./lib/api";
  import { markdown } from "./lib/markdown";
  import Icon from "./Icon.svelte";

  let { path = $bindable(null) }: { path: string | null } = $props();
  let note = $state.raw<Item | null>(null);
  $effect(() => { if (path) return follow(`note/${path}`, (f) => (note = f.items[0] ?? null)); note = null; });
  type Link = { target?: string; title?: string; path?: string | null };
  // shown as read: the front matter left out (its tags show above), [[links]] as their names
  const body = (text: string) => text.replace(/^---\n[\s\S]*?\n---\n/, "").replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_, target, alias) => `**${alias ?? target}**`);
  const go = (l: Link) => { if (l.path) path = l.path; };
</script>

{#if path}
  <section class="editor-pane note-pane" aria-label="A note">
    <div class="ebar"><code title={path}>{path}</code><span class="grow"></span><button class="tiny" aria-label="Close the note" title="Close" onclick={() => (path = null)}><Icon name="x" size={14} /></button></div>
    <article class="note">
      {#if note?.id === "error"}<p class="error">{note.message}</p>
      {:else if note}
        {#if note.tags?.length}<p class="tags">{#each note.tags as t (t)}<span>#{t}</span>{/each}</p>{/if}
        <div class="md">{@html markdown(body(note.text ?? ""))}</div>
        {#if note.links?.length}<h3>Links to</h3><p class="links">{#each note.links as l, i (i)}<button class="link" disabled={!l.path} title={l.path ? l.path : "no such note yet"} onclick={() => go(l)}>{l.target ?? l.title ?? l.path}</button>{/each}</p>{/if}
        {#if note.backlinks?.length}<h3>Linked from</h3><p class="links">{#each note.backlinks as l, i (i)}<button class="link" onclick={() => go(l)}>{l.title ?? l.target ?? l.path}</button>{/each}</p>{/if}
      {:else}<p class="quiet">Reading…</p>{/if}
    </article>
  </section>
{/if}
