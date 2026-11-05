<script lang="ts">
  // What agents ask beyond their conversation, where no window of loom's shows it: a file
  // to see, one to fill in (the agent waits), a word. Newest first.
  import { command } from "./lib/api";
  import type { AsksItem } from "./lib/api-types";
  import { ago } from "./lib/state";

  let { asks, agents, pick }: { asks: AsksItem[]; agents: { id: string; title: string }[]; pick: (id: string) => void } = $props();
  let problem = $state("");

  async function run(p: Promise<unknown>) {
    problem = "";
    try { await p; } catch (e) { problem = (e as Error).message; }
  }
  const what: Record<string, string> = { open: "wants you to see a file", edit: "waits for you to fill in a file", notify: "needs you" };
  const who = (id: string) => agents.find((a) => a.id === id);
</script>

<section class="asks-page" aria-label="Asks">
  <header class="header"><span class="headline"><span class="title">Asks</span></span></header>
  <div class="asks-list">
    {#if problem}<p class="error">{problem}</p>{/if}
    {#each [...asks].reverse() as ask (ask.id)}
      {@const a = who(ask.agent)}
      <div class="permission-card" class:answered={ask.kind !== "edit"}>
        <div class="qh">
          {#if ask.kind === "edit"}<span class="needs">waits for you</span>{/if}
          <b>{ask.title}</b><span class="quiet">{what[ask.kind] ?? ask.kind} · {ago(ask.at)}</span>
        </div>
        {#if ask.message}<p>{ask.message}</p>{/if}
        {#if ask.path}<pre class="code-block">{ask.path}</pre>{/if}
        <div class="controls">
          {#if ask.path}<button class="btn" onclick={() => run(command("ask_open", { id: ask.id }))}>Open in my editor</button>{/if}
          <button class="btn" class:suggested={ask.kind === "edit"} onclick={() => run(command("ask_done", { id: ask.id }))}>{ask.kind === "edit" ? "Done: the agent goes on" : "Dismiss"}</button>
          {#if a}<button class="btn" onclick={() => pick(a.id)}>Go to {a.title || "the agent"}</button>{/if}
        </div>
      </div>
    {:else}
      <p class="empty">Nothing asked.</p>
    {/each}
  </div>
</section>
