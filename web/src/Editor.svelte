<script lang="ts">
  // The open file, beside the chat, as the window has it: the files opened lately as tabs
  // over it, read (a document rendered, a picture shown) or edited (Ctrl+S saves; a file
  // changed on disk since is not overwritten without asking).
  import { command } from "./lib/api";
  import { markdown } from "./lib/markdown";
  import Icon from "./Icon.svelte";

  let { path = $bindable(null), project, mention }: { path: string | null; project: string; mention?: (rel: string) => void } = $props();

  const ext = (p: string) => p.split("/").pop()!.split(".").slice(1).pop()?.toLowerCase() ?? "";
  const PICTURE: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", avif: "image/avif", bmp: "image/bmp", ico: "image/x-icon" };
  const isDoc = (p: string) => ["md", "markdown"].includes(ext(p));
  const base = (p: string) => p.split("/").pop() ?? p;
  const rel = (p: string) => (project && p.startsWith(project + "/") ? p.slice(project.length + 1) : p);

  let file = $state<{ path: string; text: string | null; modified: number; saved: string } | null>(null);
  let picture = $state<string | null>(null);
  let reading = $state(true);
  let problem = $state("");
  // the files opened lately, newest last: the tabs
  let recent = $state<string[]>([]);
  const dirty = $derived(!!file && file.text !== null && file.text !== file.saved);

  // Another file asked for: unsaved edits asked about first (kept: the file stays).
  $effect(() => {
    const want = path;
    if (!want || want === file?.path) return;
    if (dirty && !confirm("Leave this file without saving it?")) { path = file!.path; return; }
    load(want);
  });
  async function load(p: string) {
    problem = "";
    picture = null;
    try {
      if (PICTURE[ext(p)]) {
        const got = await command("file_read", { path: p, bytes: true });
        picture = `data:${PICTURE[ext(p)]};base64,${got.data}`;
        file = { path: p, text: null, modified: 0, saved: "" };
      } else {
        const got = await command("file_read", { path: p });
        file = { path: p, text: got.text, modified: got.modified, saved: got.text ?? "" };
      }
      reading = isDoc(p);
      recent = [...recent.filter((r) => r !== p), p].slice(-8);
    } catch (e) {
      problem = (e as Error).message;
    }
  }
  async function save(force = false) {
    if (!file || file.text === null) return;
    try {
      // what went: typed meanwhile is still unsaved
      const { path: saving, text: sent } = file;
      const got = await command("file_write", { path: saving, text: sent, ...(force ? {} : { modified: file.modified }) });
      // another file opened meanwhile: this answer is the saved one's, not its
      if (file?.path === saving) file = { ...file, modified: got.modified, saved: sent };
      problem = "";
    } catch (e) {
      const why = (e as Error).message;
      if (why.includes("changed on disk") && confirm("It changed on disk since you opened it. Overwrite it with yours?")) return save(true);
      problem = why;
    }
  }
  function close() {
    if (dirty && !confirm("Close this file without saving it?")) return;
    file = null;
    path = null;
  }
  function untab(p: string) {
    recent = recent.filter((r) => r !== p);
    if (file?.path === p) { if (recent.length) path = recent.at(-1)!; else close(); }
  }
</script>

<svelte:window onkeydown={(e) => { if ((e.ctrlKey || e.metaKey) && e.key === "s" && file) { e.preventDefault(); save(); } }} />

{#if file}
  <section class="editor-pane" aria-label="The open file">
    {#if recent.length > 1}
      <div class="etabs">
        {#each recent as r (r)}<span class="etab" class:current={r === file.path}><button title={r} onclick={() => (path = r)}>{base(r)}</button><button class="x" aria-label="Take {base(r)} off the tabs" onclick={() => untab(r)}>×</button></span>{/each}
      </div>
    {/if}
    <div class="ebar">
      <code title={file.path}>{rel(file.path)}</code>{#if dirty}<span class="needs">not saved</span>{/if}<span class="grow"></span>
      {#if file.text !== null && isDoc(file.path)}<button class="btn small" onclick={() => (reading = !reading)}>{reading ? "Edit" : "Read"}</button>{/if}
      {#if mention}<button class="btn small" title="Mention it in the chat" onclick={() => mention!(rel(file!.path))}>Ask Claude</button>{/if}
      <button class="btn small" onclick={() => command("edit_file", { path: file!.path }).catch((e) => (problem = e.message))}>Open in my editor</button>
      {#if file.text !== null}<button class="btn small suggested" disabled={!dirty} onclick={() => save()}>Save</button>{/if}
      <button class="tiny" aria-label="Close the file" title="Close" onclick={close}><Icon name="x" size={14} /></button>
    </div>
    {#if problem}<p class="error" role="alert">{problem}</p>{/if}
    {#if picture}
      <div class="picture"><img src={picture} alt={base(file.path)} /></div>
    {:else if file.text === null}
      <p class="quiet">Not text, or too big to show here. <button class="btn small" onclick={() => command("open_path", { path: file!.path, how: "app" }).catch((e) => (problem = e.message))}>Open in its app</button></p>
    {:else if isDoc(file.path) && reading}
      <div class="note md doc-read">{@html markdown(file.text)}</div>
    {:else}
      <textarea class="code" aria-label="The file's text" spellcheck="false" bind:value={file.text}></textarea>
    {/if}
  </section>
{:else if problem}
  <section class="editor-pane"><p class="error" role="alert">{problem}</p><button class="btn small" onclick={() => { problem = ""; path = null; }}>Close</button></section>
{/if}
