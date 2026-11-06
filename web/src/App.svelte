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
  import Palette, { type Action } from "./Palette.svelte";
  import Scratch from "./Scratch.svelte";
  import { command } from "./lib/api";

  let conn = $state<{ state: State; hello: any }>({ state: "connecting", hello: null });
  let agents = $state.raw<AgentsItem[]>([]);
  let projects = $state.raw<ProjectsItem[]>([]);
  let asks = $state.raw<AsksItem[]>([]);
  let jobs = $state.raw<JobsItem[]>([]);
  let job = $state<string | null>(null);
  let chat = $state<{ agent: string; title: string } | null>(null);
  let filesProject = $state("");
  let findAsked = $state(0);
  let palette = $state(false);
  let starting = $state(false);
  let noSidebar = $state(false);
  let scratch = $state(false);
  let beside = $state<string | null>(null);
  let previous: string | null = null;
  let runAsked = $state(0);

  const pick = (id: string) => { if (id !== selected) previous = selected; selected = id; view = "chat"; };
  const files = (find = false) => { if (current) filesProject = current.project_id; view = "files"; if (find) findAsked++; };
  const newestJob = () => { const j = jobs.at(-1); if (j) { job = j.id; view = "jobs"; } };
  const nextMode = () => {
    const modes = (current?.modes ?? []) as string[];
    if (current && modes.length) command("set_mode", { agent: current.id, mode: modes[(modes.indexOf(current.mode ?? "") + 1) % modes.length] }).catch((e) => (problem = e.message));
  };
  const has = () => !!current;
  // Every action, its keys (the browser keeps Ctrl+N, Ctrl+W and Ctrl+T: those get Alt).
  const actions: Action[] = [
    { name: "New agent… (or reopen, or resume one)", keys: "Alt+N", run: () => (starting = true) },
    { name: "Reopen the last closed agent", keys: "Alt+Shift+T", run: () => reopenLast() },
    { name: "Close this agent", keys: "Alt+W", when: has, run: () => current && confirm(`Close ${current.title || "this agent"}? You can reopen it.`) && act(command("close_agent", { agent: current.id })) },
    { name: "Rename this agent", keys: "F2", when: has, run: () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "F2" })) },
    { name: "Stop the turn", keys: "Esc (in the box)", when: () => !!current?.busy, run: () => current && command("stop", { agent: current.id }) },
    { name: "Next permission mode", keys: "Shift+Tab (in the box)", when: has, run: nextMode },
    { name: "Move this agent up", keys: "Alt+Shift+↑", when: has, run: () => current && command("move_agent", { agent: current.id, by: -1 }) },
    { name: "Move this agent down", keys: "Alt+Shift+↓", when: has, run: () => current && command("move_agent", { agent: current.id, by: 1 }) },
    { name: "Show the previous agent beside", keys: "Ctrl+\\", when: has, run: () => toggleBeside() },
    { name: "Swap the two agents", keys: "Alt+←", when: () => !!beside, run: () => swap() },
    { name: "Toggle the sidebar", keys: "Ctrl+B", run: () => (noSidebar = !noSidebar) },
    { name: "Files and git", keys: "Ctrl+E", run: () => files() },
    { name: "Find a file in the project", keys: "Ctrl+P", run: () => files(true) },
    { name: "Scratch terminal", keys: "Ctrl+`", run: () => (scratch = !scratch) },
    { name: "Run a command in the tray…", keys: "Ctrl+J", run: () => { view = "jobs"; runAsked++; } },
    { name: "Peek the newest job", keys: "Ctrl+Shift+J", run: newestJob },
    { name: "Questions waiting: the next", keys: "Ctrl+I", run: () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "i", ctrlKey: true })) },
    { name: "Overview of all agents", keys: "Alt+O", run: () => (view = "overview") },
    { name: "Notes: chat with your notes", keys: "Alt+M", run: () => (view = "notes") },
    { name: "Asks", run: () => (view = "asks") },
    { name: "Settings", keys: "Ctrl+,", run: () => (view = "settings") },
    { name: "Actions and keys", keys: "Ctrl+K, Ctrl+/", run: () => (palette = true) },
  ];
  // the conversation closed last, reopened (`closed`, newest first)
  function reopenLast() {
    let stop = () => {};
    stop = follow("closed", (f) => {
      if (!f.synced && !f.error) return;
      queueMicrotask(() => stop());
      const last = f.items[0];
      if (last) act(command("reopen", { session: last.id }));
      else problem = "Nothing closed lately.";
    });
  }
  function toggleBeside() {
    if (beside) return void (beside = null);
    const other = previous && agents.some((a) => a.id === previous) ? previous : agents.find((a) => a.id !== selected)?.id;
    if (other && other !== selected) { beside = other; view = "chat"; }
  }
  function swap() { if (beside && selected) [selected, beside] = [beside, selected]; }

  function keys(e: KeyboardEvent) {
    const ctrl = e.ctrlKey || e.metaKey, k = e.key.toLowerCase();
    const take = (f: () => void) => { e.preventDefault(); f(); };
    if (ctrl && !e.altKey && !e.shiftKey) {
      if (k === "k" || k === "/") return take(() => (palette = !palette));
      if (k === "e") return take(() => (view === "files" ? (view = "chat") : files()));
      if (k === "p") return take(() => files(true));
      if (k === "`") return take(() => (scratch = !scratch));
      if (k === "j") return take(() => { view = "jobs"; runAsked++; });
      if (k === "b") return take(() => (noSidebar = !noSidebar));
      if (k === "\\") return take(toggleBeside);
      if (k === ",") return take(() => (view = "settings"));
    }
    if (ctrl && e.shiftKey && k === "j") return take(newestJob);
    if (e.altKey && !ctrl && !e.shiftKey) {
      if (k === "n") return take(() => (starting = true));
      if (k === "w") return take(() => actions[2].run());
      if (k === "o") return take(() => (view = view === "overview" ? "chat" : "overview"));
      if (k === "m") return take(() => (view = view === "notes" ? "chat" : "notes"));
      if (e.key === "ArrowLeft" && beside) return take(swap);
    }
    if (e.altKey && e.shiftKey && !ctrl) {
      if (k === "t") return take(reopenLast);
      if ((e.key === "ArrowUp" || e.key === "ArrowDown") && current) return take(() => command("move_agent", { agent: current!.id, by: e.key === "ArrowUp" ? -1 : 1 }));
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
  const besideAgent = $derived(beside && beside !== selected ? (agents.find((a) => a.id === beside) ?? null) : null);

  async function act(p: Promise<{ agent?: string }>) {
    problem = "";
    try {
      const answer = await p;
      if (answer?.agent) pick(answer.agent);
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
  <div class="app" class:no-sidebar={noSidebar}>
    {#if !noSidebar}<Sidebar {agents} {projects} {selected} {view} {pick} bind:starting show={(v) => { if (v === "files" && current) filesProject = current.project_id; view = v; }} {act} />{/if}
    <main class="main" class:with-scratch={scratch && !!current}>
      {#if conn.state !== "open"}<p class="banner">{conn.state === "closed" ? "loom isn't answering: trying again…" : "Connecting to loom…"}</p>{/if}
      {#if problem}<p class="banner error">{problem}</p>{/if}
      {#if view === "files"}
        <Files {projects} bind:project={filesProject} {findAsked} />
      {:else if view === "overview"}
        <Overview {agents} {pick} />
      {:else if view === "settings"}
        <Settings />
      {:else if view === "notes"}
        <Notes bind:chat />
      {:else if view === "asks"}
        <Asks {asks} {agents} {pick} showJob={(id) => { job = id; view = "jobs"; }} />
      {:else if view === "jobs"}
        <Jobs {jobs} {projects} bind:open={job} {runAsked} />
      {:else if current}
        <div class="pair" class:two={!!besideAgent}>
          {#key current.id}<Conversation agent={current} active jobs={jobs.filter((j) => j.project_id === current.project_id && j.name !== "scratch")} showJob={(id) => { job = id; view = "jobs"; }} expectCopy={(on) => (copyAfter = on ? Date.now() : 0)} />{/key}
          {#if besideAgent}{#key besideAgent.id}<Conversation agent={besideAgent} active={false} expectCopy={(on) => (copyAfter = on ? Date.now() : 0)} />{/key}{/if}
        </div>
      {:else}
        <p class="empty centre-note">No agents yet. Start one with the + at the bottom left.</p>
      {/if}
      {#if scratch && current}<Scratch project={current.project_id} {jobs} close={() => (scratch = false)} />{/if}
    </main>
  </div>
{/if}
{#if palette}<Palette {actions} close={() => (palette = false)} />{/if}
