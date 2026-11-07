<script lang="ts">
  // loom's web page: the sidebar and the open agent's conversation, as the window has them.
  import { onState, follow, type State } from "./lib/api";
  import type { AgentsItem, ProjectsItem, AsksItem, JobsItem } from "./lib/api-types";
  import Sidebar from "./Sidebar.svelte";
  import Conversation from "./Conversation.svelte";
  import Tray from "./Tray.svelte";
  import Asks from "./Asks.svelte";
  import NotesChats from "./NotesChats.svelte";
  import NotesPanel from "./NotesPanel.svelte";
  import NoteView from "./NoteView.svelte";
  import NotesHistory from "./NotesHistory.svelte";
  import Settings from "./Settings.svelte";
  import Overview from "./Overview.svelte";
  import Panel from "./Panel.svelte";
  import Editor from "./Editor.svelte";
  import Finder from "./Finder.svelte";
  import Project from "./Project.svelte";
  import Palette, { type Action } from "./Palette.svelte";
  import Scratch from "./Scratch.svelte";
  import { command } from "./lib/api";
  import wordmarkDark from "../../data/logo/wordmark-dark.svg?url";
  import wordmarkLight from "../../data/logo/wordmark-light.svg?url";

  let conn = $state<{ state: State; hello: any }>({ state: "connecting", hello: null });
  let agents = $state.raw<AgentsItem[]>([]);
  let projects = $state.raw<ProjectsItem[]>([]);
  let asks = $state.raw<AsksItem[]>([]);
  let jobs = $state.raw<JobsItem[]>([]);
  // the job peeked in the tray, and the project whose tray shows (the open agent's, unless a
  // job of another project was brought forward)
  let peekJob = $state<string | null>(null);
  let trayOf = $state<string | null>(null);
  // The Notes view (Alt+M): its chat open, the note open beside it
  let chat = $state<{ agent: string; title: string } | null>(null);
  let notePath = $state<string | null>(null);
  let notesHistory = $state(false);
  async function openChat(session?: string, title = "Notes", topic?: string) {
    try {
      const answer = await command("notes_open", topic ? { topic } : session ? { session } : {});
      chat = { agent: answer.agent, title: topic ?? title };
      notesHistory = false;
    } catch (e) { problem = (e as Error).message; }
  }
  // the chat as Conversation draws an agent: a Notes chat isn't among the agents
  const chatAgent = $derived(chat && ({ id: chat.agent, title: chat.title, project: "your notes", project_id: "", status: "notes", busy: false, questions: 0, unseen: false, used: "", place: "", edited: 0, awake: true } as AgentsItem));
  // The right panel (Ctrl+E) and its tab; the file open beside the chat; Ctrl+P's finder
  let panel = $state(false);
  let panelTab = $state<"agent" | "project" | "changes" | "graph" | "notes">("agent");
  let openPath = $state<string | null>(null);
  let finding = $state(false);
  // text put into the open conversation's box from elsewhere (a file mentioned from Files)
  let insert = $state<string | null>(null);
  // a note on a line of the agent's changes, for its next message
  let note = $state<{ file: string; line: string; note: string } | null>(null);
  let projectPage = $state<string | null>(null);
  let palette = $state(false);
  let about = $state(false);

  // A word from this browser when an agent starts waiting for you or finishes a turn, while
  // the page is out of sight (Settings: the same switches as the window's; the browser asks
  // you once, from Settings → General).
  let notifyOn = { questions: true, done: true };
  $effect(() => follow("settings", (f) => (notifyOn = { questions: f.items[0]?.notify_questions !== false, done: f.items[0]?.notify_done !== false })));
  let before = new Map<string, { questions: number; busy: boolean }>();
  $effect(() => {
    const now = new Map(agents.map((a) => [a.id, { questions: a.questions, busy: a.busy }]));
    const away = document.visibilityState !== "visible" || !document.hasFocus();
    if (away && "Notification" in window && Notification.permission === "granted") {
      for (const a of agents) {
        const was = before.get(a.id);
        if (!was) continue;
        const word = notifyOn.questions && was.questions === 0 && a.questions > 0 ? "Waits for you" : notifyOn.done && was.busy && !a.busy && a.questions === 0 ? "Finished" : null;
        if (word) {
          const n = new Notification(a.title || "loom", { body: word, tag: `loom-${a.id}` });
          n.onclick = () => { window.focus(); pick(a.id); n.close(); };
        }
      }
    }
    before = now;
  });
  let starting = $state(false);
  let noSidebar = $state(false);
  let scratch = $state(false);
  let beside = $state<string | null>(null);
  let previous: string | null = null;
  let runAsked = $state(false);

  const pick = (id: string) => { if (id !== selected) previous = selected; selected = id; view = "chat"; };
  const togglePanel = () => (panel = !panel);
  const find = () => { if (current || projects.length) finding = true; };
  const mention = (rel: string) => { if (current) { insert = `@${rel} `; view = "chat"; } };
  const showJob = (id: string) => { const j = jobs.find((x) => x.id === id); trayOf = j?.project_id ?? null; peekJob = id; if (view !== "chat" && view !== "overview") view = "chat"; };
  const newestJob = () => { const j = trayJobs.at(-1); if (j) peekJob = j.id; };
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
    { name: "Files and git: the right panel", keys: "Ctrl+E", run: togglePanel },
    { name: "Find a file in the project", keys: "Ctrl+P", run: find },
    { name: "Scratch terminal", keys: "Ctrl+`", run: () => (scratch = !scratch) },
    { name: "Run a command in the tray…", keys: "Ctrl+J", run: () => (runAsked = true) },
    { name: "Peek the newest job", keys: "Ctrl+Shift+J", run: newestJob },
    { name: "Questions waiting: the next", keys: "Ctrl+I", run: () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "i", ctrlKey: true })) },
    { name: "Overview of all agents", keys: "Alt+O", run: () => (view = "overview") },
    { name: "Notes: chat with your notes", keys: "Alt+M", run: () => (view = "notes") },
    { name: "Asks", run: () => (view = "asks") },
    { name: "Settings", keys: "Ctrl+,", run: () => (view = "settings") },
    { name: "Actions and keys", keys: "Ctrl+K, Ctrl+/", run: () => (palette = true) },
    { name: "About loom", run: () => (about = true) },
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
      if (k === "e") return take(togglePanel);
      if (k === "p") return take(find);
      if (k === "`") return take(() => (scratch = !scratch));
      if (k === "j") return take(() => (runAsked = true));
      if (k === "b") return take(() => (noSidebar = !noSidebar));
      if (k === "\\") return take(toggleBeside);
      if (k === ",") return take(() => (view = "settings"));
    }
    if (ctrl && e.shiftKey && k === "j") return take(newestJob);
    if (e.altKey && !ctrl && !e.shiftKey) {
      if (k === "n") return take(() => (starting = true));
      if (k === "w") return take(() => actions.find((a) => a.name === "Close this agent")?.run());
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
  const notes = $derived(view === "notes");
  let problem = $state("");

  onState((state, hello) => (conn = { state, hello }));

  $effect(() => {
    if (conn.state !== "open") return;
    const stops = [
      follow("agents", (f) => (agents = [...f.items] as AgentsItem[])),
      follow("projects", (f) => (projects = [...f.items] as ProjectsItem[])),
      // the asks there when the page opens are not new
      follow("asks", (f) => { seenAsks ??= new Set(f.items.map((a) => a.id)); asks = [...f.items] as AsksItem[]; }),
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
    if (!seenAsks) return;
    const fresh = asks.filter((a) => a.kind === "input" && !seenAsks!.has(a.id));
    for (const a of asks) seenAsks.add(a.id);
    const latest = fresh.at(-1) as (typeof fresh)[number] & { job?: string } | undefined;
    if (latest?.job) showJob(latest.job);
  });
  // A job an agent opened for you (Claude's login, say): brought forward, as the window does.
  let shown: Map<string, number> | null = null;
  $effect(() => {
    if (conn.state !== "open") return;
    return follow("jobs_shown", (f) => {
      const was = shown;
      shown = new Map(f.items.map((i) => [i.id, i.n]));
      const fresh = was ? f.items.find((i) => was.get(i.id) !== i.n) : null;
      if (fresh?.job) showJob(fresh.job);
    });
  });

  // An input ask whose job has ended is answered: it goes.
  $effect(() => {
    for (const a of asks as (AsksItem & { job?: string })[]) {
      if (a.kind === "input" && a.job && jobs.find((j) => j.id === a.job)?.status === "exited") command("ask_done", { id: a.id }).catch(() => {});
    }
  });

  const current = $derived(agents.find((a) => a.id === selected) ?? null);
  // another agent opened: its project's tray
  $effect(() => { current?.id; trayOf = null; });
  const trayProject = $derived(trayOf ?? current?.project_id ?? "");
  const trayJobs = $derived(jobs.filter((j) => j.project_id === trayProject && j.name !== "scratch"));
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
  <div class="app" class:no-sidebar={noSidebar} class:with-panel={panel || notes}>
    {#if !noSidebar}<Sidebar {agents} {projects} {selected} {view} {pick} bind:starting openProject={(id) => { projectPage = id; view = "project"; }} show={(v) => { if (v === "about") about = true; else if (v === "scratch") scratch = !scratch; else if (v === "run") runAsked = true; else if (v === "palette") palette = true; else view = v; }} asks={asks.length} {act}>
      {#snippet notesPage()}<NotesChats {agents} {chat} open={(session, title) => openChat(session, title)} back={() => (view = "chat")} />{/snippet}
    </Sidebar>{/if}
    <div class="middle">
    <div class="reading">
    <main class="main" class:with-scratch={scratch && !!current}>
      {#if conn.state !== "open"}<p class="banner">{conn.state === "closed" ? "loom isn't answering: trying again…" : "Connecting to loom…"}</p>{/if}
      {#if problem}<p class="banner error">{problem}</p>{/if}
      {#if view === "project" && projects.find((p) => p.id === projectPage)}
        <Project project={projects.find((p) => p.id === projectPage)!} {act} />
      {:else if view === "overview"}
        <Overview {agents} {pick} />
      {:else if view === "settings"}
        <Settings />
      {:else if notes}
        {#if notesHistory}<NotesHistory close={() => (notesHistory = false)} />{:else if chatAgent}{#key chatAgent.id}<Conversation agent={chatAgent} />{/key}{:else}<p class="empty centre-note">Ask your notes anything, or tell them something worth keeping. Start with New chat.</p>{/if}
      {:else if view === "asks"}
        <Asks {asks} {agents} {pick} {showJob} />
      {:else if current}
        <div class="pair" class:two={!!besideAgent}>
          {#key current.id}<Conversation agent={current} active bind:insert bind:note showChanges={() => { panel = true; panelTab = "agent"; }} expectCopy={(on) => (copyAfter = on ? Date.now() : 0)} />{/key}
          {#if besideAgent}{#key besideAgent.id}<Conversation agent={besideAgent} active={false} expectCopy={(on) => (copyAfter = on ? Date.now() : 0)} />{/key}{/if}
        </div>
      {:else}
        <p class="empty centre-note">No agents yet. Start one with the + at the bottom left.</p>
      {/if}
      {#if scratch && current}<Scratch project={current.project_id} {jobs} close={() => (scratch = false)} />{/if}
    </main>
    {#if notes}<NoteView bind:path={notePath} />{:else}<Editor bind:path={openPath} project={current?.project_id ?? ""} {mention} />{/if}
    </div>
    {#if trayProject && !notes}<Tray project={trayProject} jobs={trayJobs} bind:peek={peekJob} bind:runAsked />{/if}
    </div>
    {#if notes}<NotesPanel openNote={(p) => (notePath = p)} chatAbout={(t) => openChat(undefined, t, t)} history={() => (notesHistory = true)} />{:else if panel}<Panel agent={current} {projects} bind:tab={panelTab} open={(p) => (openPath = p)} {mention} comment={(file, line, said) => (note = { file, line, note: said })} />{/if}
  </div>
{/if}
{#if finding}<Finder project={current?.project_id ?? projects[0]?.id ?? ""} pick={(p) => (openPath = p)} close={() => (finding = false)} />{/if}
{#if palette}<Palette {actions} close={() => (palette = false)} />{/if}
{#if about}
  <div class="scrim" role="presentation" onclick={() => (about = false)}></div>
  <div class="plan about" role="dialog" aria-modal="true" aria-label="About loom" tabindex="-1" onkeydown={(e) => e.key === "Escape" && (about = false)}>
    <img class="wm on-dark" src={wordmarkDark} alt="loom" /><img class="wm on-light" src={wordmarkLight} alt="loom" />
    <p>A workspace for Claude Code agents: many agents across projects, their jobs, your notes.</p>
    <p class="quiet small">loom {conn.hello?.server?.version ?? ""} · {conn.hello?.server?.host === "daemon" ? "loomd, with no window" : "loom's window"} · protocol {conn.hello?.protocol ?? ""}</p>
    <p class="small"><a href="https://github.com/Soheil7799/loom" target="_blank" rel="noopener noreferrer">github.com/Soheil7799/loom</a> · MIT licence · by shirvani.dev</p>
    <div class="controls"><span class="grow"></span><button class="btn" onclick={() => (about = false)}>Close</button></div>
  </div>
{/if}
