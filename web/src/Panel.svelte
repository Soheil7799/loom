<script lang="ts">
  // The right panel, as the window's files panel (Ctrl+E): Agent (the files this agent wrote),
  // Project (its tree), Changes (git status: stage, unstage, throw away, commit), Graph (the
  // commits, every branch) and Notes (Scribe). A file opens beside the chat; a diff in a
  // window of its own.
  import { untrack } from "svelte";
  import { command } from "./lib/api";
  import type { AgentsItem, ProjectsItem } from "./lib/api-types";
  import Icon from "./Icon.svelte";
  import Scribe from "./Scribe.svelte";
  import DiffDialog from "./DiffDialog.svelte";
  import GitGraph from "./GitGraph.svelte";

  type Tab = "agent" | "project" | "changes" | "graph" | "notes";
  let { agent, projects, tab = $bindable("agent"), open, mention, comment }: {
    agent: AgentsItem | null; projects: ProjectsItem[]; tab?: Tab;
    open: (path: string) => void; mention?: (rel: string) => void; comment?: (file: string, line: string, note: string) => void;
  } = $props();

  const project = $derived(agent?.project_id ?? projects[0]?.id ?? "");
  const ext = (p: string) => p.split("/").pop()!.split(".").slice(1).pop()?.toLowerCase() ?? "";
  const kind = (p: string) => {
    const e = ext(p);
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "ico"].includes(e)) return "pic";
    if (["md", "markdown", "txt", "rst"].includes(e)) return "doc";
    if (["json", "toml", "yaml", "yml", "ini", "conf", "env", "lock"].includes(e) || /^\./.test(p.split("/").pop()!)) return "conf";
    if (["rs", "ts", "js", "svelte", "py", "go", "kt", "swift", "c", "h", "cpp", "java", "sh", "css", "html", "sql"].includes(e)) return "code";
    return "file";
  };
  const rel = (p: string) => (p.startsWith(project + "/") ? p.slice(project.length + 1) : p);
  let problem = $state("");
  const call = async <T,>(p: Promise<T>) => { problem = ""; try { return await p; } catch (e) { problem = (e as Error).message; } };
  let dialog = $state<{ title: string; text: string; about?: string; file?: AgentFile } | null>(null);

  // ---- Agent: the files it created or changed ----
  type AgentFile = { path: string; mark: string; repo: string | null; status: string | null; diff: string | null };
  let written = $state.raw<AgentFile[] | null>(null);
  async function loadWritten() {
    if (!agent) return void (written = []);
    const got = await call(command("changes", { agent: agent.id }));
    written = got?.files ?? [];
  }
  // again for another agent, when its count of files changes and after each turn
  $effect(() => { if (tab !== "agent") return; agent?.id; agent?.edited; (agent?.last_turn as { turns?: number } | undefined)?.turns; untrack(loadWritten); });
  const name = (f: AgentFile) => (f.repo ? f.path.slice(f.repo.length + 1) : rel(f.path));
  async function revert(f: AgentFile, hunk?: { n: number; text: string }) {
    const what = hunk === undefined ? `every change to ${name(f)}` : `this change to ${name(f)}`;
    if (!agent || !confirm(`Throw away ${what}? It goes back to how it was at the last commit. This can't be undone.`)) return;
    await call(command("revert", { agent: agent.id, path: f.path, ...(hunk ? { hunk: hunk.n, hunk_text: hunk.text } : {}) }));
    await loadWritten();
    const now = written?.find((w) => w.path === f.path);
    dialog = now?.diff ? { title: `${name(now)} — changes`, text: now.diff, file: now } : null;
  }

  // ---- Project: the tree ----
  type Entry = { name: string; path: string; dir: boolean };
  let tree = $state<Record<string, Entry[]>>({});
  let unfolded = $state<Set<string>>(new Set());
  let treeOf = "";
  async function load(folder: string) {
    const got = await call(command("files_list", { folder }));
    if (got) tree = { ...tree, [folder]: got.entries };
  }
  $effect(() => { if (tab === "project" && project && project !== treeOf) { treeOf = project; tree = {}; unfolded = new Set(); load(project); } });
  function toggle(e: Entry) {
    const next = new Set(unfolded);
    if (!next.delete(e.path)) { next.add(e.path); if (!tree[e.path]) load(e.path); }
    unfolded = next;
  }
  let rowMenu = $state<{ x: number; y: number; e: Entry } | null>(null);
  async function rowAct(what: string, e: Entry) {
    rowMenu = null;
    if (what === "app" || what === "folder") await call(command("open_path", { path: e.path, how: what }));
    else if (what === "copy") navigator.clipboard?.writeText(e.path);
    else if (what === "copyrel") navigator.clipboard?.writeText(rel(e.path));
    else if (what === "mention") mention?.(rel(e.path));
    else if (what === "trash" && confirm(`Move ${e.name} to the trash?`)) {
      if (await call(command("trash_file", { path: e.path }))) load(e.path.slice(0, e.path.lastIndexOf("/")));
    }
  }

  // ---- Changes and Graph: git ----
  type Repo = { path: string; name: string; branch: string; ahead: number; behind: number; changes: { code: string; file: string }[] };
  let repos = $state.raw<Repo[]>([]);
  let repo = $state("");
  let message = $state("");
  let bigGraph = $state(false);
  async function loadGit() {
    const got = await call(command("git_repos", { project }));
    repos = got?.repos ?? [];
    if (!repos.some((r) => r.path === repo)) repo = repos[0]?.path ?? "";
  }
  $effect(() => { if ((tab === "changes" || tab === "graph") && project) untrack(loadGit); });
  const current = $derived(repos.find((r) => r.path === repo));
  // git's two letters: the first the index (staged), the second the work tree
  const staged = (code: string) => code[0] !== " " && code[0] !== "?";
  const unstaged = (code: string) => code[1] !== " ";
  async function showDiff(f: string, isStaged: boolean) {
    const got = await call(command("git_diff", { repo, file: f, staged: isStaged }));
    if (got) dialog = { title: `${f} — changes`, text: got.diff || "(a new file: nothing to compare with)" };
  }
  async function showCommit(hash: string) {
    const got = await call(command("git_show", { repo, hash }));
    if (got) dialog = { title: `commit ${hash}`, text: got.text, about: got.text.split("\ndiff --git ")[0] };
  }
  async function git(act: string, files: string[] = []) {
    if (act === "discard" && !confirm(`Throw away the changes to ${files.join(", ")}? They go back to the last commit. This can't be undone.`)) return;
    const done = await call(command("git_do", { repo, act, ...(files.length ? { files } : {}), ...(act === "commit" ? { message } : {}) }));
    if (done && act === "commit") message = "";
    loadGit();
  }
</script>

<aside class="panel" aria-label="Files">
  <header class="panel-head">
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === "agent"} class:on={tab === "agent"} title="Files the agent wrote" onclick={() => (tab = "agent")}>Agent</button>
      <button role="tab" aria-selected={tab === "project"} class:on={tab === "project"} title="The project's files" onclick={() => (tab = "project")}>Project</button>
      <button role="tab" aria-selected={tab === "changes"} class:on={tab === "changes"} title="Git status of the repos in this project" onclick={() => (tab = "changes")}>Changes</button>
      <button role="tab" aria-selected={tab === "graph"} class:on={tab === "graph"} title="The repo's commits and where each branch is" onclick={() => (tab = "graph")}>Graph</button>
      <button role="tab" aria-selected={tab === "notes"} class:on={tab === "notes"} title="Your notes: Scribe's questions, changes waiting for your OK, what it wrote" onclick={() => (tab = "notes")}>Notes</button>
    </div>
    <span class="grow"></span>
    <button class="tiny" aria-label="Open the project folder" title="Open the project folder in the file manager" onclick={() => call(command("open_path", { path: project, how: "folder" }))}><Icon name="folder" size={14} /></button>
  </header>
  {#if (tab === "changes" || tab === "graph") && repos.length > 1}
    <div class="repo-bar"><select class="chip" aria-label="Repository" bind:value={repo} onchange={loadGit}>{#each repos as r (r.path)}<option value={r.path}>{r.name}</option>{/each}</select><button class="tiny" aria-label="Read git status again" title="Read git status again" onclick={loadGit}><Icon name="refresh" size={13} /></button></div>
  {/if}
  {#if problem}<p class="error" role="alert">{problem}</p>{/if}

  <div class="panel-body" class:fill={tab === "graph"}>
    {#if tab === "agent"}
      <p class="caption">Written by this agent</p>
      {#each written ?? [] as f (f.path)}
        <div class="grow-row">
          <button class="trow frow" title={f.path} onclick={() => open(f.path)}>
            <span class="ficon {kind(f.path)}"><Icon name={kind(f.path) === "pic" ? "image" : kind(f.path) === "doc" ? "notes" : "file"} size={12} /></span>
            <span class="names"><span>{name(f).split("/").pop()}</span>{#if name(f).includes("/")}<small>{name(f).split("/").slice(0, -1).join("/")}</small>{/if}</span>
            <span class="mark {f.mark === 'created' ? 'new' : 'edited'}">{f.mark === "created" ? "new" : "edited"}</span>
          </button>
          {#if f.diff}<span class="gacts"><button class="tiny" title="Its changes since the last commit" aria-label="Changes to {name(f)}" onclick={() => (dialog = { title: `${name(f)} — changes`, text: f.diff!, file: f })}><Icon name="diff" size={13} /></button></span>{/if}
        </div>
      {:else}
        <p class="empty small">{written === null ? "Reading…" : "Files the agent writes or edits show up here, to read or fill in without leaving loom."}</p>
      {/each}
    {:else if tab === "project"}
      <nav class="tree" aria-label="The project's files">
        {#snippet folder(path: string, depth: number)}
          {#each tree[path] ?? [] as e (e.path)}
            <button class="trow" style:padding-left="{10 + depth * 14}px" aria-expanded={e.dir ? unfolded.has(e.path) : undefined} onclick={() => (e.dir ? toggle(e) : open(e.path))} oncontextmenu={(ev) => { ev.preventDefault(); rowMenu = { x: ev.clientX, y: ev.clientY, e }; }}>
              {#if e.dir}<Icon name={unfolded.has(e.path) ? "chev-d" : "chev-r"} size={12} />{:else}<span class="ficon {kind(e.name)}"><Icon name={kind(e.name) === "pic" ? "image" : kind(e.name) === "doc" ? "notes" : "file"} size={12} /></span>{/if}<span>{e.name}</span>
            </button>
            {#if e.dir && unfolded.has(e.path)}{@render folder(e.path, depth + 1)}{/if}
          {/each}
        {/snippet}
        {@render folder(project, 0)}
      </nav>
    {:else if tab === "changes"}
      <div class="git">
        {#if current}
          <p class="quiet small">⎇ {current.branch}{#if current.ahead}{" · "}↑{current.ahead} to push{/if}{#if current.behind}{" · "}↓{current.behind} to pull{/if}{current.changes.length ? "" : " · clean"}</p>
          {#each current.changes as c (c.file)}
            <div class="grow-row">
              <button class="trow" onclick={() => showDiff(c.file, staged(c.code) && !unstaged(c.code))}><span class="badge" class:st={staged(c.code)}>{c.code.trim()}</span><span>{c.file}</span></button>
              <span class="gacts">
                {#if unstaged(c.code)}<button class="tiny" title="Stage: put it in the next commit" aria-label="Stage {c.file}" onclick={() => git("stage", [c.file])}><Icon name="plus" size={13} /></button>{/if}
                {#if staged(c.code)}<button class="tiny" title="Unstage: leave it out of the next commit" aria-label="Unstage {c.file}" onclick={() => git("unstage", [c.file])}><Icon name="back" size={13} /></button>{/if}
                {#if c.code !== "??" && !"AR".includes(c.code[0])}<button class="tiny" title="Throw its changes away" aria-label="Throw away {c.file}" onclick={() => git("discard", [c.file])}><Icon name="x" size={13} /></button>{/if}
              </span>
            </div>
          {/each}
          <form class="commit" onsubmit={(e) => { e.preventDefault(); git("commit"); }}>
            <input aria-label="Commit message" placeholder="Commit message (commits what is staged)" bind:value={message} />
            <button class="go" disabled={!message.trim() || !current.changes.some((c) => staged(c.code))}>Commit</button>
          </form>
        {:else}
          <p class="empty small">No git repository in this project.</p>
        {/if}
      </div>
    {:else if tab === "graph"}
      {#if repo}
        <div class="graph-bar"><span class="quiet small">{current ? `⎇ ${current.branch}` : ""}</span><span class="grow"></span><button class="btn small" title="The whole graph, big: drag it around" onclick={() => (bigGraph = true)}>Expand</button></div>
        {#key repo}<GitGraph {repo} open={showCommit} />{/key}
      {:else}
        <p class="empty small">No git repository in this project.</p>
      {/if}
    {:else}
      <Scribe />
    {/if}
  </div>

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
</aside>

{#if bigGraph && repo}
  <div class="scrim top" role="presentation" onclick={() => (bigGraph = false)}></div>
  <!-- svelte-ignore a11y_autofocus -->
  <div class="diff-window graph-window" role="dialog" aria-modal="true" aria-label="Commit graph" tabindex="-1" autofocus onkeydown={(e) => e.key === "Escape" && !dialog && (bigGraph = false)}>
    <div class="ebar"><code>{current?.name ?? ""} · every branch</code><span class="quiet small">drag to move around · click a commit to open it</span><span class="grow"></span><button class="btn small" onclick={() => (bigGraph = false)}>Close</button></div>
    <GitGraph {repo} wide open={showCommit} />
  </div>
{/if}
{#if dialog}
  {@const f = dialog.file}
  <DiffDialog title={dialog.title} text={dialog.text} about={dialog.about}
    undo={f && f.status !== "??" ? (n, hunk) => revert(f, { n, text: hunk }) : undefined}
    undoAll={f && f.status !== "??" ? () => revert(f) : undefined}
    comment={f && comment ? (line) => { const said = prompt(`Your note on this line of ${name(f)} (it goes with your next message):`, ""); if (said?.trim()) comment!(name(f), line, said.trim()); } : undefined}
    close={() => (dialog = null)} />
{/if}
