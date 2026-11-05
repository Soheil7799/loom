<script>
  import { command } from "./lib/api";

  // What agents ask beyond their conversation: a file to see, one to fill in, a word.
  let { asks } = $props();
  let problem = $state("");

  async function run(promise) {
    problem = "";
    try {
      await promise;
    } catch (e) {
      problem = e.message;
    }
  }

  const what = { open: "wants you to see", edit: "waits for you to fill in", notify: "needs you" };
</script>

<div class="asks">
  <header><h1>Asks</h1></header>
  {#if problem}<p class="error">{problem}</p>{/if}
  {#if asks.length === 0}<p class="dim">Nothing asked.</p>{/if}
  {#each [...asks].reverse() as ask (ask.id)}
    <div class="ask {ask.kind}">
      <p><strong>{ask.title}</strong> {what[ask.kind] ?? ask.kind}{#if ask.path}: <code>{ask.path}</code>{/if}</p>
      {#if ask.message}<p>{ask.message}</p>{/if}
      <p>
        {#if ask.path}<button onclick={() => run(command("ask_open", { id: ask.id }))}>Open in my editor</button>{/if}
        <button onclick={() => run(command("ask_done", { id: ask.id }))}>{ask.kind === "edit" ? "Done" : "Dismiss"}</button>
      </p>
    </div>
  {/each}
</div>
