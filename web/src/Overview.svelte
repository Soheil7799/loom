<script lang="ts">
  // Every open agent on one page, by what it asks of you: needs you, working, done but not
  // seen, then the quiet ones (smaller); each group the last used first. Beside them, the
  // prompts scheduled to be sent (run now, pause, remove), and Schedule… on each card.
  import { command, follow, type Item } from "./lib/api";
  import type { AgentsItem } from "./lib/api-types";
  import { stateOf, clock } from "./lib/state";
  import Icon from "./Icon.svelte";

  let { agents, pick }: { agents: AgentsItem[]; pick: (id: string) => void } = $props();
  let cards = $state.raw<Item[]>([]);
  let schedules = $state.raw<Item[]>([]);
  let problem = $state("");
  let planning = $state<AgentsItem | null>(null);
  let plan = $state({ prompt: "", when: "", same: true });

  $effect(() => follow("overview", (f) => (cards = f.items)));
  $effect(() => follow("schedules", (f) => (schedules = f.items)));

  const card = (a: AgentsItem) => cards.find((c) => c.id === a.id);
  const byUse = (list: AgentsItem[]) => [...list].sort((x, y) => (y.used ?? "").localeCompare(x.used ?? ""));
  const needs = (a: AgentsItem) => a.questions > 0 || ((a.failed as { times?: number } | undefined)?.times ?? 0) > 0;
  const working = (a: AgentsItem) => !needs(a) && a.busy;
  const unseen = (a: AgentsItem) => !needs(a) && !working(a) && a.unseen;
  const groups = $derived([
    { title: "NEEDS YOU", list: byUse(agents.filter(needs)), quiet: false },
    { title: "WORKING", list: byUse(agents.filter(working)), quiet: false },
    { title: "DONE, NOT SEEN", list: byUse(agents.filter(unseen)), quiet: false },
    { title: "QUIET", list: byUse(agents.filter((a) => !needs(a) && !working(a) && !unseen(a))), quiet: true },
  ].filter((g) => g.list.length));
  const summary = $derived.by(() => {
    const parts = [
      [agents.filter((a) => a.questions > 0).length, "need you"],
      [agents.filter((a) => ((a.failed as { times?: number } | undefined)?.times ?? 0) > 0).length, "stopped"],
      [agents.filter(working).length, "working"],
      [agents.filter(unseen).length, "done, not seen yet"],
    ].filter(([n]) => (n as number) > 0).map(([n, w]) => `${n} ${w}`);
    return !agents.length ? "No agents open. Pick a project in the sidebar to start one." : parts.length ? parts.join(" · ") : `${agents.length} agents, all quiet`;
  });
  const scheduledFor = (a: AgentsItem) => { const s = card(a)?.session; return s ? schedules.filter((x) => x.session === s).length : 0; };
  const target = (s: Item) => (s.session ? s.title : `a new conversation in ${String(s.folder).split("/").filter(Boolean).pop()}`);

  async function run(p: Promise<unknown>) {
    problem = "";
    try { await p; return true; } catch (e) { problem = (e as Error).message; return false; }
  }
  function schedule(a: AgentsItem) {
    planning = a;
    plan = { prompt: "", when: "", same: !!card(a)?.session };
  }
  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (planning && (await run(command("schedule_new", { agent: planning.id, prompt: plan.prompt.trim(), when: plan.when.trim(), same: plan.same })))) planning = null;
  }
</script>

<section class="overview" aria-label="Overview">
  <header class="header"><span class="headline"><span class="title">Overview</span><span class="place">{summary}</span></span></header>
  {#if problem}<p class="error" role="alert">{problem}</p>{/if}
  <div class="overview-split">
    <div class="cards">
      {#each groups as g (g.title)}
        <h3>{g.title}</h3>
        <div class="card-grid" class:compact={g.quiet}>
          {#each g.list as a (a.id)}
            {@const s = stateOf(a)}
            {@const c = card(a)}
            {@const n = scheduledFor(a)}
            <article class="ocard" class:needs={g.title === "NEEDS YOU"}>
              <button class="ohead" onclick={() => pick(a.id)}>
                <i class="st {s.shape}" role="img" aria-label={s.word}></i><b>{a.title || "New agent"}</b>
                <span class="w-{s.shape}">{s.word}</span>
              </button>
              <p class="where">{a.project}{#if a.place}{" · "}{a.place}{/if}{#if n}{" · "}<Icon name="clock" size={11} /> {n}{/if}</p>
              {#each (c?.waiting ?? []) as w, i (i)}<p class="wants">Wants to: {w}</p>{/each}
              {#if !g.quiet && c?.reply}<p class="reply">{c.reply}</p>{/if}
              <div class="controls"><button class="btn small" onclick={() => pick(a.id)}>Open</button><button class="btn small" title="Send this agent a prompt at a time you pick" onclick={() => schedule(a)}>Schedule…</button></div>
            </article>
          {/each}
        </div>
      {/each}
    </div>
    <aside class="scheduled" aria-label="Scheduled prompts">
      <h3>SCHEDULED PROMPTS</h3>
      {#each schedules as sc (sc.id)}
        <div class="srow" class:paused={sc.paused}>
          <p class="sprompt">{sc.prompt}</p>
          <p class="quiet small">{target(sc)} · {sc.when}{#if sc.paused}{" · paused"}{:else if sc.next}{" · next "}{clock(sc.next)}{/if}</p>
          <div class="controls">
            <button class="btn small" title="Send it now (the schedule stays)" onclick={() => run(command("schedule_act", { id: sc.id, act: "run_now" }))}>Run now</button>
            <button class="btn small" onclick={() => run(command("schedule_act", { id: sc.id, act: "pause" }))}>{sc.paused ? "Go on" : "Pause"}</button>
            <button class="btn small deny" onclick={() => confirm("Remove this scheduled prompt?") && run(command("schedule_act", { id: sc.id, act: "remove" }))}>Remove</button>
          </div>
        </div>
      {:else}
        <p class="quiet small">None yet. On a card, Schedule… sends that agent a prompt at a time you pick: every day at 9, every 2h, in 20m.</p>
      {/each}
    </aside>
  </div>

  {#if planning}
    <div class="scrim" role="presentation" onclick={() => (planning = null)}></div>
    <div class="plan" role="dialog" aria-modal="true" aria-label="Schedule a prompt" tabindex="-1" onkeydown={(e) => e.key === "Escape" && (planning = null)}>
    <form onsubmit={save}>
      <h3>Schedule a prompt for {planning.title || "this agent"}</h3>
      <label>Prompt<!-- svelte-ignore a11y_autofocus --><textarea rows="4" bind:value={plan.prompt} autofocus></textarea></label>
      <label>When<input placeholder="09:00, weekdays 9:30, every 2h, in 20m…" bind:value={plan.when} /></label>
      <label class="check"><input type="checkbox" bind:checked={plan.same} disabled={!card(planning)?.session} /> Continue this conversation (off: a new one each time)</label>
      <div class="controls"><span class="grow"></span><button type="button" class="btn" onclick={() => (planning = null)}>Cancel</button><button class="btn suggested" disabled={!plan.prompt.trim() || !plan.when.trim()}>Schedule</button></div>
    </form>
    </div>
  {/if}
</section>
