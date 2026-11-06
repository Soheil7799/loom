<script lang="ts">
  // The scratch terminal (Ctrl+`): your login shell in the project, a job of its tray named
  // "scratch", typed into here; started (or again, once it exited) when opened.
  import { untrack } from "svelte";
  import { command } from "./lib/api";
  import type { JobsItem } from "./lib/api-types";
  import Terminal from "./Terminal.svelte";
  import Icon from "./Icon.svelte";

  let { project, jobs, close }: { project: string; jobs: JobsItem[]; close: () => void } = $props();
  let problem = $state("");
  const job = $derived([...jobs].reverse().find((j) => j.project_id === project && j.name === "scratch"));
  // Opened (or moved to another project): a shell there, the one that ended started again.
  // While open, an ended shell waits for "Start again" (`exit` ends it; one that can't
  // start doesn't loop).
  let opened = "";
  function start() {
    const run = !job ? command("run_job", { project, name: "scratch", command: 'exec "${SHELL:-bash}" -l' }) : job.status === "exited" ? command("restart_job", { job: job.id }) : null;
    run?.catch((e) => (problem = e.message));
  }
  $effect(() => {
    if (opened === project) return;
    opened = project;
    untrack(start);
  });
</script>

<section class="scratch" aria-label="Scratch terminal">
  <div class="job-bar"><b>Scratch</b><span class="quiet small">your shell in {project.split("/").filter(Boolean).pop()}</span><span class="grow"></span><button class="tiny" aria-label="Hide the scratch terminal (Ctrl+`)" onclick={close}><Icon name="x" size={14} /></button></div>
  {#if problem}<p class="error">{problem}</p>{/if}
  {#if job?.status === "exited"}<p class="quiet small">The shell ended. <button class="btn small" onclick={start}>Start again</button></p>{/if}
  {#if job}{#key job.id}<Terminal job={job.id} />{/key}{/if}
</section>
