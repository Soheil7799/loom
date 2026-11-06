<script lang="ts">
  // loom's web page: the sidebar and the open agent's conversation, as the window has them.
  import { onState, follow, type State } from "./lib/api";
  import type { AgentsItem, ProjectsItem, AsksItem, JobsItem } from "./lib/api-types";
  import Sidebar from "./Sidebar.svelte";
  import Conversation from "./Conversation.svelte";
  import Jobs from "./Jobs.svelte";
  import Asks from "./Asks.svelte";
  import Notes from "./Notes.svelte";
  import Settings from "./Settings.svelte";
  import Overview from "./Overview.svelte";
  import Files from "./Files.svelte";

  let conn = $state<{ state: State; hello: any }>({ state: "connecting", hello: null });
  let agents = $state.raw<AgentsItem[]>([]);
  let projects = $state.raw<ProjectsItem[]>([]);
  let asks = $state.raw<AsksItem[]>([]);
  let jobs = $state.raw<JobsItem[]>([]);
  let job = $state<string | null>(null);
  let chat = $state<{ agent: string; title: string } | null>(null);
  let filesProject = $state("");
  let findAsked = $state(0);
  // Ctrl+E: the files (of the open agent's project); Ctrl+P: find a file there
  function keys(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
    if (e.key === "e" || e.key === "p") {
      e.preventDefault();
      if (view !== "files" && current) filesProject = current.project_id;
      if (e.key === "e" && view === "files") return void (view = "chat");
      view = "files";
      if (e.key === "p") findAsked++;
    }
  }
  let selected = $state<string | null>(null);
  let view = $state("chat");
  let problem = $state("");

  onState((state, hello) => (conn = { state, hello }));

  $effect(() => {
    if (conn.state !== "open") return;
    const stops = [
      follow("agents", (f) => (agents = [...f.items] as AgentsItem[])),
      follow("projects", (f) => (projects = [...f.items] as ProjectsItem[])),
      follow("asks", (f) => (asks = [...f.items] as AsksItem[])),
      follow("jobs", (f) => (jobs = [...f.items] as JobsItem[])),
    ];
    return () => stops.forEach((s) => s());
  });

  // the first agent waiting for you, else the first one: until you pick, and when yours closes
  // a rewind's copy opens once it shows in `agents` (the first agent new since asking)
  let had = new Set<string>();
  let copyAfter = 0;
  $effect(() => {
    const ids = new Set(agents.map((a) => a.id));
    if (!selected || (had.has(selected) && !ids.has(selected))) selected = (agents.find((a) => a.questions > 0) ?? agents[0])?.id ?? null;
    const copy = copyAfter && agents.find((a) => !had.has(a.id) && Date.now() - copyAfter < 180000);
    if (copy) { selected = copy.id; copyAfter = 0; }
    had = ids;
  });

  // A job waiting for you to type (an agent's `loom_run` with input): its terminal, at once.
  let seenAsks: Set<string> | null = null;
  $effect(() => {
    const fresh = seenAsks ? asks.filter((a) => a.kind === "input" && !seenAsks!.has(a.id)) : [];
    seenAsks = new Set(asks.map((a) => a.id));
    const latest = fresh.at(-1) as (typeof fresh)[number] & { job?: string } | undefined;
    if (latest?.job) { job = latest.job; view = "jobs"; }
  });

  const current = $derived(agents.find((a) => a.id === selected) ?? null);

  async function act(p: Promise<{ agent?: string }>) {
    problem = "";
    try {
      const answer = await p;
      if (answer?.agent) { selected = answer.agent; view = "chat"; }
    } catch (e) {
      problem = (e as Error).message;
    }
  }
</script>

<svelte:window onkeydown={keys} />

{#if conn.state === "refused"}
  <main class="refused">
    <h1>loom</h1>
    <p>This page isn't connected to loom. Open a new link from loom: <code>loom api web --open</code>, or <code>loomd --web</code>.</p>
  </main>
{:else}
  <div class="app">
    <Sidebar {agents} {projects} {selected} {view} pick={(id) => { selected = id; view = "chat"; }} show={(v) => { if (v === "files" && current) filesProject = current.project_id; view = v; }} {act} />
    <main class="main">
      {#if conn.state !== "open"}<p class="banner">{conn.state === "closed" ? "loom isn't answering: trying again…" : "Connecting to loom…"}</p>{/if}
      {#if problem}<p class="banner error">{problem}</p>{/if}
      {#if view === "files"}
        <Files {projects} bind:project={filesProject} {findAsked} />
      {:else if view === "overview"}
        <Overview {agents} pick={(id) => { selected = id; view = "chat"; }} />
      {:else if view === "settings"}
        <Settings />
      {:else if view === "notes"}
        <Notes bind:chat />
      {:else if view === "asks"}
        <Asks {asks} {agents} pick={(id) => { selected = id; view = "chat"; }} showJob={(id) => { job = id; view = "jobs"; }} />
      {:else if view === "jobs"}
        <Jobs {jobs} {projects} bind:open={job} />
      {:else if current}
        {#key current.id}<Conversation agent={current} jobs={jobs.filter((j) => j.project_id === current.project_id)} showJob={(id) => { job = id; view = "jobs"; }} expectCopy={(on) => (copyAfter = on ? Date.now() : 0)} />{/key}
      {:else}
        <p class="empty centre-note">No agents yet. Start one with the + at the bottom left.</p>
      {/if}
    </main>
  </div>
{/if}
