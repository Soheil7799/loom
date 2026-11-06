<script lang="ts">
  // What an agent works through, above its composer: Claude's plan (its to-do list, folded to
  // one line once it is under way) and the workflows it runs (their agents, Stop).
  import { command, follow, type Item } from "./lib/api";
  import Icon from "./Icon.svelte";

  let { agent }: { agent: string } = $props();
  let plan = $state.raw<Item[]>([]);
  let workflows = $state.raw<Item[]>([]);
  let open = $state(false);
  let problem = $state("");
  $effect(() => follow(`plan/${agent}`, (f) => (plan = f.items)));
  $effect(() => follow(`workflows/${agent}`, (f) => (workflows = f.items)));
  const done = $derived(plan.filter((p) => p.status === "completed").length);
  const now = $derived(plan.find((p) => p.status === "in_progress"));
  const live = $derived(workflows.filter((w) => w.running));
</script>

{#if plan.length && done < plan.length}
  <div class="plan-box">
    <button class="plan-head" aria-expanded={open} onclick={() => (open = !open)}>
      <Icon name={open ? "chev-d" : "chev-r"} size={12} /><b>Plan</b><span class="quiet">{done} of {plan.length} done{#if now}{" · now: "}{now.content}{/if}</span>
    </button>
    {#if open}
      <ol>
        {#each plan as p (p.id)}<li class={p.status}><span class="mark">{p.status === "completed" ? "✓" : p.status === "in_progress" ? "▸" : "○"}</span>{p.content}</li>{/each}
      </ol>
    {/if}
  </div>
{/if}
{#each live as w (w.id)}
  <div class="plan-box wf">
    <Icon name="grid" size={13} /><b>{w.name}</b><span class="quiet">{w.agents ? `${w.done} of ${w.agents} agents done` : "starting"}{#if w.members?.length}{" · "}{w.members.filter((m: Item) => !m.done).map((m: Item) => m.label).slice(0, 3).join(", ")}{/if}</span>
    <span class="grow"></span>
    <button class="btn small deny" onclick={() => command("stop_workflow", { agent, workflow: w.id }).catch((e) => (problem = e.message))}>Stop</button>
  </div>
{/each}
{#if problem}<p class="error">{problem}</p>{/if}
