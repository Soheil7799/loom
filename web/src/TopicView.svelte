<script lang="ts">
  // One topic: its description (its file, rendered), and its notes, newest first.
  import { follow, type Item } from "./lib/api";
  import { markdown } from "./lib/markdown";
  import { ago } from "./lib/state";
  import Icon from "./Icon.svelte";

  let { file, folder, openNote }: { file: string | null; folder: string; openNote: (path: string) => void } = $props();
  let about = $state.raw<Item | null>(null);
  let notes = $state.raw<Item[]>([]);
  $effect(() => { if (file) return follow(`note/${file}`, (f) => (about = f.items[0] ?? null)); about = null; });
  $effect(() => follow(`vault/${folder}`, (f) => (notes = f.items.filter((i) => i.kind === "note").sort((a, b) => (b.modified ?? 0) - (a.modified ?? 0)))));
  const body = (t: string) => t.replace(/^---\n[\s\S]*?\n---\n/, "");
</script>

{#if about?.text}<div class="md topic-about">{@html markdown(body(about.text))}</div>{/if}
<h3>Its notes</h3>
{#each notes as n (n.id)}
  <button class="trow" onclick={() => openNote(n.path)}><Icon name="notes" size={13} /><span>{n.title || n.name}</span><small class="quiet">{n.modified ? ago(Math.round(n.modified > 1e12 ? n.modified / 1000 : n.modified)) : ""}</small></button>
{:else}
  <p class="quiet small">None yet: Scribe writes them here from your chats.</p>
{/each}
