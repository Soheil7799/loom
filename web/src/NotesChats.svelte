<script lang="ts">
  // The Notes view's sidebar page, as the window's: + New chat, the past chats by day (newest
  // first), and the way back to the agents (with how many work and wait meanwhile).
  import { follow, type Item } from "./lib/api";
  import type { AgentsItem } from "./lib/api-types";
  import { ago } from "./lib/state";
  import Icon from "./Icon.svelte";

  let { agents, chat, open, back }: { agents: AgentsItem[]; chat: { agent: string; title: string } | null; open: (session?: string, title?: string) => void; back: () => void } = $props();
  let chats = $state.raw<Item[]>([]);
  $effect(() => follow("notes_chats", (f) => (chats = f.items)));
  // "Today", "Yesterday", else the date
  const day = (when?: string) => {
    if (!when) return "Earlier";
    const d = new Date(when), n = new Date();
    const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
    if (same(d, n)) return "Today";
    if (same(d, new Date(n.getTime() - 864e5))) return "Yesterday";
    return d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" });
  };
  const working = $derived(agents.filter((a) => a.busy).length);
  const waiting = $derived(agents.filter((a) => a.questions > 0).length);
</script>

<div class="notes-side">
  <p class="heading">NOTES</p>
  <button class="arow new-chat" title="A new chat with your notes" onclick={() => open()}><Icon name="plus" size={13} /><span class="t">New chat</span></button>
  <div class="list">
    {#each chats as c, i (c.id)}
      {#if i === 0 || day(c.updated) !== day(chats[i - 1].updated)}<p class="notes-day">{day(c.updated)}</p>{/if}
      <button class="arow" class:sel={!!c.current && chat?.title === c.title} onclick={() => open(c.id, c.title)}><span class="t">{c.title || "Untitled"}</span><span class="s">{ago(c.updated)}{#if c.current} · open{/if}</span></button>
    {:else}
      <p class="empty small">Your chats with your notes show here.</p>
    {/each}
  </div>
  <button class="notes-back" title="Back to the agents (Alt+M)" onclick={back}>← Agents{#if working}{" · "}{working} working{/if}{#if waiting}{" · "}{waiting} {waiting === 1 ? "waits" : "wait"}{/if}</button>
</div>
