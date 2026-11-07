<script lang="ts">
  // Notes history, in the middle as the window's "Full history": everything Scribe and the
  // Notes chat wrote in your notes, newest first, to undo; what you said no to on request.
  import { command } from "./lib/api";
  import { ago } from "./lib/state";
  import Icon from "./Icon.svelte";

  let { close }: { close: () => void } = $props();
  type Change = { id: string | null; kind: string; topic: string; reason: string; change: { op?: string; note?: string } | null; title: string; at: number; undone: boolean };
  let changes = $state.raw<Change[] | null>(null);
  let dropped = $state(false);
  let find = $state("");
  let problem = $state("");
  async function load() {
    try { changes = (await command("scribe_history", { limit: 2000 })).changes; problem = ""; } catch (e) { problem = (e as Error).message; changes = []; }
  }
  $effect(() => { load(); });
  const shown = $derived((changes ?? []).filter((c) => (dropped || c.kind !== "dropped") && (!find.trim() || `${c.change?.note ?? ""} ${c.topic} ${c.reason} ${c.title}`.toLowerCase().includes(find.trim().toLowerCase()))));
  async function undo(c: Change) {
    if (c.id && confirm(`Undo the change to ${c.change?.note ?? "the note"}?`)) {
      try { await command("scribe_undo", { id: c.id }); } catch (e) { problem = (e as Error).message; }
      load();
    }
  }
</script>

<section class="notes-history" aria-label="Notes history">
  <header class="header"><button class="tiny" aria-label="Back to the chat" title="Back to the chat" onclick={close}><Icon name="back" size={15} /></button><span class="headline"><span class="title">Notes history</span></span></header>
  <div class="history-body">
    <p class="quiet">Everything Scribe and the Notes chat wrote in your notes, newest first. Undo puts a note back as it was; if it changed since, only that change's lines are taken out.</p>
    <div class="controls"><input class="find" aria-label="Find a change" placeholder="Find by note, topic or reason" bind:value={find} /><label class="quiet small"><input type="checkbox" bind:checked={dropped} /> Show what you said no to</label></div>
    {#if problem}<p class="error">{problem}</p>{/if}
    {#each shown as c, i (c.id ?? i)}
      <div class="change-row" class:undone={c.undone || c.kind === "dropped"}>
        <span><b>{c.change?.note ?? c.topic}</b><small class="quiet">{c.kind === "dropped" ? "not written · " : c.undone ? "undone · " : ""}{c.topic} · {c.reason}{#if c.title}{" · from "}{c.title}{/if} · {ago(c.at)}</small></span>
        {#if c.kind === "written" && !c.undone && c.id}<button class="btn small" onclick={() => undo(c)}>Undo</button>{/if}
      </div>
    {:else}<p class="empty">{changes === null ? "Reading…" : changes.length ? "No change matches." : "Nothing written yet."}</p>{/each}
  </div>
</section>
