<script lang="ts">
  // A question in the conversation: a permission's options as numbered buttons (1-9 on the
  // keyboard), a form's fields as inputs. Answered ones show how.
  import { command } from "./lib/api";
  import type { Entry } from "./lib/turns";

  let { entry, agent }: { entry: Entry; agent: string } = $props();
  type Option = { id: string; name: string; kind?: string };
  type Field = { title?: string; description?: string; type?: string; oneOf?: { const: string; title?: string }[]; items?: { anyOf?: { const: string; title?: string }[] } };
  let content = $state<Record<string, unknown>>({});
  let problem = $state("");

  const options = $derived((entry.options ?? []) as Option[]);
  const fields = $derived(Object.entries(((entry.form as { properties?: Record<string, Field> } | undefined)?.properties ?? {})));
  const waiting = $derived(entry.status === "awaiting_user");

  async function answer(params: { option?: string; content?: Record<string, unknown>; skip?: boolean }) {
    problem = "";
    try { await command("answer", { agent, entry: entry.id, ...params }); } catch (e) { problem = (e as Error).message; }
  }
  function toggle(name: string, value: string, on: boolean) {
    const now = new Set((content[name] as string[] | undefined) ?? []);
    if (on) now.add(value); else now.delete(value);
    content = { ...content, [name]: [...now] };
  }
  function keys(e: KeyboardEvent) {
    if (!waiting || entry.form || e.ctrlKey || e.metaKey) return;
    if ((e.target as HTMLElement)?.closest("textarea, input")) return;
    const n = Number(e.key);
    if (n >= 1 && n <= options.length) { e.preventDefault(); answer({ option: options[n - 1].id }); }
  }
</script>

<svelte:window onkeydown={keys} />

<div class="permission-card" class:answered={!waiting}>
  <div class="qh">{#if waiting}<span class="needs">waits for you</span>{/if}<b>{entry.text}</b></div>
  {#if entry.detail}<pre class="code-block">{entry.detail}</pre>{/if}
  {#if !waiting}
    <p class="quiet">{entry.status === "done" ? `Answered: ${entry.answer ?? "yes"}` : entry.answer || "Not answered"}</p>
  {:else if entry.form}
    <form onsubmit={(e) => { e.preventDefault(); answer({ content }); }}>
      {#each fields as [name, field] (name)}
        <fieldset>
          {#if field.title}<legend>{field.title}</legend>{/if}
          {#if field.description}<p class="quiet">{field.description}</p>{/if}
          {#if field.oneOf}
            {#each field.oneOf as choice}<label><input type="radio" {name} value={choice.const} onchange={() => (content = { ...content, [name]: choice.const })} /> {choice.title ?? choice.const}</label>{/each}
          {:else if field.items?.anyOf}
            {#each field.items.anyOf as choice}<label><input type="checkbox" onchange={(e) => toggle(name, choice.const, e.currentTarget.checked)} /> {choice.title ?? choice.const}</label>{/each}
          {:else if field.type === "boolean"}
            <label><input type="checkbox" onchange={(e) => (content = { ...content, [name]: e.currentTarget.checked })} /> yes</label>
          {:else}
            <input aria-label={field.title ?? name} oninput={(e) => (content = { ...content, [name]: e.currentTarget.value })} />
          {/if}
        </fieldset>
      {/each}
      <div class="controls"><button class="btn suggested">Send</button><button type="button" class="btn" onclick={() => answer({ skip: true })}>Skip</button></div>
    </form>
  {:else}
    <div class="controls">
      {#each options as o, i (o.id)}
        <button class="btn" class:suggested={i === 0 && o.kind?.startsWith("allow")} class:deny={o.kind?.startsWith("reject")} onclick={() => answer({ option: o.id })}>{i + 1}  {o.name}</button>
      {/each}
      {#if options.length}<span class="quiet small">1–{Math.min(options.length, 9)} on the keyboard</span>{/if}
    </div>
  {/if}
  {#if problem}<p class="error">{problem}</p>{/if}
</div>
