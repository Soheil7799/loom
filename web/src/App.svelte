<script>
  import { onState, follow, command, has } from "./lib/api.js";
  import Conversation from "./Conversation.svelte";
  import Jobs from "./Jobs.svelte";
  import Asks from "./Asks.svelte";

  let conn = $state({ state: "connecting", hello: null });
  let agents = $state.raw([]);
  let projects = $state.raw([]);
  let chats = $state.raw([]);
  let asks = $state.raw([]);
  let selected = $state(null); // { id, title, notes }
  let view = $state("chat"); // chat, jobs, asks
  let folder = $state("");
  let problem = $state("");

  onState((state, hello) => (conn = { state, hello }));

  $effect(() => {
    if (conn.state !== "open") return;
    const stops = [
      follow("agents", (f) => (agents = [...f.items])),
      follow("projects", (f) => (projects = [...f.items])),
      follow("asks", (f) => (asks = [...f.items])),
    ];
    if (has("notes_open")) stops.push(follow("notes_chats", (f) => (chats = [...f.items])));
    return () => stops.forEach((s) => s());
  });

  // The agents under their project, in loom's order.
  let grouped = $derived(
    projects
      .map((p) => ({ ...p, agents: agents.filter((a) => a.project_id === p.id) }))
      .filter((p) => p.agents.length > 0),
  );

  async function run(promise) {
    problem = "";
    try {
      return await promise;
    } catch (e) {
      problem = e.message;
    }
  }

  async function newAgent() {
    const answer = await run(command("new_agent", { folder: folder.trim() }));
    if (answer) {
      folder = "";
      selected = { id: answer.agent, notes: false };
      view = "chat";
    }
  }

  async function openChat(session) {
    const answer = await run(command("notes_open", session ? { session } : {}));
    if (answer) {
      selected = { id: answer.agent, title: session ? chats.find((c) => c.id === session)?.title : "New chat", notes: true };
      view = "chat";
    }
  }

  let current = $derived(selected && (agents.find((a) => a.id === selected.id) ?? (selected.notes ? { id: selected.id, title: selected.title ?? "Notes chat", status: "", modes: [] } : null)));
</script>

{#if conn.state === "refused"}
  <main class="center">
    <h1>loom</h1>
    <p>This page isn't connected to loom. Open a new link from loom: <code>loom api web --open</code>, or <code>loomd --web</code>.</p>
  </main>
{:else}
  <div class="app">
    <aside>
      <header>
        <strong>loom</strong>
        <span class="dim">{conn.state === "open" ? `${conn.hello.server.host} · as ${conn.hello.principal.kind}` : conn.state}</span>
      </header>
      <nav>
        <button class:on={view === "asks"} onclick={() => (view = "asks")}>Asks {#if asks.length}<span class="badge">{asks.length}</span>{/if}</button>
        <button class:on={view === "jobs"} onclick={() => (view = "jobs")}>Jobs</button>
      </nav>
      {#each grouped as project (project.id)}
        <section>
          <h2 title={project.id}>{project.name}</h2>
          {#each project.agents as agent (agent.id)}
            <button class="agent" class:on={view === "chat" && selected?.id === agent.id} onclick={() => { selected = { id: agent.id, notes: false }; view = "chat"; }}>
              <span class="dot {agent.status}"></span>
              <span class="title">{agent.title || "New agent"}</span>
              {#if agent.questions}<span class="badge">{agent.questions}</span>{/if}
            </button>
          {/each}
        </section>
      {/each}
      {#if has("notes_open")}
        <section>
          <h2>Notes chat</h2>
          <button class="agent" onclick={() => openChat(null)}>+ New chat</button>
          {#each chats.slice(0, 12) as chat (chat.id)}
            <button class="agent" class:on={view === "chat" && selected?.notes && selected?.id === chat.agent} onclick={() => openChat(chat.id)}>
              <span class="title">{chat.title || "Untitled"}</span>
            </button>
          {/each}
        </section>
      {/if}
      <form class="new" onsubmit={(e) => { e.preventDefault(); newAgent(); }}>
        <input placeholder="/path/to/project" bind:value={folder} />
        <button disabled={!folder.trim()}>New agent</button>
      </form>
      {#if problem}<p class="error">{problem}</p>{/if}
    </aside>
    <main>
      {#if view === "asks"}
        <Asks {asks} />
      {:else if view === "jobs"}
        <Jobs {projects} />
      {:else if current}
        {#key current.id}
          <Conversation agent={current} />
        {/key}
      {:else}
        <p class="center dim">Pick an agent, or start one.</p>
      {/if}
    </main>
  </div>
{/if}
