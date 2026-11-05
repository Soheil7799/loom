<script>
  import { follow, command } from "./lib/api";
  import Question from "./Question.svelte";

  let { agent } = $props();
  let entries = $state.raw([]);
  let gone = $state(false);
  let text = $state("");
  let problem = $state("");
  let list;

  $effect(() => {
    return follow(`conversation/${agent.id}`, (f) => {
      entries = [...f.items];
      gone = f.gone;
      if (f.error) problem = f.error;
    });
  });

  // Kept at the end as it grows, unless the user scrolled up.
  $effect(() => {
    entries;
    if (list && list.scrollHeight - list.scrollTop - list.clientHeight < 120) queueMicrotask(() => (list.scrollTop = list.scrollHeight));
  });

  async function run(promise) {
    problem = "";
    try {
      await promise;
      return true;
    } catch (e) {
      problem = e.message;
      return false;
    }
  }

  async function send() {
    if (!text.trim()) return;
    if (await run(command("send", { agent: agent.id, text }))) text = "";
  }

  function keys(e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  const label = (e) => ({ user: "You", agent: "Agent", thought: "Thinking", notice: "" })[e.kind] ?? e.kind;
</script>

<div class="conversation">
  <header>
    <h1>{agent.title || "New agent"}</h1>
    <span class="dim">{agent.status}{agent.place ? ` · ${agent.place}` : ""}</span>
    <span class="grow"></span>
    {#if agent.modes?.length}
      <select value={agent.mode} onchange={(e) => run(command("set_mode", { agent: agent.id, mode: e.currentTarget.value }))}>
        {#each agent.modes as mode}<option value={mode}>{mode}</option>{/each}
      </select>
    {/if}
    {#if agent.busy}<button onclick={() => run(command("stop", { agent: agent.id }))}>Stop</button>{/if}
  </header>

  <div class="entries" bind:this={list}>
    {#if gone}<p class="dim">This agent was closed.</p>{/if}
    {#each entries as e (e.id)}
      {#if e.kind === "tool"}
        <details class="tool {e.status}">
          <summary><span class="dot {e.status}"></span>{e.title || e.tool || "Tool"}{#if e.exit !== undefined && e.exit !== null} <span class="dim">exit {e.exit}</span>{/if}</summary>
          {#if e.input}<pre>{JSON.stringify(e.input, null, 2)}</pre>{/if}
          {#if e.output}<pre>{e.output}</pre>{/if}
        </details>
      {:else if e.kind === "question"}
        <Question entry={e} agent={agent.id} />
      {:else if e.kind === "turn"}
        <p class="turn dim">— {e.seconds ?? 0}s —</p>
      {:else if e.text || e.images?.length}
        <div class="entry {e.kind}">
          {#if label(e)}<span class="who">{label(e)}</span>{/if}
          <div class="text">{e.text}</div>
          {#if e.images?.length}<span class="dim">({e.images.length} picture{e.images.length === 1 ? "" : "s"})</span>{/if}
        </div>
      {/if}
    {/each}
  </div>

  {#if problem}<p class="error">{problem}</p>{/if}
  <form class="input" onsubmit={(e) => { e.preventDefault(); send(); }}>
    <textarea rows="3" placeholder="Message (Enter sends, Shift+Enter: a new line)" bind:value={text} onkeydown={keys} disabled={gone}></textarea>
    <button disabled={!text.trim() || gone}>Send</button>
  </form>
</div>
