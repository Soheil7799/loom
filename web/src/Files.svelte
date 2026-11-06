<script lang="ts">
  // A project's files, as the window's files panel: the tree, a file open to read and edit
  // (Ctrl+S saves; a file changed on disk since is not overwritten without asking), Ctrl+P to
  // find one; and Git: each repository's changes, their diffs, stage, unstage, throw away,
  // commit, and the newest commits.
  import { command } from "./lib/api";
  import type { ProjectsItem } from "./lib/api-types";
  import { unified } from "./lib/diff";
  import Icon from "./Icon.svelte";

  let { projects, project = $bindable(""), findAsked = 0 }: { projects: ProjectsItem[]; project: string; findAsked?: number } = $props();
  type Entry = { name: string; path: string; dir: boolean };
  type Repo = { path: string; name: string; branch: string; ahead: number; behind: number; changes: { code: string; file: string }[] };
  let tab = $state<"files" | "git">("files");
  let tree = $state<Record<string, Entry[]>>({});
  let open = $state<Set<string>>(new Set());
  let file = $state<{ path: string; text: string | null; modified: number; saved: string } | null>(null);
  let problem = $state("");
  let finding = $state(false);
  let query = $state("");
  let found = $state<string[]>([]);
  let repos = $state.raw<Repo[]>([]);
  let repo = $state("");
  let diff = $state<{ file: string; staged: boolean; text: string } | null>(null);
  let message = $state("");
  let commits = $state.raw<{ hash: string; refs: string; subject: string; author: string; when: string }[]>([]);

  $effect(() => { if (!project && projects.length) project = projects[0].id; });
  // Ctrl+P from elsewhere: the page opened here to find a file
  $effect(() => { if (findAsked) { tab = "files"; finding = true; query = ""; found = []; } });
  const call = async <T,>(p: Promise<T>) => { problem = ""; try { return await p; } catch (e) { problem = (e as Error).message; } };

  async function load(folder: string) {
    const got = await call(command("files_list", { folder }));
    if (got) tree = { ...tree, [folder]: got.entries };
  }
  $effect(() => { if (project) { tree = {}; open = new Set(); file = null; load(project); if (tab === "git") loadGit(); } });
  function toggle(e: Entry) {
    const next = new Set(open);
    if (!next.delete(e.path)) { next.add(e.path); if (!tree[e.path]) load(e.path); }
    open = next;
  }
  const dirty = $derived(!!file && file.text !== null && file.text !== file.saved);
  async function show(path: string) {
    if (dirty && !confirm("Leave this file without saving it?")) return;
    const got = await call(command("file_read", { path }));
    if (got) file = { path, text: got.text, modified: got.modified, saved: got.text ?? "" };
  }
  async function save(force = false) {
    if (!file || file.text === null) return;
    try {
      const got = await command("file_write", { path: file.path, text: file.text, ...(force ? {} : { modified: file.modified }) });
      file = { ...file, modified: got.modified, saved: file.text };
      problem = "";
    } catch (e) {
      const why = (e as Error).message;
      if (why.includes("changed on disk") && confirm("It changed on disk since you opened it. Overwrite it with yours?")) return save(true);
      problem = why;
    }
  }
  const rel = (path: string) => (path.startsWith(project + "/") ? path.slice(project.length + 1) : path);

  async function find() {
    const got = query.trim() ? await call(command("files_find", { project, query, limit: 30 })) : null;
    found = got?.files ?? [];
  }
  function keys(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === "s" && file) { e.preventDefault(); save(); }
    if (e.key === "Escape" && finding) finding = false;
  }

  async function loadGit() {
    const got = await call(command("git_repos", { project }));
    repos = got?.repos ?? [];
    if (!repos.some((r) => r.path === repo)) repo = repos[0]?.path ?? "";
    diff = null;
    const log = repo ? await call(command("git_log", { repo, limit: 60 })) : null;
    commits = log?.commits ?? [];
  }
  const current = $derived(repos.find((r) => r.path === repo));
  // git's two letters: the first the index (staged), the second the work tree
  const staged = (code: string) => code[0] !== " " && code[0] !== "?";
  const unstaged = (code: string) => code[1] !== " ";
  async function showDiff(f: string, isStaged: boolean) {
    const got = await call(command("git_diff", { repo, file: f, staged: isStaged }));
    diff = { file: f, staged: isStaged, text: got?.diff || "(a new file: nothing to compare with)" };
  }
  async function git(act: string, files: string[] = []) {
    if (act === "discard" && !confirm(`Throw away the changes to ${files.join(", ")}? They go back to the last commit. This can't be undone.`)) return;
    const done = await call(command("git_do", { repo, act, ...(files.length ? { files } : {}), ...(act === "commit" ? { message } : {}) }));
    if (done && act === "commit") message = "";
    loadGit();
  }
</script>

<svelte:window onkeydown={keys} />

<section class="files-page" aria-label="Files">
  <header class="header">
    <span class="headline"><span class="title">Files</span></span>
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === "files"} class:on={tab === "files"} onclick={() => (tab = "files")}>Files</button>
      <button role="tab" aria-selected={tab === "git"} class:on={tab === "git"} onclick={() => { tab = "git"; loadGit(); }}>Git</button>
    </div>
    <span class="grow"></span>
    <select class="chip" aria-label="Project" bind:value={project}>{#each projects as p (p.id)}<option value={p.id}>{p.name}</option>{/each}</select>
    <button class="tiny" aria-label="Find a file (Ctrl+P)" title="Find a file (Ctrl+P)" onclick={() => { finding = true; tab = "files"; }}><Icon name="search" size={15} /></button>
  </header>
  {#if problem}<p class="error" role="alert">{problem}</p>{/if}

  {#if tab === "files"}
    <div class="files-split">
      <nav class="tree" aria-label="The project's files">
        {#snippet folder(path: string, depth: number)}
          {#each tree[path] ?? [] as e (e.path)}
            <button class="trow" class:sel={file?.path === e.path} style:padding-left="{10 + depth * 14}px" aria-expanded={e.dir ? open.has(e.path) : undefined} onclick={() => (e.dir ? toggle(e) : show(e.path))}>
              {#if e.dir}<Icon name={open.has(e.path) ? "chev-d" : "chev-r"} size={12} />{:else}<span class="fdot"></span>{/if}<span>{e.name}</span>
            </button>
            {#if e.dir && open.has(e.path)}{@render folder(e.path, depth + 1)}{/if}
          {/each}
        {/snippet}
        {@render folder(project, 0)}
      </nav>
      <div class="editor">
        {#if file}
          <div class="ebar">
            <code title={file.path}>{rel(file.path)}</code>{#if dirty}<span class="needs">not saved</span>{/if}<span class="grow"></span>
            <button class="btn small" onclick={() => command("edit_file", { path: file!.path }).catch((e) => (problem = e.message))}>Open in my editor</button>
            {#if file.text !== null}<button class="btn small suggested" disabled={!dirty} onclick={() => save()}>Save</button>{/if}
          </div>
          {#if file.text === null}
            <p class="quiet">Not text, or too big to show here.</p>
          {:else}
            <textarea class="code" aria-label="The file's text" spellcheck="false" bind:value={file.text}></textarea>
          {/if}
        {:else}
          <p class="empty">Pick a file, or find one with Ctrl+P.</p>
        {/if}
      </div>
    </div>
  {:else}
    <div class="files-split">
      <div class="git">
        {#if repos.length > 1}<select class="chip" aria-label="Repository" bind:value={repo} onchange={loadGit}>{#each repos as r (r.path)}<option value={r.path}>{r.name}</option>{/each}</select>{/if}
        {#if current}
          <p class="quiet small">{current.name} · {current.branch}{#if current.ahead}{" · "}↑{current.ahead} to push{/if}{#if current.behind}{" · "}↓{current.behind} to pull{/if}{current.changes.length ? "" : " · clean"}</p>
          {#each current.changes as c (c.file)}
            <div class="grow-row">
              <button class="trow" class:sel={diff?.file === c.file} onclick={() => showDiff(c.file, staged(c.code) && !unstaged(c.code))}><span class="badge" class:st={staged(c.code)}>{c.code.trim()}</span><span>{c.file}</span></button>
              <span class="gacts">
                {#if unstaged(c.code)}<button class="tiny" title="Stage: put it in the next commit" aria-label="Stage {c.file}" onclick={() => git("stage", [c.file])}><Icon name="plus" size={13} /></button>{/if}
                {#if staged(c.code)}<button class="tiny" title="Unstage: leave it out of the next commit" aria-label="Unstage {c.file}" onclick={() => git("unstage", [c.file])}><Icon name="back" size={13} /></button>{/if}
                {#if c.code !== "??"}<button class="tiny" title="Throw its changes away" aria-label="Throw away {c.file}" onclick={() => git("discard", [c.file])}><Icon name="x" size={13} /></button>{/if}
              </span>
            </div>
          {/each}
          {#if current.changes.some((c) => staged(c.code))}
            <form class="commit" onsubmit={(e) => { e.preventDefault(); git("commit"); }}>
              <input aria-label="Commit message" placeholder="What this commit does" bind:value={message} />
              <button class="go" disabled={!message.trim()}>Commit</button>
            </form>
          {/if}
          <h3>Newest commits</h3>
          {#each commits as c (c.hash)}<p class="commitrow"><code>{c.hash}</code>{#if c.refs}<span class="refs">{c.refs}</span>{/if} {c.subject} <span class="quiet">· {c.author}, {c.when}</span></p>{/each}
        {:else}
          <p class="empty">No git repository in this project.</p>
        {/if}
      </div>
      <div class="editor">
        {#if diff}
          <div class="ebar"><code>{diff.file}</code><span class="quiet small">{diff.staged ? "staged" : "not staged yet"}</span></div>
          <div class="diffs">
            {#each unified(diff.text) as h, i (i)}
              <div class="diff"><div class="f"><span>{h.head}</span></div><pre>{#each h.lines as l, k (k)}<span class={l.t === "+" ? "add" : l.t === "-" ? "del" : ""}>{l.t}{l.text}
</span>{/each}</pre></div>
            {:else}<p class="quiet">{diff.text}</p>{/each}
          </div>
        {:else}
          <p class="empty">Pick a changed file to see its diff.</p>
        {/if}
      </div>
    </div>
  {/if}

  {#if finding}
    <div class="scrim" role="presentation" onclick={() => (finding = false)}></div>
    <div class="finder" role="dialog" aria-modal="true" aria-label="Find a file">
      <!-- svelte-ignore a11y_autofocus -->
      <input placeholder="A file's name…" bind:value={query} oninput={find} autofocus onkeydown={(e) => { if (e.key === "Enter" && found[0]) { finding = false; show(`${project}/${found[0]}`); } }} />
      {#each found as f (f)}<button class="trow" onclick={() => { finding = false; show(`${project}/${f}`); }}>{f}</button>{/each}
    </div>
  {/if}
</section>
