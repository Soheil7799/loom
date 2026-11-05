<script>
  import { follow, command, fromBase64 } from "./lib/api";

  let { projects } = $props();
  let jobs = $state.raw([]);
  let open = $state(null);
  let output = $state("");
  let project = $state("");
  let line = $state("");
  let problem = $state("");

  $effect(() => follow("jobs", (f) => (jobs = [...f.items].reverse())));

  // A job's terminal as text: its screen, then the bytes after it, escapes taken out.
  $effect(() => {
    if (!open) return;
    return follow(`job/${open}`, (f) => {
      const item = f.items[0];
      if (!item) return (output = "");
      const screen = fromBase64(item.screen);
      const tail = item.tail ?? new Uint8Array();
      const all = new Uint8Array(screen.length + tail.length);
      all.set(screen);
      all.set(tail, screen.length);
      output = plain(new TextDecoder().decode(all));
    });
  });

  function plain(text) {
    return text
      .replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, "")
      .replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, "")
      .replace(/\x1b[()][0-9A-Za-z]/g, "")
      .replace(/\x1b./g, "")
      .replace(/\r\n/g, "\n")
      .replace(/\r/g, "");
  }

  async function run(promise) {
    problem = "";
    try {
      return await promise;
    } catch (e) {
      problem = e.message;
    }
  }

  async function start() {
    const answer = await run(command("run_job", { project: project || projects[0]?.id, command: line }));
    if (answer) {
      line = "";
      open = answer.job;
    }
  }

  const name = (id) => projects.find((p) => p.id === id)?.name ?? id;
</script>

<div class="jobs">
  <header><h1>Jobs</h1></header>
  <form class="run" onsubmit={(e) => { e.preventDefault(); start(); }}>
    <select bind:value={project}>
      {#each projects as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
    </select>
    <input placeholder="a command to run in the project's tray" bind:value={line} />
    <button disabled={!line.trim()}>Run</button>
  </form>
  {#if problem}<p class="error">{problem}</p>{/if}
  <div class="split">
    <ul>
      {#each jobs as job (job.id)}
        <li>
          <button class:on={open === job.id} onclick={() => (open = job.id)}>
            <span class="dot {job.status === 'exited' ? (job.exit_code === 0 ? 'done' : 'failed') : 'working'}"></span>
            {job.name} <span class="dim">{name(job.project_id)} · {job.status}{job.exit_code !== null && job.exit_code !== undefined ? ` ${job.exit_code}` : ""}</span>
          </button>
        </li>
      {/each}
    </ul>
    {#if open}
      {@const job = jobs.find((j) => j.id === open)}
      <section>
        {#if job}
          <p>
            <code>{job.command}</code>
            {#if job.status === "running"}<button onclick={() => run(command("stop_job", { job: job.id }))}>Stop</button>{/if}
            <button onclick={() => run(command("restart_job", { job: job.id }))}>Run again</button>
          </p>
        {/if}
        <pre class="terminal">{output}</pre>
      </section>
    {/if}
  </div>
</div>
