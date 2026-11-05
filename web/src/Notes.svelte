<script lang="ts">
  // Notes: a chat with your notes vault (an agent working in it) and Scribe beside it. The
  // chat open stays open when you go elsewhere; past chats open again from the list.
  import { command, follow, type Item } from "./lib/api";
  import type { AgentsItem } from "./lib/api-types";
  import { ago } from "./lib/state";
  import Conversation from "./Conversation.svelte";
  import Scribe from "./Scribe.svelte";
  import Icon from "./Icon.svelte";

  let { chat = $bindable(null) }: { chat: { agent: string; title: string } | null } = $props();
  let chats = $state.raw<Item[]>([]);
  let listing = $state(false);
  let problem = $state("");

  $effect(() => follow("notes_chats", (f) => (chats = f.items)));

  async function open(session?: string, title = "Notes") {
    problem = "";
    listing = false;
    try {
      const answer = await command("notes_open", session ? { session } : {});
      chat = { agent: answer.agent, title };
    } catch (e) { problem = (e as Error).message; }
  }

  // the chat as Conversation draws an agent: a Notes chat isn't among the agents
  const agent = $derived(chat && ({ id: chat.agent, title: chat.title, project: "your notes", project_id: "", status: "notes", busy: false, questions: 0, unseen: false, used: "", place: "", edited: 0, awake: true } as AgentsItem));
</script>

<div class="notes">
  <div class="notes-chat">
    <div class="notes-bar">
      <button class="btn" onclick={() => open()}><Icon name="plus" size={13} /> New chat</button>
      <div class="inbox-wrap">
        <button class="btn" aria-haspopup="menu" aria-expanded={listing} disabled={!chats.length} onclick={(e) => { e.stopPropagation(); listing = !listing; }}>Past chats <Icon name="chev-d" size={13} /></button>
        {#if listing}
          <div class="menu" role="menu">
            {#each chats as c (c.id)}
              <button class="it" role="menuitem" onclick={() => open(c.id, c.title)}><Icon name="back" size={13} /><span>{c.title || "Untitled"}</span><small>{ago(c.updated)}{#if c.current} · open{/if}</small></button>
            {/each}
          </div>
        {/if}
      </div>
      {#if problem}<span class="error">{problem}</span>{/if}
    </div>
    {#if agent}
      {#key agent.id}<Conversation {agent} />{/key}
    {:else}
      <p class="empty centre-note">Ask your notes anything, or tell them something worth keeping. Start with New chat.</p>
    {/if}
  </div>
  <Scribe />
</div>

<svelte:window onclick={() => (listing = false)} />
