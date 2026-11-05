<script>
  import { command } from "./lib/api";

  // A question entry: a permission's options as buttons, a form's fields as inputs.
  let { entry, agent } = $props();
  let content = $state({});
  let problem = $state("");

  let fields = $derived(Object.entries(entry.form?.properties ?? {}));

  async function answer(params) {
    problem = "";
    try {
      await command("answer", { agent, entry: entry.id, ...params });
    } catch (e) {
      problem = e.message;
    }
  }

  function toggle(name, value, on) {
    const now = new Set(content[name] ?? []);
    if (on) now.add(value); else now.delete(value);
    content = { ...content, [name]: [...now] };
  }
</script>

<div class="question {entry.status}">
  <p><strong>{entry.text}</strong></p>
  {#if entry.detail}<pre>{entry.detail}</pre>{/if}
  {#if entry.status !== "awaiting_user"}
    <p class="dim">{entry.status === "done" ? `→ ${entry.answer ?? "Answered"}` : entry.answer || "Not answered"}</p>
  {:else if entry.form}
    <form onsubmit={(e) => { e.preventDefault(); answer({ content }); }}>
      {#each fields as [name, field] (name)}
        <fieldset>
          {#if field.title}<legend>{field.title}</legend>{/if}
          {#if field.description}<p class="dim">{field.description}</p>{/if}
          {#if field.oneOf}
            {#each field.oneOf as choice}
              <label><input type="radio" name={name} value={choice.const} onchange={() => (content = { ...content, [name]: choice.const })} /> {choice.title ?? choice.const}</label>
            {/each}
          {:else if field.items?.anyOf}
            {#each field.items.anyOf as choice}
              <label><input type="checkbox" onchange={(e) => toggle(name, choice.const, e.currentTarget.checked)} /> {choice.title ?? choice.const}</label>
            {/each}
          {:else if field.type === "boolean"}
            <label><input type="checkbox" onchange={(e) => (content = { ...content, [name]: e.currentTarget.checked })} /> yes</label>
          {:else}
            <input oninput={(e) => (content = { ...content, [name]: e.currentTarget.value })} />
          {/if}
        </fieldset>
      {/each}
      <button>Send</button>
      <button type="button" onclick={() => answer({ skip: true })}>Skip</button>
    </form>
  {:else}
    <div class="options">
      {#each entry.options ?? [] as option (option.id)}
        <button class={option.kind} onclick={() => answer({ option: option.id })}>{option.name}</button>
      {/each}
    </div>
  {/if}
  {#if problem}<p class="error">{problem}</p>{/if}
</div>
