<script lang="ts">
  // An agent's changes beside its conversation: each file it created or changed, its diff
  // against the last commit, and going back on one hunk or the whole file (git; it can't be
  // undone, so it asks first).
  import { command } from "./lib/api";
  import { unified } from "./lib/diff";
  import Icon from "./Icon.svelte";

  let { agent, edited, close }: { agent: string; edited: number; close: () => void } = $props();
  type File = { path: string; mark: string; repo: string | null; status: string | null; diff: string | null };
  let files = $state.raw<File[] | null>(null);
  let problem = $state("");

  async function load() {
    try { files = (await command("changes", { agent })).files; problem = ""; } catch (e) { problem = (e as Error).message; }
  }
  // again whenever its count of files changes (and when it opens)
  $effect(() => { edited; load(); });

  const name = (f: File) => (f.repo ? f.path.slice(f.repo.length + 1) : f.path);
  async function revert(f: File, hunk?: number) {
    const what = hunk === undefined ? `every change to ${name(f)}` : `this change to ${name(f)}`;
    if (!confirm(`Throw away ${what}? It goes back to how it was at the last commit. This can't be undone.`)) return;
    try { await command("revert", { agent, path: f.path, ...(hunk === undefined ? {} : { hunk }) }); } catch (e) { problem = (e as Error).message; }
    load();
  }
</script>

<aside class="changes" aria-label="Its changes">
  <div class="changes-head"><b>Changes</b><span class="grow"></span><button class="tiny" aria-label="Refresh" title="Refresh" onclick={load}><Icon name="refresh" size={13} /></button><button class="tiny" aria-label="Close" onclick={close}><Icon name="x" size={14} /></button></div>
  {#if problem}<p class="error">{problem}</p>{/if}
  {#if files === null}
    <p class="quiet">Reading…</p>
  {:else}
    {#each files as f (f.path)}
      <section class="cfile">
        <div class="cfile-head">
          <span class="badge">{f.status?.trim() || (f.mark === "created" ? "new" : "=")}</span>
          <span class="cname" title={f.path}>{name(f)}</span>
          {#if f.diff && f.status !== "??"}<button class="btn small deny" onclick={() => revert(f)}>Throw away</button>{/if}
        </div>
        {#if f.diff}
          {#each unified(f.diff) as h, i (i)}
            <div class="diff">
              <div class="f"><span>{h.head}</span>{#if f.status !== "??"}<button class="btn small" title="Undo only this change" onclick={() => revert(f, i)}>Undo</button>{/if}</div>
              <pre>{#each h.lines as l, k (k)}<span class={l.t === "+" ? "add" : l.t === "-" ? "del" : ""}>{l.t}{l.text}
</span>{/each}</pre>
            </div>
          {/each}
        {:else if !f.repo}
          <p class="quiet small">Not in a git repository: no diff to show.</p>
        {:else}
          <p class="quiet small">{f.status === "??" ? "New, not yet known to git." : "As committed: nothing to undo."}</p>
        {/if}
      </section>
    {:else}
      <p class="quiet">It changed no files.</p>
    {/each}
  {/if}
</aside>
