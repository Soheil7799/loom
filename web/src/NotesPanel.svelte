<script lang="ts">
  // The Notes view's right panel, as the window's: Topics (each topic with its notes, newest
  // first; its description and a chat about it a click away) and History (Scribe: its
  // questions, what waits for your OK, and what it wrote, to undo).
  import { command } from "./lib/api";
  import { ago } from "./lib/state";
  import Icon from "./Icon.svelte";
  import Scribe from "./Scribe.svelte";
  import TopicView from "./TopicView.svelte";

  let { openNote, chatAbout }: { openNote: (path: string) => void; chatAbout: (topic: string) => void } = $props();
  let tab = $state<"topics" | "history">("topics");
  let problem = $state("");
  type Topic = { name: string; path: string; folder: string; listens_for: string[]; enabled: boolean };
  let topics = $state.raw<Topic[] | null>(null);
  let vault = $state("");
  let unfolded = $state<string | null>(null);
  async function loadTopics() {
    try { const got = await command("topics", {}); topics = got.topics; vault = got.vault; problem = ""; } catch (e) { problem = (e as Error).message; topics = []; }
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
  $effect(() => { if (tab === "topics") loadTopics(); else loadHistory(); });
</script>

<aside class="panel" aria-label="Your notes">
  <header class="panel-head">
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === "topics"} class:on={tab === "topics"} title="Your topics and their notes, newest first" onclick={() => (tab = "topics")}>Topics</button>
      <button role="tab" aria-selected={tab === "history"} class:on={tab === "history"} title="Scribe's questions, changes waiting for your OK, what was written" onclick={() => (tab = "history")}>History</button>
    </div>
  </header>
  {#if problem}<p class="error" role="alert">{problem}</p>{/if}
  <div class="panel-body">
    {#if tab === "topics"}
      {#each topics ?? [] as t (t.name)}
        <button class="trow topic-row" aria-expanded={unfolded === t.name} onclick={() => (unfolded = unfolded === t.name ? null : t.name)}><Icon name={unfolded === t.name ? "chev-d" : "chev-r"} size={12} /><span>{t.name}</span>{#if !t.enabled}<small class="quiet">off</small>{/if}</button>
        {#if unfolded === t.name}
          <div class="topic-open">
            <div class="controls"><span class="quiet small">Its notes in {t.folder}/</span><span class="grow"></span><button class="btn small suggested" onclick={() => chatAbout(t.name)}>Chat about it</button></div>
            <TopicView file={t.path.startsWith(vault + "/") ? t.path.slice(vault.length + 1) : null} folder={t.folder} {openNote} />
          </div>
        {/if}
      {:else}
        <p class="empty small">{topics === null ? "Reading…" : "No topics yet (Settings → Notes)."}</p>
      {/each}
    {:else}
      <Scribe />
      <p class="caption">Written</p>
      {#each history ?? [] as c, i (c.id ?? i)}
        <div class="change-row" class:undone={c.undone}>
          <span><b>{c.kind === "dropped" ? "Not written" : c.undone ? "Undone" : "Wrote"}: {c.change?.note ?? c.topic}</b><small class="quiet">{c.topic} · {c.reason}{#if c.title}{" · from "}{c.title}{/if} · {ago(c.at)}</small></span>
          {#if c.kind === "written" && !c.undone && c.id}<button class="btn small" onclick={() => undo(c)}>Undo</button>{/if}
        </div>
      {:else}<p class="empty small">{history === null ? "Reading…" : "Scribe hasn't written anything yet."}</p>{/each}
    {/if}
  </div>
</aside>
