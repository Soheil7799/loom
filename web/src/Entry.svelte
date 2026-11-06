<script lang="ts">
  // One row of a turn: the agent's words, a thought, a tool call (its detail on a click, a
  // diff for edits), a run of finished tool calls as one line, a question, a notice.
  import type { Row } from "./lib/turns";
  import { summary } from "./lib/turns";
  import { hunks, lines } from "./lib/diff";
  import Icon from "./Icon.svelte";
  import Question from "./Question.svelte";
  import { markdown } from "./lib/markdown";
  import { command } from "./lib/api";

  let { row, agent }: { row: Row; agent: string } = $props();
  let open = $state(false);
  let pressProblem = $state("");

  type Diff = { path?: string; oldText?: string; newText?: string };
  const diffs = (content: unknown[] | undefined): Diff[] =>
    (content ?? []).filter((c): c is Diff & { type: string } => !!c && typeof c === "object" && (c as { type?: string }).type === "diff");
  const name = (path?: string) => path?.split("/").pop() ?? "";
  // ACP's kind of tool, as a label; "other" (an MCP tool, say) says nothing: TOOL
  const kindOf = (kind?: string) => (!kind || kind === "other" ? "tool" : kind).toUpperCase();
</script>

{#if row.kind === "run"}
  <div class="run" class:open>
    <button class="summary" aria-expanded={open} onclick={() => (open = !open)}>
      <span class="g"><Icon name="check" size={13} /></span>{summary(row.tools)} <Icon name={open ? "chev-d" : "chev-r"} size={13} />
    </button>
    {#if open}
      <ul class="tools">
        {#each row.tools as t (t.id)}<li><span class="kind">{kindOf(t.tool)}</span><span class="tool-title">{t.title ?? ""}</span></li>{/each}
      </ul>
    {/if}
  </div>
{:else}
  {@const e = row.entry}
  {#if e.kind === "agent"}
    {#if e.text}<div class="msg-agent md">{@html markdown(e.text)}</div>{/if}
  {:else if e.kind === "thought"}
    {#if e.text}<details class="thought"><summary>Thinking</summary><div>{e.text}</div></details>{/if}
  {:else if e.kind === "tool"}
    {@const ds = diffs(e.content)}
    <div class="tool {e.status}" data-bead={e.status === "failed" ? "fail" : undefined} data-bead-y="9">
      <button class="tool-row" aria-expanded={open} onclick={() => (open = !open)}>
        <span class="g">
          {#if e.status === "failed"}<Icon name="x" size={13} label="Failed" />{:else if e.status === "done"}<Icon name="check" size={13} />{:else}<i class="spin" aria-label="Running"></i>{/if}
        </span>
        <span class="kind">{kindOf(e.tool)}</span>
        <span class="tool-title">{e.title ?? ""}</span>
        {#if e.exit !== undefined && e.exit !== null && e.exit !== 0}<span class="exit">exit {e.exit}</span>{/if}
        <Icon name={open || ds.length ? "chev-d" : "chev-r"} size={13} />
      </button>
      {#if e.status === "failed" && e.output}<div class="err-line">{e.output.trim().split("\n").at(-1)}</div>{/if}
      {#each ds as d, i (i)}
        <div class="diff">
          <div class="f"><span>{name(d.path)}</span></div>
          <pre>{#each hunks(lines(d.oldText ?? "", d.newText ?? "")) as l, k (k)}<span class={l.t === "+" ? "add" : l.t === "-" ? "del" : ""}>{l.t}{l.text}
</span>{/each}</pre>
        </div>
      {/each}
      {#if open}
        {#if e.input}<pre class="detail">{JSON.stringify(e.input, null, 2)}</pre>{/if}
        {#if e.output}<pre class="detail">{e.output}</pre>{/if}
      {/if}
    </div>
  {:else if e.kind === "question"}
    <div class="q-wrap" data-bead={e.status === "awaiting_user" ? "wait" : undefined} data-bead-y="24" aria-live="polite">
      <Question entry={e} {agent} />
    </div>
  {:else if e.kind === "notice"}
    {#if e.text}
      <p class="notice" class:error={e.status === "failed"}>{e.text}
        {#if e.button}<button class="btn small" onclick={() => command("press", { agent, entry: e.id }).catch((err) => (pressProblem = err.message))}>{e.button}</button>{/if}
        {#if pressProblem}<span class="error">{pressProblem}</span>{/if}
      </p>
    {/if}
  {:else if e.text}
    <div class="msg-agent">{e.text}</div>
  {/if}
{/if}
