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
  import TopicView from "./TopicView.svelte";
  // a note opened from a topic: the vault shows it
  let vaultNote = $state<string | null>(null);
  let tab = $state<"chat" | "vault" | "history" | "topics">("chat");
  // "Today", "Yesterday", else the date: the past chats by day, newest first
  const day = (when?: string) => {
    if (!when) return "Earlier";
    const d = new Date(when), n = new Date();
    const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
    if (same(d, n)) return "Today";
    if (same(d, new Date(n.getTime() - 864e5))) return "Yesterday";
    return d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" });
  };
  // Topics: each one's description and notes, and a chat about it
  type Topic = { name: string; path: string; folder: string; listens_for: string[]; enabled: boolean };
  let topics = $state.raw<Topic[] | null>(null);
  let vault = $state("");
  let topic = $state<Topic | null>(null);
  async function loadTopics() {
    try { const got = await command("topics", {}); topics = got.topics; vault = got.vault; } catch (e) { problem = (e as Error).message; topics = []; }
  }
  async function aboutTopic(t: Topic) {
    try { const got = await command("notes_open", { topic: t.name }); chat = { agent: got.agent, title: t.name }; tab = "chat"; } catch (e) { problem = (e as Error).message; }
  }
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
    <div class="notes-bar header">
      <span class="headline"><span class="title">Notes</span></span>
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "chat"} class:on={tab === "chat"} onclick={() => (tab = "chat")}>Chat</button>
        <button role="tab" aria-selected={tab === "vault"} class:on={tab === "vault"} onclick={() => (tab = "vault")}>Vault</button>
        <button role="tab" aria-selected={tab === "topics"} class:on={tab === "topics"} onclick={() => { tab = "topics"; loadTopics(); }}>Topics</button>
        <button role="tab" aria-selected={tab === "history"} class:on={tab === "history"} onclick={() => { tab = "history"; loadHistory(); }}>History</button>
      </div>
      <button class="btn" onclick={() => open()}><Icon name="plus" size={13} /> New chat</button>
      <div class="inbox-wrap">
        <button class="btn" aria-expanded={listing} disabled={!chats.length} onclick={(e) => { e.stopPropagation(); listing = !listing; }}>Past chats <Icon name="chev-d" size={13} /></button>
        {#if listing}
          <div class="menu">
            {#each chats as c, i (c.id)}
              {#if i === 0 || day(c.updated) !== day(chats[i - 1].updated)}<p class="foot day">{day(c.updated)}</p>{/if}
              <button class="it" onclick={() => open(c.id, c.title)}><Icon name="back" size={13} /><span>{c.title || "Untitled"}</span><small>{ago(c.updated)}{#if c.current} · open{/if}</small></button>
            {/each}
          </div>
        {/if}
      </div>
      {#if problem}<span class="error">{problem}</span>{/if}
    </div>
    {#if tab === "vault"}
      {#key vaultNote}<Vault start={vaultNote ? vaultNote.split("/").slice(0, -1).join("/") : ""} note={vaultNote} />{/key}
    {:else if tab === "topics"}
      <div class="vault">
        <nav class="vault-list" aria-label="Topics">
          {#each topics ?? [] as t (t.name)}
            <button class="trow" class:sel={topic?.name === t.name} onclick={() => (topic = t)}><Icon name="notes" size={13} /><span>{t.name}</span>{#if !t.enabled}<small class="quiet">off</small>{/if}</button>
          {:else}<p class="quiet small">{topics === null ? "Reading…" : "No topics yet (Settings → Notes)."}</p>{/each}
        </nav>
        <article class="note">
          {#if topic}
            <div class="controls"><h3 class="tname">{topic.name}</h3><span class="grow"></span><button class="btn small suggested" onclick={() => aboutTopic(topic!)}>Chat about it</button></div>
            <p class="quiet small">Its notes in {topic.folder}/ · listens for {topic.listens_for.join(", ") || "nothing yet"}</p>
            <TopicView file={topic.path.startsWith(vault + "/") ? topic.path.slice(vault.length + 1) : null} folder={topic.folder} openNote={(p) => { vaultNote = p; tab = "vault"; }} />
          {:else}<p class="empty">Pick a topic: its description, its notes, a chat about it.</p>{/if}
        </article>
      </div>
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

<svelte:window onclick={() => (listing = false)} onkeydown={(e) => e.key === "Escape" && (listing = false)} />
