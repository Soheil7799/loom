<script lang="ts">
  // An agent's conversation as Turns: your message opens each turn, pinned while you read it;
  // finished turns fold to your message and how they ended; the tube down the left marks turn
  // starts, questions and failures, and moves while the agent works. Alt+Up/Down: the previous
  // or next question or failure.
  import { follow, command, type Item } from "./lib/api";
  import type { AgentsItem, UsageItem } from "./lib/api-types";
  import { turns, type Entry } from "./lib/turns";
  import { stateOf } from "./lib/state";
  import Entry_ from "./Entry.svelte";
  import Tube from "./Tube.svelte";
  import Icon from "./Icon.svelte";

  let { agent }: { agent: AgentsItem } = $props();
  let entries = $state.raw<Entry[]>([]);
  let usage = $state.raw<UsageItem | null>(null);
  let gone = $state(false);
  let text = $state("");
  let problem = $state("");
  let expanded = $state(new Set<string>());
  let box: HTMLElement | undefined = $state();
  let column: HTMLElement | undefined = $state();
  let naming = $state<string | null>(null);

  $effect(() => follow(`conversation/${agent.id}`, (f) => {
    entries = [...f.items] as Entry[];
    gone = f.gone;
    if (f.error) problem = f.error;
  }));
  $effect(() => follow("usage", (f) => (usage = (f.items.find((u: Item) => u.id === agent.id) as UsageItem) ?? null)));

  const all = $derived(turns(entries));
  const st = $derived(stateOf(agent));
  const context = $derived(usage?.used && usage?.size ? Math.round((usage.used / usage.size) * 100) : null);
  const cost = $derived(usage?.cost as { amount?: number; currency?: string } | undefined);

  // "Since you last looked": the newest entry seen here before, kept per agent in this browser.
  const seenKey = $derived(`loom-seen:${agent.id}`);
  const seen = $derived(Number(localStorage.getItem(seenKey)?.slice(1) ?? "0"));
  const num = (id: string) => Number(id.slice(1)) || 0;
  const sinceAt = $derived(seen ? all.findIndex((t) => [t.user, ...t.rows.flatMap((r) => (r.kind === "run" ? r.tools : [r.entry]))].some((e) => e && num(e.id) > seen)) : -1);
  $effect(() => {
    const last = entries.at(-1);
    if (last && document.visibilityState === "visible") localStorage.setItem(seenKey, last.id);
  });

  // Kept at the end as it grows, unless you scrolled up.
  $effect(() => {
    entries;
    if (box && box.scrollHeight - box.scrollTop - box.clientHeight < 160) queueMicrotask(() => box && (box.scrollTop = box.scrollHeight));
  });

  // the result line is one line of plain words: markdown's marks taken out
  const plain = (md: string) => md.replace(/[`*_#>]+/g, "").replace(/\s+/g, " ").trim();

  const folded = (i: number) => {
    const t = all[i];
    return i < all.length - 1 && !t.live && !t.waits && !t.failed && !expanded.has(t.id);
  };
  function toggle(id: string) {
    const next = new Set(expanded);
    if (!next.delete(id)) next.add(id);
    expanded = next;
  }

  async function run(p: Promise<unknown>) {
    problem = "";
    try { await p; return true; } catch (e) { problem = (e as Error).message; return false; }
  }
  async function send() {
    if (!text.trim()) return;
    if (await run(command("send", { agent: agent.id, text }))) text = "";
  }
  function keys(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
  }
  async function rename(e?: SubmitEvent) {
    e?.preventDefault();
    const title = naming?.trim();
    naming = null;
    if (title && title !== agent.title) await run(command("rename", { agent: agent.id, title }));
  }
  function jump(e: KeyboardEvent) {
    if (e.key === "F2" && naming === null) { e.preventDefault(); naming = agent.title; return; }
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown") || !box) return;
    const top = box.getBoundingClientRect().top, at = box.scrollTop + 120;
    const marks = [...box.querySelectorAll<HTMLElement>('[data-bead="wait"], [data-bead="fail"]')].map((m) => m.getBoundingClientRect().top - top + box!.scrollTop);
    const to = e.key === "ArrowDown" ? marks.find((y) => y > at + 4) : marks.reverse().find((y) => y < at - 4);
    if (to !== undefined) { box.scrollTo({ top: to - 120, behavior: "smooth" }); e.preventDefault(); }
  }
</script>

<svelte:window onkeydown={jump} />

<section class="centre" aria-label={agent.title || "Conversation"}>
  <header class="header">
    <i class="st {st.shape}" role="img" aria-label={st.word}></i>
    <span class="headline">
      {#if naming !== null}
        <form class="rename" onsubmit={rename}>
          <!-- svelte-ignore a11y_autofocus -->
          <input aria-label="The agent's name" bind:value={naming} autofocus onblur={() => rename()} onkeydown={(e) => e.key === "Escape" && (naming = null)} />
        </form>
      {:else}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="title" ondblclick={() => (naming = agent.title)}>{agent.title || "New agent"}</span>
        <button class="tiny" aria-label="Rename (F2)" title="Rename (F2)" onclick={() => (naming = agent.title)}><Icon name="pencil" size={13} /></button>
      {/if}
      <span class="place">· {agent.project}</span>
      {#if st.shape === "wait"}<span class="needs">waits for you</span>{/if}
    </span>
    <span class="grow"></span>
    {#if context !== null}<span class="ctx"><span class="bar"><i style:width="{Math.min(context, 100)}%"></i></span>{context}%</span>{/if}
    {#if cost?.amount !== undefined}<span class="ctx">{cost.currency === "USD" || !cost.currency ? "$" : cost.currency + " "}{cost.amount.toFixed(2)}</span>{/if}
  </header>

  <div class="messages" bind:this={box} role="log" aria-label="Conversation">
    <div class="column" bind:this={column}>
      <Tube {column} live={agent.busy && st.shape !== "wait"} />
      {#if gone}<p class="notice">This agent was closed.</p>{/if}
      {#each all as t, i (t.id)}
        {#if i === sinceAt}<div class="since">Since you last looked</div>{/if}
        <section class="turn" class:current={i === all.length - 1}>
          {#if t.user}
            <div class="msg-user" data-bead="turn" data-bead-y="26">
              <span class="bub"><span class="text">{t.user.text}</span><span class="when"><b>You</b>{#if t.seconds !== undefined} · {t.seconds}s{/if}</span></span>
            </div>
          {/if}
          {#if folded(i)}
            <button class="result" aria-expanded="false" onclick={() => toggle(t.id)}>
              <span class="ok"><Icon name="check" size={13} /></span><span class="what">{plain(t.result ?? "Done")}</span>
              <span class="n">{t.steps} steps</span><Icon name="chev-r" size={13} />
            </button>
          {:else}
            {#if expanded.has(t.id)}<button class="result" aria-expanded="true" onclick={() => toggle(t.id)}><span class="what quiet">Fold this turn</span><Icon name="chev-d" size={13} /></button>{/if}
            {#each t.rows as row, k (row.kind === "run" ? row.id : row.entry.id)}
              <div class="row" data-latest={i === all.length - 1 && k === t.rows.length - 1 ? "" : undefined}><Entry_ {row} agent={agent.id} /></div>
            {/each}
          {/if}
        </section>
      {/each}
      {#if !all.length && !gone}<p class="empty">Nothing said yet. Tell this agent what to do.</p>{/if}
    </div>
  </div>

  <div class="input-box">
    {#if problem}<p class="error">{problem}</p>{/if}
    <div class="composer">
      <textarea aria-label="Message to {agent.title || 'the agent'}" rows="2" placeholder="Ask Claude…  Enter sends, Shift+Enter: a new line" bind:value={text} onkeydown={keys} disabled={gone}></textarea>
      <div class="cbar">
        {#if agent.modes?.length}
          <select class="chip" aria-label="Permission mode" value={agent.mode} onchange={(e) => run(command("set_mode", { agent: agent.id, mode: e.currentTarget.value }))}>
            {#each agent.modes as mode}<option value={mode}>Mode · {mode}</option>{/each}
          </select>
        {/if}
        <span class="grow"></span>
        {#if st.shape === "wait"}<span class="status waiting">Waits for you</span>{:else if agent.busy}<span class="status">Working</span>{/if}
        {#if agent.busy && !text.trim()}
          <button class="send stop" aria-label="Stop" title="Stop (Esc)" onclick={() => run(command("stop", { agent: agent.id }))}><Icon name="stop" /></button>
        {:else}
          <button class="send" aria-label="Send" disabled={!text.trim() || gone} onclick={send}><Icon name="send" /></button>
        {/if}
      </div>
    </div>
  </div>
</section>
