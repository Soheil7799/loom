<script lang="ts">
  // A project's job tray, along the bottom as the window has it: a chip per command (a click
  // peeks its terminal over the chip; a right-click: Run again, Stop, Pin, Copy, Remove),
  // the pinned ones docked above the tray side by side, and Ctrl+J's box for a command.
  import { command } from "./lib/api";
  import type { JobsItem } from "./lib/api-types";
  import Terminal from "./Terminal.svelte";

  let { project, jobs, peek = $bindable(null), runAsked = $bindable(false) }: { project: string; jobs: JobsItem[]; peek?: string | null; runAsked?: boolean } = $props();

  let pinned = $state<string[]>([]);
  let menu = $state<{ x: number; y: number; job: JobsItem } | null>(null);
  let running = $state(false);
  let line = $state("");
  let problem = $state("");
  let chipsEl: HTMLElement | undefined = $state();

  const docked = $derived(pinned.map((id) => jobs.find((j) => j.id === id)).filter((j): j is JobsItem => !!j));
  const peeked = $derived(jobs.find((j) => j.id === peek && !pinned.includes(j.id)) ?? null);
  const shape = (j: JobsItem) => (j.status !== "exited" ? "work" : j.exit_code === 0 ? "done" : "fail");
  const short = (s: string) => (s.length > 28 ? `${s.slice(0, 27)}…` : s);
  const status = (j: JobsItem) => (j.status === "exited" ? `exited${j.exit_code != null ? ` with ${j.exit_code}` : ""}` : j.status);
  // Ctrl+J: the box for a command, once per press
  $effect(() => { if (runAsked) { runAsked = false; running = true; } });
  // the peek, over its chip: where the chip is
  let peekLeft = $state(0);
  $effect(() => {
    if (!peeked || !chipsEl) return;
    const chip = chipsEl.querySelector<HTMLElement>(`[data-job="${peeked.id}"]`);
    const bar = chipsEl.closest(".tray")?.getBoundingClientRect();
    if (chip && bar) peekLeft = Math.max(0, Math.min(chip.getBoundingClientRect().left - bar.left, bar.width - 780));
  });

  async function run<T>(p: Promise<T>) {
    problem = "";
    try { return await p; } catch (e) { problem = (e as Error).message; }
  }
  async function start(e: SubmitEvent) {
    e.preventDefault();
    const answer = await run(command("run_job", { project, command: line.trim() }));
    if (answer) { line = ""; running = false; peek = answer.job; }
  }
  const pin = (j: JobsItem) => { pinned = [...pinned.filter((p) => p !== j.id), j.id]; if (peek === j.id) peek = null; };
  const unpin = (j: JobsItem) => (pinned = pinned.filter((p) => p !== j.id));
  async function remove(j: JobsItem) {
    unpin(j);
    if (peek === j.id) peek = null;
    await run(command("remove_job", { job: j.id }));
  }
  async function copyOutput(j: JobsItem) {
    const got = await run(command("job_text", { job: j.id }));
    if (got) navigator.clipboard?.writeText(got.text);
  }
</script>

{#snippet frame(j: JobsItem, isDocked: boolean)}
  <div class={isDocked ? "docked" : "peek-frame"}>
    <div class="frame-head">
      <code title={j.command}>{j.name}</code><span class="quiet small">{status(j)}</span>
      <button class="btn small" title="Run the command again" onclick={() => run(command("restart_job", { job: j.id }))}>Rerun</button>
      <button class="btn small" title="Stop the command" disabled={j.status === "exited"} onclick={() => run(command("stop_job", { job: j.id }))}>Stop</button>
      {#if isDocked}<button class="btn small" title="Back to the tray" onclick={() => unpin(j)}>Unpin</button>{:else}<button class="btn small" title="Dock above the tray" onclick={() => pin(j)}>Pin</button>{/if}
      <button class="btn small" title="Remove from the tray" onclick={() => remove(j)}>Remove</button>
    </div>
    {#key j.id}<Terminal job={j.id} />{/key}
  </div>
{/snippet}

{#if docked.length}
  <div class="dock">{#each docked as j (j.id)}{@render frame(j, true)}{/each}</div>
{/if}
<div class="tray">
  {#if peeked}
    <div class="scrim clear" role="presentation" onclick={() => (peek = null)}></div>
    <div class="peek" style:left="{peekLeft}px" role="dialog" aria-label="{peeked.name}'s output" tabindex="-1" onkeydown={(e) => e.key === "Escape" && (peek = null)}>{@render frame(peeked, false)}</div>
  {/if}
  {#if running}
    <div class="scrim clear" role="presentation" onclick={() => (running = false)}></div>
    <form class="run-pop" onsubmit={start}>
      <!-- svelte-ignore a11y_autofocus -->
      <input aria-label="Command" placeholder="A command to run in this project's tray" bind:value={line} autofocus onkeydown={(e) => e.key === "Escape" && (running = false)} />
      <button class="go" disabled={!line.trim() || !project}>Run</button>
    </form>
  {/if}
  <div class="chips" bind:this={chipsEl}>
    {#each jobs as j (j.id)}
      <button class="tchip" class:on={peek === j.id || pinned.includes(j.id)} data-job={j.id} title={j.command} onclick={() => (pinned.includes(j.id) ? null : (peek = peek === j.id ? null : j.id))} oncontextmenu={(e) => { e.preventDefault(); menu = { x: e.clientX, y: e.clientY, job: j }; }}><i class="st {shape(j)}" role="img" aria-label={j.status}></i>{short(j.name)}</button>
    {/each}
  </div>
  {#if problem}<span class="error small">{problem}</span>{/if}
  {#if !jobs.length}<span class="hint">Ctrl+J run · Ctrl+Shift+J peek · Ctrl+` scratch</span>{/if}
</div>

{#if menu}
  {@const j = menu.job}
  <div class="scrim clear" role="presentation" onclick={() => (menu = null)} oncontextmenu={(e) => { e.preventDefault(); menu = null; }}></div>
  <div class="menu rowmenu" style:left="{menu.x}px" style:top="{Math.max(8, menu.y - 290)}px">
    <button class="it" onclick={() => { menu = null; peek = j.id; }}><span>Show its output</span></button>
    <button class="it" onclick={() => { menu = null; run(command("restart_job", { job: j.id })); }}><span>Run again</span></button>
    <button class="it" disabled={j.status === "exited"} onclick={() => { menu = null; run(command("stop_job", { job: j.id })); }}><span>Stop</span></button>
    <button class="it" onclick={() => { menu = null; if (pinned.includes(j.id)) unpin(j); else pin(j); }}><span>{pinned.includes(j.id) ? "Unpin" : "Pin above the tray"}</span></button>
    <hr />
    <button class="it" onclick={() => { menu = null; navigator.clipboard?.writeText(j.command); }}><span>Copy the command</span></button>
    <button class="it" onclick={() => { menu = null; copyOutput(j); }}><span>Copy its output</span></button>
    <hr />
    <button class="it danger" onclick={() => { menu = null; remove(j); }}><span>Remove from the tray</span></button>
  </div>
{/if}
