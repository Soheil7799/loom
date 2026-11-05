<script lang="ts">
  // The sidebar, as the GTK window's: the wordmark, what waits for you, the open agents
  // (numbered: Ctrl+1-9), the projects with their agents, and the bottom bar.
  import type { AgentsItem, ProjectsItem } from "./lib/api-types";
  import { stateOf } from "./lib/state";
  import Icon from "./Icon.svelte";
  import wordmarkDark from "../../data/logo/wordmark-dark.svg?url";
  import wordmarkLight from "../../data/logo/wordmark-light.svg?url";

  let {
    agents,
    projects,
    selected,
    pick,
    view,
    show,
    newAgent,
  }: {
    agents: AgentsItem[];
    projects: ProjectsItem[];
    selected: string | null;
    pick: (id: string) => void;
    view: string;
    show: (view: string) => void;
    newAgent: (folder: string) => Promise<void>;
  } = $props();

  let menu = $state(false);
  let adding = $state(false);
  let folder = $state("");
  let folded = $state(new Set<string>());

  const open = $derived(agents.filter((a) => a.status !== "suspended"));
  const waiting = $derived(agents.filter((a) => a.questions > 0));
  const grouped = $derived(projects.map((p) => ({ ...p, agents: agents.filter((a) => a.project_id === p.id) })).filter((p) => p.agents.length > 0));

  function nextWaiting() {
    if (!waiting.length) return;
    const at = waiting.findIndex((a) => a.id === selected);
    pick(waiting[(at + 1) % waiting.length].id);
  }

  function keys(e: KeyboardEvent) {
    if (!e.ctrlKey || e.altKey || e.metaKey) return;
    if (e.key === "i") { e.preventDefault(); nextWaiting(); }
    const n = Number(e.key);
    if (n >= 1 && n <= 9 && open[n - 1]) { e.preventDefault(); pick(open[n - 1].id); }
  }

  function toggle(id: string) {
    const next = new Set(folded);
    if (!next.delete(id)) next.add(id);
    folded = next;
  }

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!folder.trim()) return;
    await newAgent(folder.trim());
    folder = "";
    adding = false;
  }
</script>

<svelte:window onkeydown={keys} onclick={() => (menu = false)} />

<nav class="sidebar" aria-label="Agents">
  <div class="top">
    <span class="lock">
      <img class="wm on-dark" src={wordmarkDark} alt="loom" />
      <img class="wm on-light" src={wordmarkLight} alt="loom" />
      <span class="by">by shirvani.dev</span>
    </span>
  </div>

  {#if waiting.length}
    <div class="inbox-wrap">
      <button class="inbox" aria-haspopup="menu" aria-expanded={menu} onclick={(e) => { e.stopPropagation(); menu = !menu; }}>
        <i class="st wait"></i>{waiting.length === 1 ? "1 waits for you" : `${waiting.length} wait for you`} · Ctrl+I
        <Icon name="chev-d" size={13} />
      </button>
      {#if menu}
        <div class="menu" role="menu">
          {#each waiting as a (a.id)}
            <button class="it" role="menuitem" onclick={() => pick(a.id)}><i class="st wait"></i><span>{a.title || "New agent"}</span><small>{a.project}</small></button>
          {/each}
          <p class="foot">Ctrl+I goes to the next</p>
        </div>
      {/if}
    </div>
  {/if}

  <div class="list">
    {#if open.length}
      <h2 class="heading">OPEN · {open.length}</h2>
      {#each open as a, i (a.id)}
        {@const s = stateOf(a)}
        <button class="running" class:sel={a.id === selected} aria-current={a.id === selected ? "page" : undefined} onclick={() => pick(a.id)}>
          <span class="num">{i < 9 ? i + 1 : ""}</span><i class="st {s.shape}" role="img" aria-label={s.word}></i>
          <span class="t">{a.title || "New agent"}</span><span class="p">{a.project}</span>
        </button>
      {/each}
    {/if}

    <h2 class="heading">PROJECTS</h2>
    {#each grouped as p (p.id)}
      {@const waits = p.agents.filter((a) => a.questions > 0).length}
      <section class="project">
        <button class="prow" aria-expanded={!folded.has(p.id)} onclick={() => toggle(p.id)}>
          <Icon name={folded.has(p.id) ? "chev-r" : "chev-d"} size={13} /><span class="pname">{p.name}</span>
          {#if waits}<span class="badge-wait"><i class="st wait"></i>{waits}</span>{/if}
        </button>
        {#if !folded.has(p.id)}
          {#each p.agents as a (a.id)}
            {@const s = stateOf(a)}
            <button class="arow" class:sel={a.id === selected} aria-current={a.id === selected ? "page" : undefined} onclick={() => pick(a.id)}>
              <i class="st {s.shape}" role="img" aria-label={s.word}></i>
              <span class="t">{a.title || "New agent"}</span>
              <span class="s"><span class="w-{s.shape}">{s.word}</span>{#if a.place} · {a.place}{/if}{#if a.edited} · <Icon name="diff" size={11} />{a.edited}{/if}</span>
            </button>
          {/each}
        {/if}
      </section>
    {/each}
    {#if !grouped.length}<p class="empty">No agents yet. Start one with the + below.</p>{/if}
  </div>

  {#if adding}
    <form class="adding" onsubmit={add}>
      <!-- svelte-ignore a11y_autofocus -->
      <input aria-label="Project folder" placeholder="/path/to/project" bind:value={folder} autofocus />
      <button class="go" disabled={!folder.trim()}>Start</button>
    </form>
  {/if}
  <div class="bottom">
    <button class="icon" aria-label="New agent" title="New agent" onclick={() => (adding = !adding)}><Icon name="plus" /></button>
    <button class="icon" class:on={view === "jobs"} aria-label="Jobs" title="Jobs" onclick={() => show(view === "jobs" ? "chat" : "jobs")}><Icon name="terminal" /></button>
    <span class="grow"></span>
    <button class="icon" class:on={view === "asks"} aria-label="Asks" title="Asks" onclick={() => show(view === "asks" ? "chat" : "asks")}><Icon name="inbox" /></button>
  </div>
</nav>
