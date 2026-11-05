<script lang="ts">
  // Scribe, beside the Notes chat: what it does, its questions, the changes waiting for your
  // OK, what it wrote lately (undo), and the changes you turned down, each with a box to say
  // why (Scribe drafts a rule from it, kept with your OK).
  import { command, follow, type Item } from "./lib/api";
  import { ago } from "./lib/state";

  let items = $state.raw<Item[]>([]);
  let said = $state<Record<string, string>>({});
  let problem = $state("");

  $effect(() => follow("scribe", (f) => (items = f.items)));
  const status = $derived(items.find((i) => i.id === "status"));
  const list = (name: string) => items.filter((i) => i.list === name);
  const questions = $derived(list("question"));
  const proposals = $derived(list("proposal"));
  const changes = $derived(list("change"));
  const teach = $derived(list("teach"));

  async function run(p: Promise<unknown>) {
    problem = "";
    try { await p; return true; } catch (e) { problem = (e as Error).message; return false; }
  }
  async function answer(id: string, text: string) {
    if (text.trim() && (await run(command("scribe_answer", { id, answer: text.trim() })))) said = { ...said, [id]: "" };
  }
  // A change as one line: what it does to which note.
  function what(c: { op?: string; note?: string; key?: string; under?: string } | undefined) {
    if (!c) return "";
    const verb = { add: "Adds to", replace: "Changes", create: "Creates", property: `Sets ${c.key ?? "a property"} in` }[c.op ?? ""] ?? "Changes";
    return `${verb} ${c.note ?? "a note"}${c.op === "add" && c.under ? ` under ${c.under}` : ""}`;
  }
  const body = (c: Item["change"]) => (c?.op === "replace" ? null : (c?.text ?? c?.value ?? ""));
</script>

<aside class="scribe" aria-label="Scribe">
  <div class="scribe-head">
    <b>Scribe</b>
    <span class="quiet">{status?.running ? status.doing || "working…" : "idle"}{#if status?.calls_left !== undefined}{" · "}{status.calls_left} calls left today{/if}</span>
  </div>
  {#if problem}<p class="error">{problem}</p>{/if}

  {#if questions.length}
    <h3>Asks you</h3>
    {#each questions as q (q.id)}
      <div class="permission-card">
        <div class="qh"><span class="needs">{q.topic}</span><b>{q.question}</b></div>
        {#if q.excerpt}<blockquote>{q.excerpt}</blockquote>{/if}
        {#if q.options?.length}
          <div class="controls">{#each q.options as o (o)}<button class="btn" onclick={() => answer(q.id, o)}>{o}</button>{/each}</div>
        {/if}
        <form class="adding" onsubmit={(e) => { e.preventDefault(); answer(q.id, said[q.id] ?? ""); }}>
          <input aria-label="Your answer" placeholder="Or in your words" bind:value={said[q.id]} />
          <button class="go" disabled={!said[q.id]?.trim()}>Answer</button>
        </form>
        <div class="controls"><button class="btn small" onclick={() => run(command("scribe_drop", { id: q.id }))}>Leave it</button><span class="quiet small">from {q.title || "a chat"} · {ago(q.at)}</span></div>
      </div>
    {/each}
  {/if}

  {#if proposals.length}
    <h3>Waits for your OK</h3>
    {#each proposals as p (p.id)}
      <div class="permission-card">
        <div class="qh"><span class="needs">{p.topic}</span><b>{what(p.change)}</b></div>
        {#if p.reason || p.why}<p class="quiet">{p.reason}{#if p.why}{" · held: "}{p.why}{/if}</p>{/if}
        {#if p.change?.op === "replace"}
          <div class="diff"><pre><span class="del">-{p.change.old}
</span><span class="add">+{p.change.new}
</span></pre></div>
        {:else if body(p.change)}<pre class="code-block">{body(p.change)}</pre>{/if}
        <div class="controls">
          <button class="btn suggested" onclick={() => run(command("scribe_decide", { id: p.id, write: true }))}>Write it</button>
          <button class="btn deny" onclick={() => run(command("scribe_decide", { id: p.id, write: false }))}>No</button>
        </div>
      </div>
    {/each}
  {/if}

  {#if teach.length}
    <h3>Tell Scribe why not</h3>
    {#each teach as t (t.id)}
      <div class="permission-card answered">
        <div class="qh"><b>{t.kind === "answer" ? t.question : what(t.change)}</b></div>
        {#if t.stage === "draft" && t.draft}
          <p>Scribe would keep this rule:</p>
          <form class="adding" onsubmit={(e) => { e.preventDefault(); run(command("scribe_keep_rule", { id: t.id, rule: said[t.id] ?? t.draft })); }}>
            <input aria-label="The rule" value={said[t.id] ?? t.draft} oninput={(e) => (said = { ...said, [t.id]: e.currentTarget.value })} />
            <button class="go">Keep</button>
          </form>
        {:else if t.stage === "drafting"}
          <p class="quiet">Drafting a rule…</p>
        {:else}
          <form class="adding" onsubmit={(e) => { e.preventDefault(); if (said[t.id]?.trim()) run(command("scribe_teach", { id: t.id, said: said[t.id].trim() })); }}>
            <input aria-label="Why not" placeholder="Why not?" bind:value={said[t.id]} />
            <button class="go" disabled={!said[t.id]?.trim()}>Tell</button>
          </form>
        {/if}
        <div class="controls"><button class="btn small" onclick={() => run(command("scribe_forget", { id: t.id }))}>Forget it</button></div>
      </div>
    {/each}
  {/if}

  {#if changes.length}
    <h3>Wrote lately</h3>
    {#each changes as c (c.id)}
      <div class="change-row">
        <span><b>{what(c.change)}</b><small class="quiet">{c.topic} · {ago(c.at)}</small></span>
        {#if c.undoable}<button class="btn small" onclick={() => run(command("scribe_undo", { id: c.id }))}>Undo</button>{/if}
      </div>
    {/each}
  {/if}

  {#if !questions.length && !proposals.length && !teach.length && !changes.length}<p class="empty">Nothing from Scribe yet. It reads your chats and keeps your notes.</p>{/if}
</aside>
