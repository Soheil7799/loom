<script lang="ts">
  // A project's files, as the window's files panel: the tree, a file open to read and edit
  // (Ctrl+S saves; a file changed on disk since is not overwritten without asking), Ctrl+P to
  // find one; and Git: each repository's changes, their diffs, stage, unstage, throw away,
  // commit, and the newest commits.
  import { untrack } from "svelte";
  import { command } from "./lib/api";
  import type { ProjectsItem } from "./lib/api-types";
  import { unified } from "./lib/diff";
  import Icon from "./Icon.svelte";

  let { projects, project = $bindable(""), findAsked = $bindable(false), mention }: { projects: ProjectsItem[]; project: string; findAsked?: boolean; mention?: (path: string) => void } = $props();
  import { markdown } from "./lib/markdown";
  // A file's kind, by its name: how it is shown, and its icon's colour.
  const ext = (p: string) => p.split("/").pop()!.split(".").slice(1).pop()?.toLowerCase() ?? "";
  const PICTURE: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", avif: "image/avif", bmp: "image/bmp", ico: "image/x-icon" };
  const kind = (p: string) => {
    const e = ext(p);
    if (PICTURE[e]) return "pic";
    if (["md", "markdown", "txt", "rst"].includes(e)) return "doc";
    if (["json", "toml", "yaml", "yml", "ini", "conf", "env", "lock"].includes(e) || /^\./.test(p.split("/").pop()!)) return "conf";
    if (["rs", "ts", "js", "svelte", "py", "go", "kt", "swift", "c", "h", "cpp", "java", "sh", "css", "html", "sql"].includes(e)) return "code";
    return "file";
  };
  let picture = $state<string | null>(null);
  let reading = $state(true);
  // the right-click menu of a row
  let rowMenu = $state<{ x: number; y: number; e: Entry } | null>(null);
  async function rowAct(what: string, e: Entry) {
    rowMenu = null;
    const rel = e.path.startsWith(project + "/") ? e.path.slice(project.length + 1) : e.path;
    if (what === "app" || what === "folder") await call(command("open_path", { path: e.path, how: what }));
    else if (what === "copy") navigator.clipboard?.writeText(e.path);
    else if (what === "copyrel") navigator.clipboard?.writeText(rel);
    else if (what === "mention") mention?.(rel);
    else if (what === "trash" && confirm(`Move ${e.name} to the trash?`)) {
      if (await call(command("trash_file", { path: e.path }))) {
        if (file?.path === e.path) file = null;
        const parent = e.path.slice(0, e.path.lastIndexOf("/"));
        load(parent);
      }
    }
  }
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
  let commits = $state.raw<{ graph?: string; hash: string | null; refs: string; subject: string; author: string; when: string }[]>([]);
  let graph = $state(false);

  $effect(() => { if (!project && projects.length) project = projects[0].id; });
  // Ctrl+P from elsewhere: the page opened here to find a file
  // Ctrl+P (here or from elsewhere): the finder, once per press
  $effect(() => { if (findAsked) { findAsked = false; tab = "files"; finding = true; query = ""; found = []; } });
  const call = async <T,>(p: Promise<T>) => { problem = ""; try { return await p; } catch (e) { problem = (e as Error).message; } };

  async function load(folder: string) {
    const got = await call(command("files_list", { folder }));
    if (got) tree = { ...tree, [folder]: got.entries };
  }
  // Another project: its tree (unsaved edits asked about first; kept, the project stays).
  let shownProject = "";
  $effect(() => {
    if (!project || project === shownProject) return;
    if (untrack(() => dirty) && !confirm("Leave this file without saving it?")) { project = shownProject; return; }
    shownProject = project;
    tree = {};
    open = new Set();
    file = null;
    load(project);
    if (untrack(() => tab) === "git") loadGit();
  });
  function toggle(e: Entry) {
    const next = new Set(open);
    if (!next.delete(e.path)) { next.add(e.path); if (!tree[e.path]) load(e.path); }
    open = next;
  }
  const dirty = $derived(!!file && file.text !== null && file.text !== file.saved);
  async function show(path: string) {
    if (dirty && !confirm("Leave this file without saving it?")) return;
    picture = null;
    if (kind(path) === "pic") {
      const got = await call(command("file_read", { path, bytes: true }));
      if (got) { picture = `data:${PICTURE[ext(path)]};base64,${got.data}`; file = { path, text: null, modified: 0, saved: "" }; }
      return;
    }
    const got = await call(command("file_read", { path }));
    if (got) file = { path, text: got.text, modified: got.modified, saved: got.text ?? "" };
    // a document opens to be read; its text a click away
    reading = kind(path) === "doc";
  }
  async function save(force = false) {
    if (!file || file.text === null) return;
    try {
      // what went: typed meanwhile is still unsaved
      const sent = file.text;
      const got = await command("file_write", { path: file.path, text: sent, ...(force ? {} : { modified: file.modified }) });
      file = { ...file!, modified: got.modified, saved: sent };
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
    const log = repo ? await call(command("git_log", { repo, limit: graph ? 200 : 60, graph })) : null;
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
            <button class="trow" class:sel={file?.path === e.path} style:padding-left="{10 + depth * 14}px" aria-expanded={e.dir ? open.has(e.path) : undefined} onclick={() => (e.dir ? toggle(e) : show(e.path))} oncontextmenu={(ev) => { ev.preventDefault(); rowMenu = { x: ev.clientX, y: ev.clientY, e }; }}>
              {#if e.dir}<Icon name={open.has(e.path) ? "chev-d" : "chev-r"} size={12} />{:else}<span class="ficon {kind(e.name)}"><Icon name={kind(e.name) === "pic" ? "image" : kind(e.name) === "doc" ? "notes" : "file"} size={12} /></span>{/if}<span>{e.name}</span>
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
            {#if file.text !== null && kind(file.path) === "doc"}<button class="btn small" onclick={() => (reading = !reading)}>{reading ? "Edit" : "Read"}</button>{/if}
            <button class="btn small" onclick={() => command("edit_file", { path: file!.path }).catch((e) => (problem = e.message))}>Open in my editor</button>
            {#if file.text !== null}<button class="btn small suggested" disabled={!dirty} onclick={() => save()}>Save</button>{/if}
          </div>
          {#if picture}
            <div class="picture"><img src={picture} alt={file.path.split("/").pop()} /></div>
          {:else if file.text === null}
            <p class="quiet">Not text, or too big to show here. <button class="btn small" onclick={() => call(command("open_path", { path: file!.path, how: "app" }))}>Open in its app</button></p>
          {:else if kind(file.path) === "doc" && reading}
            <div class="note md doc-read">{@html markdown(file.text)}</div>
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
                {#if c.code !== "??" && !"AR".includes(c.code[0])}<button class="tiny" title="Throw its changes away" aria-label="Throw away {c.file}" onclick={() => git("discard", [c.file])}><Icon name="x" size={13} /></button>{/if}
              </span>
            </div>
          {/each}
          {#if current.changes.some((c) => staged(c.code))}
            <form class="commit" onsubmit={(e) => { e.preventDefault(); git("commit"); }}>
              <input aria-label="Commit message" placeholder="What this commit does" bind:value={message} />
              <button class="go" disabled={!message.trim()}>Commit</button>
            </form>
          {/if}
          <h3>Newest commits <label class="quiet small graph-toggle"><input type="checkbox" checked={graph} onchange={(e) => { graph = e.currentTarget.checked; loadGit(); }} /> every branch, as a graph</label></h3>
          {#each commits as c, i (i)}
            <p class="commitrow">{#if graph}<span class="graph">{c.graph}</span>{/if}{#if c.hash}<code>{c.hash}</code>{#each (c.refs ?? "").split(", ").filter(Boolean) as r (r)}<span class="refs" class:head={r.startsWith("HEAD")} class:tag={r.startsWith("tag:")}>{r.replace("HEAD -> ", "")}</span>{/each} {c.subject} <span class="quiet">· {c.author}, {c.when}</span>{/if}</p>
          {/each}
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

  {#if rowMenu}
    <div class="scrim clear" role="presentation" onclick={() => (rowMenu = null)} oncontextmenu={(e) => { e.preventDefault(); rowMenu = null; }}></div>
    <div class="menu rowmenu" style:left="{rowMenu.x}px" style:top="{rowMenu.y}px">
      {#if !rowMenu.e.dir}<button class="it" onclick={() => rowAct("app", rowMenu!.e)}><Icon name="file" size={13} /><span>Open in its app</span></button>{/if}
      <button class="it" onclick={() => rowAct("folder", rowMenu!.e)}><Icon name="folder" size={13} /><span>Show in the file manager</span></button>
      {#if mention}<button class="it" onclick={() => rowAct("mention", rowMenu!.e)}><Icon name="send" size={13} /><span>Mention in the chat</span></button>{/if}
      <button class="it" onclick={() => rowAct("copy", rowMenu!.e)}><Icon name="copy" size={13} /><span>Copy its path</span></button>
      <button class="it" onclick={() => rowAct("copyrel", rowMenu!.e)}><Icon name="copy" size={13} /><span>Copy its path in the project</span></button>
      <button class="it danger" onclick={() => rowAct("trash", rowMenu!.e)}><Icon name="x" size={13} /><span>Move to the trash</span></button>
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
