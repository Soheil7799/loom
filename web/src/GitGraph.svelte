<script lang="ts">
  // A repository's commits as a graph: every branch, a coloured lane each, lines curving where
  // they fork and merge (lib/lanes), the branches and tags on their commits. A click opens a
  // commit; the next commits load as you scroll down. `wide`: the window's whole width, with
  // who and when, dragged around to look.
  import { untrack } from "svelte";
  import { command } from "./lib/api";
  import { layout, path } from "./lib/lanes";

  type Commit = { hash: string | null; refs: string; subject: string; author: string; when: string; parents?: string[] };
  let { repo, wide = false, open }: { repo: string; wide?: boolean; open: (hash: string) => void } = $props();
  const PAGE = 200;
  const W = $derived(wide ? 16 : 12), H = $derived(wide ? 30 : 26);
  // the lanes' colours: the theme's, then two more
  const COLOURS = ["var(--accent)", "var(--ok)", "var(--wait)", "var(--err)", "#b48ead", "#5fb3b3"];
  const colour = (lane: number) => COLOURS[lane % COLOURS.length];

  let commits = $state.raw<Commit[]>([]);
  let more = $state(true);
  let loading = $state(false);
  let problem = $state("");
  let hover = $state<number | null>(null);
  let box: HTMLElement | undefined = $state();

  // a new repo's load overtakes one still on its way: only the latest's answer is used
  let asked = 0;
  async function load(reset = false) {
    if ((loading && !reset) || (!more && !reset)) return;
    loading = true;
    const mine = ++asked;
    try {
      const got = (await command("git_log", { repo, limit: PAGE, skip: reset ? 0 : commits.length, all: true })).commits as Commit[];
      if (mine !== asked) return;
      // a commit made since the last page shifted the list: what is shown already isn't again
      const seen = new Set(reset ? [] : commits.map((c) => c.hash));
      commits = reset ? got : [...commits, ...got.filter((c) => !c.hash || !seen.has(c.hash))];
      more = got.length === PAGE;
      problem = "";
    } catch (e) { if (mine === asked) problem = (e as Error).message; }
    if (mine === asked) loading = false;
  }
  $effect(() => { repo; untrack(() => { more = true; load(true); }); });
  const g = $derived(layout(commits));
  const rows = $derived(commits.length);
  // the lines touching the commit under the pointer stand out
  const near = (e: { row: number; to: number | null }) => hover !== null && (e.row === hover || e.to === hover);
  function scrolled() {
    if (box && box.scrollTop + box.clientHeight > box.scrollHeight - H * 20) load();
  }
  // wide: dragged around (not from a row: that opens it)
  let drag: { x: number; y: number; left: number; top: number } | null = null;
  function down(e: PointerEvent) {
    if (!wide || !box || (e.target as HTMLElement).closest("button")) return;
    drag = { x: e.clientX, y: e.clientY, left: box.scrollLeft, top: box.scrollTop };
    box.setPointerCapture(e.pointerId);
  }
  function move(e: PointerEvent) {
    if (!drag || !box) return;
    box.scrollLeft = drag.left - (e.clientX - drag.x);
    box.scrollTop = drag.top - (e.clientY - drag.y);
  }
  const badges = (refs: string) => (refs ?? "").split(", ").filter(Boolean).map((r) => ({ name: r.replace("HEAD -> ", "").replace("tag: ", ""), head: r.startsWith("HEAD"), tag: r.startsWith("tag:"), remote: r.includes("/") && !r.startsWith("tag:") }));
</script>

<div class="ggraph" class:wide bind:this={box} onscroll={scrolled} onpointerdown={down} onpointermove={move} onpointerup={() => (drag = null)} role="presentation">
  {#if problem}<p class="error">{problem}</p>{/if}
  <div class="gcanvas" style:height="{rows * H}px" style:--lanes="{g.width * W}px" style:--row="{H}px">
    <svg class="glines" width={g.width * W} height={rows * H} aria-hidden="true">
      {#each g.edges as e, i (i)}
        <path d={path(e, W, H, rows)} stroke={colour(e.lane)} class:near={near(e)} />
      {/each}
      {#each g.lanes as lane, row (row)}
        {@const merge = (commits[row].parents?.length ?? 0) > 1}
        <circle cx={lane * W + W / 2} cy={row * H + H / 2} r={merge ? 3.5 : 4} stroke={colour(lane)} fill={merge ? "var(--pane)" : colour(lane)} stroke-width={merge ? 2 : 0} />
      {/each}
    </svg>
    {#each commits as c, row (c.hash ?? row)}
      <button class="gitrow" class:on={hover === row} style:top="{row * H}px" title="{c.subject} · {c.author}, {c.when}" onclick={() => c.hash && open(c.hash)} onpointerenter={() => (hover = row)} onpointerleave={() => (hover = null)}>
        {#each badges(c.refs) as b (b.name)}<span class="gref" class:head={b.head} class:tag={b.tag} class:remote={b.remote} style:--c={colour(g.lanes[row])}>{b.name}</span>{/each}
        <span class="gsubject">{c.subject}</span>
        {#if wide}<span class="gwho">{c.author}</span><span class="gwhen">{c.when}</span>{/if}
        <code class="ghash">{c.hash}</code>
      </button>
    {/each}
  </div>
  {#if loading}<p class="quiet small gmore">Reading…</p>{:else if !commits.length && !problem}<p class="empty small">No commits yet.</p>{/if}
</div>
