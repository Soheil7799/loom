<script lang="ts">
  // The jobs: every project's, newest first, one open in its terminal; a command to run in a
  // project's tray; Stop and Run again.
  import { command } from "./lib/api";
  import type { JobsItem, ProjectsItem } from "./lib/api-types";
  import Icon from "./Icon.svelte";
  import Terminal from "./Terminal.svelte";

  let { jobs, projects, open = $bindable(null) }: { jobs: JobsItem[]; projects: ProjectsItem[]; open: string | null } = $props();
  let project = $state("");
  let line = $state("");
  let problem = $state("");

  $effect(() => { if (!project && projects.length) project = projects[0].id; });
  const newest = $derived([...jobs].reverse());
  const job = $derived(jobs.find((j) => j.id === open) ?? newest[0]);
  const name = (id: string) => projects.find((p) => p.id === id)?.name ?? id;

  async function run<T>(p: Promise<T>) {
    problem = "";
    try { return await p; } catch (e) { problem = (e as Error).message; }
  }
  async function start(e: SubmitEvent) {
    e.preventDefault();
    const answer = await run(command("run_job", { project, command: line.trim() }));
    if (answer) { line = ""; open = answer.job; }
  }
</script>

<section class="jobs-page" aria-label="Jobs">
  <header class="header"><span class="headline"><span class="title">Jobs</span></span></header>
  <form class="runbar" onsubmit={start}>
    <select class="chip" aria-label="Project" bind:value={project}>
      {#each projects as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
    </select>
    <input aria-label="Command" placeholder="A command to run in the project's tray" bind:value={line} />
    <button class="go" disabled={!line.trim() || !projects.length}>Run</button>
  </form>
  {#if problem}<p class="error">{problem}</p>{/if}
  <div class="jobs-split">
    <ul class="jobs-list">
      {#each newest as j (j.id)}
        {@const shape = j.status !== "exited" ? "work" : j.exit_code === 0 ? "done" : "fail"}
        <li>
          <button class="arow" class:sel={j.id === job?.id} aria-current={j.id === job?.id ? "true" : undefined} onclick={() => (open = j.id)}>
            <i class="st {shape}" role="img" aria-label={j.status}></i>
            <span class="t">{j.name}</span>
            <span class="s">{name(j.project_id)} · {j.status}{j.exit_code != null && j.status === "exited" ? ` ${j.exit_code}` : ""}</span>
          </button>
        </li>
      {:else}
        <li class="empty">No jobs yet.</li>
      {/each}
    </ul>
    {#if job}
      <div class="job">
        <div class="job-bar">
          <code>{job.command}</code><span class="grow"></span>
          {#if job.status === "running"}<button class="btn" onclick={() => run(command("stop_job", { job: job.id }))}><Icon name="stop" size={13} /> Stop</button>{/if}
          <button class="btn" onclick={() => run(command("restart_job", { job: job.id }))}><Icon name="undo" size={13} /> Run again</button>
        </div>
        {#key job.id}<Terminal job={job.id} />{/key}
      </div>
    {/if}
  </div>
</section>
