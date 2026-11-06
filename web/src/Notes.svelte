<script lang="ts">
  // Notes: a chat with your notes vault (an agent working in it) and Scribe beside it. The
  // chat open stays open when you go elsewhere; past chats open again from the list.
  import { command, follow, type Item } from "./lib/api";
  import type { AgentsItem } from "./lib/api-types";
  import { ago } from "./lib/state";
  import Conversation from "./Conversation.svelte";
  import Scribe from "./Scribe.svelte";
  import Icon from "./Icon.svelte";
  import Vault from "./Vault.svelte";
  let tab = $state<"chat" | "vault" | "history">("chat");
  type Change = { id: string | null; kind: string; topic: string; reason: string; change: { op?: string; note?: string } | null; title: string; at: number; undone: boolean };
  let history = $state.raw<Change[] | null>(null);
  async function loadHistory() {
    try { history = (await command("scribe_history", { limit: 300 })).changes; } catch (e) { problem = (e as Error).message; history = []; }
  }
  async function undo(c: Change) {
    if (c.id && confirm(`Undo Scribe's change to ${c.change?.note ?? "the note"}?`)) {
      try { await command("scribe_undo", { id: c.id }); } catch (e) { problem = (e as Error).message; }
      loadHistory();
    }
  }

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
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "chat"} class:on={tab === "chat"} onclick={() => (tab = "chat")}>Chat</button>
        <button role="tab" aria-selected={tab === "vault"} class:on={tab === "vault"} onclick={() => (tab = "vault")}>Vault</button>
        <button role="tab" aria-selected={tab === "history"} class:on={tab === "history"} onclick={() => { tab = "history"; loadHistory(); }}>History</button>
      </div>
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
    {#if tab === "vault"}
      <Vault />
    {:else if tab === "history"}
      <div class="nhistory">
        {#each history ?? [] as c, i (c.id ?? i)}
          <div class="change-row" class:undone={c.undone}>
            <span><b>{c.kind === "dropped" ? "Not written" : c.undone ? "Undone" : "Wrote"}: {c.change?.note ?? c.topic}</b><small class="quiet">{c.topic} · {c.reason}{#if c.title}{" · from "}{c.title}{/if} · {ago(c.at)}</small></span>
            {#if c.kind === "written" && !c.undone && c.id}<button class="btn small" onclick={() => undo(c)}>Undo</button>{/if}
          </div>
        {:else}<p class="empty">{history === null ? "Reading…" : "Scribe hasn't written anything yet."}</p>{/each}
      </div>
    {:else if agent}
      {#key agent.id}<Conversation {agent} />{/key}
    {:else}
      <p class="empty centre-note">Ask your notes anything, or tell them something worth keeping. Start with New chat.</p>
    {/if}
  </div>
  <Scribe />
</div>

<svelte:window onclick={() => (listing = false)} />
