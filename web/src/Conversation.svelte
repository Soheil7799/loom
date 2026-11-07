<script lang="ts">
  // An agent's conversation as Turns: your message opens each turn, pinned while you read it;
  // finished turns fold to your message and how they ended; the tube down the left marks turn
  // starts, questions and failures, and moves while the agent works. Alt+Up/Down: the previous
  // or next question or failure.
  import { untrack } from "svelte";
  import { follow, command, type Item } from "./lib/api";
  import type { AgentsItem, JobsItem, UsageItem } from "./lib/api-types";
  import { turns, type Entry } from "./lib/turns";
  import { stateOf } from "./lib/state";
  import Entry_ from "./Entry.svelte";
  import Tube from "./Tube.svelte";
  import Picture from "./Picture.svelte";
  import Plan from "./Plan.svelte";
  import Terminal from "./Terminal.svelte";
  import { prepare } from "./lib/pictures";
  import Icon from "./Icon.svelte";

  let { agent, jobs = [], showJob, showChanges, expectCopy, active = true, insert = $bindable(null), note = $bindable(null) }: { agent: AgentsItem; jobs?: JobsItem[]; showJob?: (id: string) => void; showChanges?: () => void; expectCopy?: (on: boolean) => void; active?: boolean; insert?: string | null; note?: { file: string; line: string; note: string } | null } = $props();
  // text put into its box from elsewhere (a file mentioned from Files), taken once
  $effect(() => { if (insert && active) { const add = insert; insert = null; text = `${untrack(() => text)}${untrack(() => text) && !untrack(() => text).endsWith(" ") ? " " : ""}${add}`; } });
  let entries = $state.raw<Entry[]>([]);
  let usage = $state.raw<UsageItem | null>(null);
  let gone = $state(false);
  // what waits in its input (a rewind's message), taken once when it opens
  let text = $state("");
  let problem = $state("");
  let expanded = $state(new Set<string>());
  let box: HTMLElement | undefined = $state();
  let column: HTMLElement | undefined = $state();
  let naming = $state<string | null>(null);
  // a job's terminal kept beside the chat (its chip, again to put it away)
  let pinnedJob = $state<string | null>(null);
  // Notes on lines of its changes, sent with the next message
  let comments = $state.raw<{ file: string; line: string; note: string }[]>([]);
  // a note on a line of its changes, from the right panel
  $effect(() => { if (note && active) { const add = note; note = null; comments = [...untrack(() => comments), add]; } });

  $effect(() => follow(`conversation/${agent.id}`, (f) => {
    entries = [...f.items] as Entry[];
    gone = f.gone;
    if (f.error) problem = f.error;
  }));
  let crew = $state.raw<Item[]>([]);
  $effect(() => follow(`crew/${agent.id}`, (f) => (crew = f.items)));
  // What waits in its input (`inputs`, people's alone): its draft (a rewind's message, one
  // taken back from the queue) comes into the composer when it opens or changes, if empty.
  let queued = $state.raw<string[]>([]);
  let commands = $state.raw<{ name: string; about: string }[]>([]);
  let suggestion = $state("");
  let draft = "";
  let lastDraft: string | null = null;
  $effect(() => follow("inputs", (f) => {
    const mine = f.items.find((i: Item) => i.id === agent.id);
    queued = mine?.queued ?? [];
    commands = mine?.commands ?? [];
    suggestion = mine?.suggestion ?? "";
    draft = mine?.draft ?? "";
    if (draft !== lastDraft) { lastDraft = draft; if (draft && !text.trim()) text = draft; }
  }));
  type Opt = { id: string; name?: string; category?: string; currentValue?: string; options?: unknown[] };
  // an option's values, groups flattened
  const flat = (os: unknown[] = []): { value: string; name: string }[] =>
    os.flatMap((o) => { const x = o as { value?: string; name?: string; options?: unknown[] }; return x.options ? flat(x.options) : x.value ? [{ value: x.value, name: x.name ?? x.value }] : []; });
  const options = $derived(((agent.options ?? []) as Opt[]).filter((o) => o.id !== "mode" && flat(o.options).length));
  let compactAt = $state<number | null>(null);
  $effect(() => follow("settings", (f) => (compactAt = f.items[0]?.compact_alert ? (f.items[0]?.compact_at ?? 80) : null)));
  // its context and cost, and the account's limits (5-hour, weekly) as Claude Code last said
  let limits = $state.raw<{ kind: string; utilization: number; resets_at?: string }[]>([]);
  $effect(() => follow("usage", (f) => {
    usage = (f.items.find((u: Item) => u.id === agent.id) as UsageItem) ?? null;
    limits = f.items.find((u: Item) => u.id === "account")?.windows ?? [];
  }));
  const limitName = (k: string) => ({ five_hour: "5h", seven_day: "week", seven_day_opus: "week, Opus", seven_day_sonnet: "week, Sonnet" } as Record<string, string>)[k] ?? k.replace(/_/g, " ");
  const pct = (u: number) => Math.round(u <= 1 ? u * 100 : u);
  // its MCP servers, as its process last said
  let mcp = $state.raw<{ name: string; status: string; error?: string }[]>([]);
  $effect(() => follow("mcp", (f) => (mcp = f.items.find((m: Item) => m.id === agent.id)?.servers ?? [])));
  const mcpBad = $derived(mcp.filter((m) => !/connected|ok|ready/i.test(m.status)));

  const all = $derived(turns(entries));
  // a Notes chat isn't among the agents: it works while an entry is still coming
  // The files the last turn edited (its tools' diffs): the Changes panel's "Last turn".
  const lastTurnFiles = $derived([...new Set((all.at(-1)?.rows ?? []).flatMap((r) => (r.kind === "run" ? r.tools : [r.entry])).flatMap((e) => (e.kind === "tool" ? ((e.content ?? []) as { type?: string; path?: string }[]).filter((c) => c.type === "diff" && c.path).map((c) => c.path!) : [])))]);
  const busy = $derived(agent.busy || (agent.status === "notes" && entries.some((e) => e.status === "pending" || e.status === "streaming")));
  const st = $derived(stateOf({ ...agent, busy }));
  const context = $derived(usage?.used && usage?.size ? Math.round((usage.used / usage.size) * 100) : null);
  const cost = $derived(usage?.cost as { amount?: number; currency?: string } | undefined);

  // "Since you last looked": the newest entry seen here before, kept per agent in this browser.
  const seenKey = $derived(`loom-seen:${agent.id}`);
  const seen = $derived(Number(localStorage.getItem(seenKey)?.slice(1) ?? "0"));
  const num = (id: string) => Number(id.slice(1)) || 0;
  const sinceAt = $derived(seen ? all.findIndex((t) => [t.user, ...t.rows.flatMap((r) => (r.kind === "run" ? r.tools : [r.entry]))].some((e) => e && num(e.id) > seen)) : -1);
  $effect(() => {
    const last = entries.at(-1);
    if (last && document.visibilityState === "visible") localStorage.setItem(seenKey, last.id);
  });

  let composerEl: HTMLTextAreaElement | undefined = $state();
  // Ctrl+F: find in the conversation, folded turns too (those with a match unfold); the
  // matches marked with the CSS Highlight API where the browser has it.
  let finding = $state(false);
  let findQuery = $state("");
  let findEl: HTMLInputElement | undefined = $state();
  let hits = $state.raw<Range[]>([]);
  let hitAt = $state(0);
  const entryText = (e: Entry) => [e.text, e.title, e.output].filter((x) => typeof x === "string").join(" ");
  $effect(() => {
    const q = findQuery.trim().toLowerCase();
    if (!finding || !q) { hits = []; return; }
    // unfold the turns that have it
    const turnsWith = all.filter((t) => [t.user, ...t.rows.flatMap((r) => (r.kind === "run" ? r.tools : [r.entry]))].some((e) => e && entryText(e).toLowerCase().includes(q))).map((t) => t.id);
    untrack(() => { const next = new Set(expanded); turnsWith.forEach((id) => next.add(id)); if (next.size !== expanded.size) expanded = next; });
    // then find them in what is drawn
    queueMicrotask(() => requestAnimationFrame(() => {
      if (!box) return;
      const found: Range[] = [];
      const walker = document.createTreeWalker(box, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const t = n.textContent?.toLowerCase() ?? "";
        for (let i = t.indexOf(q); i >= 0; i = t.indexOf(q, i + q.length)) { const r = new Range(); r.setStart(n, i); r.setEnd(n, i + q.length); found.push(r); }
      }
      hits = found;
      hitAt = Math.max(0, found.length - 1);
      showHit();
    }));
  });
  function showHit() {
    const css = (globalThis as { CSS?: { highlights?: Map<string, unknown> } }).CSS;
    const H = (globalThis as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
    if (css?.highlights && H) {
      css.highlights.set("find", new H(...hits));
      if (hits[hitAt]) css.highlights.set("find-now", new H(hits[hitAt]));
      else css.highlights.delete("find-now");
    }
    const r = hits[hitAt]?.getBoundingClientRect();
    if (r && box) box.scrollBy({ top: r.top - box.getBoundingClientRect().top - box.clientHeight / 3, behavior: "smooth" });
  }
  function stepFind(by: number) { if (hits.length) { hitAt = (hitAt + by + hits.length) % hits.length; showHit(); } }
  function closeFind() {
    finding = false;
    findQuery = "";
    (globalThis as { CSS?: { highlights?: Map<string, unknown> } }).CSS?.highlights?.clear();
  }

  // Looked at: what was new in it is seen (here, as in the window).
  $effect(() => {
    if (active && agent.unseen && agent.status !== "notes" && document.visibilityState === "visible") command("mark_seen", { agent: agent.id }).catch(() => {});
  });

  // Opened at its end, then kept there as it grows, unless you scrolled up.
  let opened = false;
  $effect(() => {
    if (!entries.length || !box) return;
    if (!opened || box.scrollHeight - box.scrollTop - box.clientHeight < 160) queueMicrotask(() => box && (box.scrollTop = box.scrollHeight));
    opened = true;
  });

  // the result line is one line of plain words: markdown's marks taken out
  const plain = (md: string) => md.replace(/```[\s\S]*?(```|$)/g, " ").replace(/[`*_#>]+/g, "").replace(/\s+/g, " ").trim();

  const folded = (i: number) => {
    const t = all[i];
    return i < all.length - 1 && !t.live && !t.waits && !t.failed && !expanded.has(t.id);
  };
  function toggle(id: string) {
    const next = new Set(expanded);
    if (!next.delete(id)) next.add(id);
    expanded = next;
  }

  async function run(p: Promise<unknown>) {
    problem = "";
    try { await p; return true; } catch (e) { problem = (e as Error).message; return false; }
  }
  // pictures to go with the next message: base64, and a preview
  let attached = $state.raw<{ data: string; preview: string }[]>([]);
  let reading = $state(false);
  async function attach(files: Iterable<File>) {
    const all = [...files];
    const pictures = all.filter((f) => f.type.startsWith("image/"));
    // anything else: saved in its folder (the browser gives no path) and mentioned
    for (const f of all.filter((f) => !f.type.startsWith("image/"))) {
      try {
        const bytes = new Uint8Array(await f.arrayBuffer());
        let raw = "";
        for (let i = 0; i < bytes.length; i += 0x8000) raw += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
        const got = await command("save", { agent: agent.id, name: f.name, data: btoa(raw) });
        text = `${text}${text && !text.endsWith(" ") ? " " : ""}@${got.path} `;
      } catch (e) { problem = (e as Error).message; }
    }
    if (!pictures.length) return;
    reading = true;
    try {
      const read = await Promise.all(pictures.map(async (f) => ({ data: await prepare(f), preview: URL.createObjectURL(f) })));
      attached = [...attached, ...read];
    } catch { problem = "That picture could not be read."; }
    reading = false;
  }
  function detach(i: number) {
    URL.revokeObjectURL(attached[i].preview);
    attached = attached.filter((_, k) => k !== i);
  }
  const ready = $derived(!!text.trim() || attached.length > 0 || comments.length > 0);
  let picker: HTMLInputElement | undefined = $state();

  async function send() {
    if (!ready || reading) return;
    const images = attached.map((a) => a.data);
    const notes = comments.length ? `About your changes:\n${comments.map((c) => `- ${c.file}, the line \`${c.line.trim()}\`: ${c.note}`).join("\n")}\n\n` : "";
    if (await run(command("send", { agent: agent.id, text: notes + text, ...(images.length ? { images } : {}) }))) {
      text = "";
      comments = [];
      attached.forEach((a) => URL.revokeObjectURL(a.preview));
      attached = [];
      // what loom kept in its input went with it
      if (draft) { lastDraft = ""; run(command("set_draft", { agent: agent.id, text: "" })); }
    }
  }
  // "/" at the start: its commands, to pick (↑↓, Tab or Enter); an empty box: Tab or →
  // takes the suggested next message.
  let pickAt = $state(0);
  const slash = $derived(/^\/\S*$/.test(text) ? commands.filter((c) => ("/" + c.name).startsWith(text)).slice(0, 8) : []);
  $effect(() => { slash; pickAt = 0; });
  function take(c: { name: string }) { text = `/${c.name} `; }
  // "@" and part of a name: the project's files, to mention (↑↓, Tab or Enter)
  const mentionQuery = $derived(/(?:^|\s)@([^\s@]*)$/.exec(text)?.[1] ?? null);
  let mentions = $state.raw<string[]>([]);
  let mentionTimer = 0;
  $effect(() => {
    const q = mentionQuery;
    clearTimeout(mentionTimer);
    if (q === null) return void (mentions = []);
    mentionTimer = window.setTimeout(async () => {
      const got = await command("files_find", { project: agent.project_id, query: q || ".", limit: 8 }).catch(() => null);
      if (mentionQuery === q) mentions = got?.files ?? [];
    }, 120);
  });
  $effect(() => { mentions; pickAt = 0; });
  function mention(path: string) { text = text.replace(/@([^\s@]*)$/, `@${path} `); mentions = []; }

  function keys(e: KeyboardEvent) {
    if (mentions.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); pickAt = (pickAt + (e.key === "ArrowDown" ? 1 : mentions.length - 1)) % mentions.length; return; }
    if (mentions.length && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey))) { e.preventDefault(); mention(mentions[pickAt]); return; }
    if (e.key === "Escape" && mentions.length) { e.preventDefault(); mentions = []; return; }
    // Ctrl+Enter: what is typed joins the queue, and the queue goes in now
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); const was = busy; send().then(() => { if (was) run(command("send_now", { agent: agent.id })); }); return; }
    if (slash.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); pickAt = (pickAt + (e.key === "ArrowDown" ? 1 : slash.length - 1)) % slash.length; return; }
    if (slash.length && (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && text !== `/${slash[pickAt].name}`))) { e.preventDefault(); take(slash[pickAt]); return; }
    if (!text && suggestion && (e.key === "Tab" || e.key === "ArrowRight")) { e.preventDefault(); text = suggestion; return; }
    if (e.key === "Escape" && busy) { e.preventDefault(); run(command("stop", { agent: agent.id })); return; }
    if (e.key === "Tab" && e.shiftKey && agent.modes?.length) {
      e.preventDefault();
      const modes = agent.modes as string[];
      run(command("set_mode", { agent: agent.id, mode: modes[(modes.indexOf(agent.mode ?? "") + 1) % modes.length] }));
      return;
    }
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
  }

  // Speech to text (Settings → Speech to text): recorded here, written by loom's service.
  let sttOn = $state(false);
  $effect(() => follow("settings", (f) => (sttOn = !!f.items[0]?.stt_enabled)));
  let recorder: MediaRecorder | null = $state(null);
  let hearing = $state(false);
  async function listen() {
    if (recorder) return recorder.stop();
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const parts: Blob[] = [];
      const r = new MediaRecorder(stream);
      r.ondataavailable = (e) => parts.push(e.data);
      r.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());
        recorder = null;
        hearing = true;
        try {
          const audio = new Blob(parts, { type: r.mimeType });
          const bytes = new Uint8Array(await audio.arrayBuffer());
          let raw = "";
          for (let i = 0; i < bytes.length; i += 0x8000) raw += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
          const got = await command("transcribe", { audio: btoa(raw), mime: r.mimeType.split(";")[0], name: "speech.webm" });
          if (got.text) text = text.trim() ? `${text.trimEnd()} ${got.text}` : got.text;
        } catch (err) { problem = (err as Error).message; }
        hearing = false;
      };
      r.start();
      recorder = r;
    } catch (err) { problem = `The microphone: ${(err as Error).message}`; }
  }

  // The agent's menu: a copy, a fresh start, another folder, Markdown, its place.
  let menuOpen = $state(false);
  async function exportIt() {
    const got = await command("export", { agent: agent.id }).catch((e) => { problem = e.message; });
    if (!got) return;
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([got.markdown], { type: "text/markdown" }));
    a.download = `${(agent.title || "conversation").replace(/[^\p{L}\p{N} -]+/gu, "_").trim()}.md`;
    a.click();
    // after the download has begun (Firefox would drop it)
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  }
  async function anotherFolder() {
    const folder = prompt("Another folder this agent may work in (absolute path). It restarts.", agent.project_id);
    if (folder?.trim()) run(command("add_folder", { agent: agent.id, folder: folder.trim() }));
  }
  async function rewind(entry: string, files: boolean) {
    expectCopy?.(true);
    // no copy is coming: the next new agent is not one
    if (!(await run(command("rewind", { agent: agent.id, entry, files })))) expectCopy?.(false);
  }
  async function rename(e?: SubmitEvent) {
    e?.preventDefault();
    const title = naming?.trim();
    naming = null;
    if (title && title !== agent.title) await run(command("rename", { agent: agent.id, title }));
  }
  function jump(e: KeyboardEvent) {
    if (e.key === "Escape" && menuOpen) { menuOpen = false; return; }
    // two side by side: the keys are the active one's
    if (!active) return;
    if (e.key === "F2" && naming === null && agent.status !== "notes") { e.preventDefault(); naming = agent.title; return; }
    if ((e.ctrlKey || e.metaKey) && e.key === "f" && !e.altKey && !e.shiftKey) { e.preventDefault(); finding = true; queueMicrotask(() => findEl?.focus()); return; }
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown") || !box) return;
    const top = box.getBoundingClientRect().top, at = box.scrollTop + 120;
    const marks = [...box.querySelectorAll<HTMLElement>('[data-bead="wait"], [data-bead="fail"]')].map((m) => m.getBoundingClientRect().top - top + box!.scrollTop);
    const to = e.key === "ArrowDown" ? marks.find((y) => y > at + 4) : marks.reverse().find((y) => y < at - 4);
    if (to !== undefined) { box.scrollTo({ top: to - 120, behavior: "smooth" }); e.preventDefault(); }
  }
</script>

<svelte:window onkeydown={jump} onclick={() => (menuOpen = false)} />

<div class="convo">
<section class="centre" aria-label={agent.title || "Conversation"}>
  <header class="header">
    <i class="st {st.shape}" role="img" aria-label={st.word}></i>
    <span class="headline">
      {#if naming !== null}
        <form class="rename" onsubmit={rename}>
          <!-- svelte-ignore a11y_autofocus -->
          <input aria-label="The agent's name" bind:value={naming} autofocus onblur={() => rename()} onkeydown={(e) => e.key === "Escape" && (naming = null)} />
        </form>
      {:else}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="title" ondblclick={() => (naming = agent.title)}>{agent.title || "New agent"}</span>
        {#if agent.status !== "notes"}<button class="tiny" aria-label="Rename (F2)" title="Rename (F2)" onclick={() => (naming = agent.title)}><Icon name="pencil" size={13} /></button>{/if}
      {/if}
      <span class="place">· {agent.project}</span>
      {#if st.shape === "wait"}<span class="needs">waits for you</span>{/if}
    </span>
    <span class="grow"></span>
    {#if context !== null}<span class="ctx"><span class="bar"><i style:width="{Math.min(context, 100)}%"></i></span>{context}%</span>{/if}
    {#if context !== null && compactAt !== null && context >= compactAt && agent.status !== "notes"}
      <button class="chip warn" title="The context is {context}% full: summarise the conversation so far to make room (/compact)" disabled={busy} onclick={() => run(command("send", { agent: agent.id, text: "/compact" }))}>Compact</button>
    {/if}
    {#if limits.length && agent.status !== "notes"}<span class="ctx" title={limits.map((l) => `${limitName(l.kind)}: ${pct(l.utilization)}% used${l.resets_at ? `, resets ${new Date(l.resets_at).toLocaleString()}` : ""}`).join("\n")}>{limits.slice(0, 2).map((l) => `${limitName(l.kind)} ${pct(l.utilization)}%`).join(" · ")}</span>{/if}
    {#if mcpBad.length}<span class="ctx warnmcp" title={mcpBad.map((m) => `${m.name}: ${m.status}${m.error ? ` (${m.error})` : ""}`).join("\n")}>⚠ {mcpBad.length === 1 ? mcpBad[0].name : `${mcpBad.length} tool servers`}</span>{/if}
    {#if cost?.amount !== undefined}<span class="ctx">{cost.currency === "USD" || !cost.currency ? "$" : cost.currency + " "}{cost.amount.toFixed(2)}</span>{/if}
    {#if agent.edited}<button class="chip" title="The files it changed" onclick={() => showChanges?.()}><Icon name="diff" size={13} /> {agent.edited}</button>{/if}
    {#if agent.status !== "notes"}
      {#if agent.awake}<span class="awake" title="Kept awake: never suspended while idle"><Icon name="coffee" size={13} /></span>{/if}
      <div class="inbox-wrap amenu">
        <button class="tiny" aria-label="More for this agent" aria-expanded={menuOpen} onclick={(e) => { e.stopPropagation(); menuOpen = !menuOpen; }}><Icon name="more" size={15} /></button>
        {#if menuOpen}
          <div class="menu">
            <button class="it" onclick={() => { menuOpen = false; expectCopy?.(true); run(command("fork", { agent: agent.id })); }}><Icon name="copy" size={13} /><span>Copy this conversation</span></button>
            <button class="it" onclick={() => { menuOpen = false; if (confirm("Start a fresh conversation here? This one stays among the past ones.")) run(command("clear", { agent: agent.id })); }}><Icon name="eraser" size={13} /><span>Clear: a fresh conversation</span></button>
            <button class="it" onclick={() => { menuOpen = false; run(command("restart", { agent: agent.id })); }}><Icon name="refresh" size={13} /><span>Restart it</span></button>
            <button class="it" onclick={() => { menuOpen = false; anotherFolder(); }}><Icon name="folder" size={13} /><span>Work in another folder too…</span></button>
            <button class="it" onclick={() => { menuOpen = false; exportIt(); }}><Icon name="download" size={13} /><span>Save as Markdown</span></button>
            <button class="it" title={agent.awake ? "Kept awake: never suspended while idle" : "Never suspend it while idle"} onclick={() => { menuOpen = false; run(command("set_awake", { agent: agent.id, awake: !agent.awake })); }}><Icon name="coffee" size={13} /><span>{agent.awake ? "Allow suspending it when idle" : "Keep it awake"}</span></button>
            <button class="it" disabled={busy || agent.status === "suspended"} title={busy ? "Stop it first" : "Frees its memory; a message resumes it"} onclick={() => { menuOpen = false; run(command("suspend", { agent: agent.id })); }}><Icon name="moon" size={13} /><span>Suspend it now</span></button>
            <button class="it" onclick={() => { menuOpen = false; run(command("move_agent", { agent: agent.id, by: -1 })); }}><Icon name="chev-u" size={13} /><span>Move it up</span></button>
            <button class="it" onclick={() => { menuOpen = false; run(command("move_agent", { agent: agent.id, by: 1 })); }}><Icon name="chev-d" size={13} /><span>Move it down</span></button>
            <button class="it" onclick={() => { menuOpen = false; run(command("move_project", { project: agent.project_id, by: -1 })); }}><Icon name="chev-u" size={13} /><span>Move its project up</span></button>
            <button class="it" onclick={() => { menuOpen = false; run(command("move_project", { project: agent.project_id, by: 1 })); }}><Icon name="chev-d" size={13} /><span>Move its project down</span></button>
            {#if mcp.length}
              <p class="foot">Tool servers: {#each mcp as m, i (m.name)}{i ? ", " : ""}<span class:bad={mcpBad.includes(m)} title={m.error ?? m.status}>{m.name} ({m.status})</span>{/each}</p>
            {/if}
          </div>
        {/if}
      </div>

    {/if}
  </header>
  {#if crew.length}
    <nav class="jobstrip crew" aria-label="Its subagents">
      {#each crew as c (c.id)}
        {@const shape = c.ended ? "done" : c.state === "requires_action" ? "wait" : c.state === "idle" ? "idle" : "work"}
        <span class="jchip" title={c.step}><i class="st {shape}" role="img" aria-label={c.state ?? (c.ended ? "ended" : "running")}></i>{c.name}{#if c.step}<small>{c.step}</small>{/if}</span>
      {/each}
    </nav>
  {/if}
  {#if jobs.length && showJob}
    <nav class="jobstrip" aria-label="This project's jobs">
      {#each jobs.slice(-6) as j (j.id)}
        {@const shape = j.status !== "exited" ? "work" : j.exit_code === 0 ? "done" : "fail"}
        <button class="jchip" class:on={pinnedJob === j.id} title="{j.command} (click: its terminal here; again: away)" onclick={() => (pinnedJob = pinnedJob === j.id ? null : j.id)}><i class="st {shape}" role="img" aria-label={j.status}></i><Icon name="terminal" size={12} />{j.name}</button>
      {/each}
    </nav>
  {/if}

  {#if finding}
    <div class="findbar">
      <input bind:this={findEl} bind:value={findQuery} placeholder="Find in the conversation" aria-label="Find in the conversation" onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); stepFind(e.shiftKey ? -1 : 1); } if (e.key === "Escape") closeFind(); }} />
      <span class="quiet small">{findQuery.trim() ? (hits.length ? `${hitAt + 1} of ${hits.length}` : "none") : ""}</span>
      <button class="tiny" aria-label="Previous" onclick={() => stepFind(-1)}><Icon name="chev-u" size={13} /></button>
      <button class="tiny" aria-label="Next" onclick={() => stepFind(1)}><Icon name="chev-d" size={13} /></button>
      <button class="tiny" aria-label="Close the find bar" onclick={closeFind}><Icon name="x" size={13} /></button>
    </div>
  {/if}
  <div class="messages" bind:this={box} role="log" aria-label="Conversation">
    <div class="column" bind:this={column}>
      <Tube {column} live={busy && st.shape !== "wait"} />
      {#if gone}<p class="notice">This agent was closed.</p>{/if}
      {#each all as t, i (t.id)}
        {#if i === sinceAt}<div class="since">Since you last looked</div>{/if}
        <section class="turn" class:current={i === all.length - 1}>
          {#if t.user}
            <div class="msg-user" data-bead="turn" data-bead-y="26">
              <span class="bub">{#if t.user.text}<span class="text">{t.user.text}</span>{/if}
                {#if (t.user.images as string[] | undefined)?.length}<span class="pics">{#each t.user.images as string[] as id (id)}<Picture {id} />{/each}</span>{/if}
                {#if agent.status !== "notes" && !busy}
                  <span class="rewind">
                    <button title="Put it back in the box, to change and send again" onclick={() => { text = t.user!.text ?? ""; composerEl?.focus(); }}><Icon name="pencil" size={12} /> Edit and send again</button>
                    <button title="A copy of the conversation from just before this message, with it in the input to change (this one is kept)" onclick={() => rewind(t.user!.id, false)}><Icon name="undo" size={12} /> Rewind to here</button>
                    <button title="The same, and the files Claude changed from here on go back too" onclick={() => rewind(t.user!.id, true)}><Icon name="undo" size={12} /> Rewind, files too</button>
                  </span>
                {/if}<span class="when"><b>You</b>{#if t.seconds !== undefined} · {t.seconds}s{/if}</span></span>
            </div>
          {/if}
          {#if folded(i)}
            <button class="result" aria-expanded="false" onclick={() => toggle(t.id)}>
              <span class="ok"><Icon name="check" size={13} /></span><span class="what">{plain(t.result ?? "Done")}</span>
              <span class="n">{t.steps} steps</span><Icon name="chev-r" size={13} />
            </button>
          {:else}
            {#if expanded.has(t.id)}<button class="result" aria-expanded="true" onclick={() => toggle(t.id)}><span class="what quiet">Fold this turn</span><Icon name="chev-d" size={13} /></button>{/if}
            {#each t.rows as row, k (row.kind === "run" ? row.id : row.entry.id)}
              <div class="row" data-latest={i === all.length - 1 && k === t.rows.length - 1 ? "" : undefined}><Entry_ {row} agent={agent.id} quote={(said) => { text = `${said.split("\n").map((l) => `> ${l}`).join("\n")}\n\n${text}`; composerEl?.focus(); }} /></div>
            {/each}
          {/if}
        </section>
      {/each}
      {#if !all.length && !gone}<p class="empty">Nothing said yet. Tell this agent what to do.</p>{/if}
    </div>
  </div>

  {#if pinnedJob && jobs.some((j) => j.id === pinnedJob)}
    <section class="pinned" aria-label="A job's terminal">
      <div class="job-bar"><Icon name="terminal" size={13} /><b>{jobs.find((j) => j.id === pinnedJob)?.name}</b><span class="grow"></span>
        <button class="btn small" onclick={() => showJob?.(pinnedJob!)}>Open in Jobs</button>
        <button class="tiny" aria-label="Put it away" onclick={() => (pinnedJob = null)}><Icon name="x" size={13} /></button>
      </div>
      {#key pinnedJob}<Terminal job={pinnedJob} />{/key}
    </section>
  {/if}
  <div class="input-box">
    {#if problem}<p class="error">{problem}</p>{/if}
    {#if agent.status !== "notes"}<Plan agent={agent.id} />{/if}
    {#if agent.edited && agent.status !== "notes"}
      <div class="changes-bar">
        <Icon name="diff" size={13} /> {agent.edited === 1 ? "1 file" : `${agent.edited} files`} changed{#if lastTurnFiles.length}{" · "}{lastTurnFiles.length} in the last turn{/if}
        <button class="link" onclick={() => showChanges?.()}>Review</button>
      </div>
    {/if}
    {#if comments.length}
      <ul class="queued notes-on" aria-label="Notes on its changes, for your next message">
        {#each comments as c, i (i)}<li><span class="qt">{c.file}: {c.note}</span><button class="tiny" aria-label="Take this note out" onclick={() => (comments = comments.filter((_, k) => k !== i))}><Icon name="x" size={12} /></button></li>{/each}
      </ul>
    {/if}
    {#if queued.length}
      <ul class="queued" aria-label="Waiting for the turn to end">
        {#each queued as q, i (i)}
          <li><span class="qt">{q || "(pictures)"}</span><button class="btn small" onclick={() => run(command("unqueue", { agent: agent.id, index: i }))}>Take back</button></li>
        {/each}
      </ul>
    {/if}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="composer" ondragover={(e) => e.preventDefault()} ondrop={(e) => { e.preventDefault(); if (e.dataTransfer) attach(e.dataTransfer.files); }}>
      {#if attached.length || reading}
        <div class="attached">
          {#each attached as a, i (a.preview)}<span class="thumb"><img src={a.preview} alt="" /><button class="tiny" aria-label="Take this picture out" onclick={() => detach(i)}><Icon name="x" size={12} /></button></span>{/each}
          {#if reading}<span class="quiet small">Reading…</span>{/if}
        </div>
      {/if}
      {#if slash.length}
        <ul class="slash" role="listbox" aria-label="Commands">
          {#each slash as c, i (c.name)}<li role="option" aria-selected={i === pickAt}><button class:on={i === pickAt} onmousedown={(e) => { e.preventDefault(); take(c); }}><b>/{c.name}</b> <span>{c.about}</span></button></li>{/each}
        </ul>
      {/if}
      {#if mentions.length}
        <ul class="slash" role="listbox" aria-label="Files to mention">
          {#each mentions as m, i (m)}<li role="option" aria-selected={i === pickAt}><button class:on={i === pickAt} onmousedown={(e) => { e.preventDefault(); mention(m); }}><b>@{m}</b></button></li>{/each}
        </ul>
      {/if}
      <textarea onpaste={(e) => { if (e.clipboardData?.files.length) { e.preventDefault(); attach(e.clipboardData.files); } }} aria-label="Message to {agent.title || 'the agent'}" rows="2" placeholder={suggestion ? `${suggestion}   (Tab takes it)` : "Ask Claude…  Enter sends, Shift+Enter: a new line"} bind:value={text} bind:this={composerEl} onkeydown={keys} disabled={gone}></textarea>
      <div class="cbar">
        <button class="tiny" aria-label="Attach pictures" title="Attach pictures (or paste, or drop them here)" disabled={gone} onclick={() => picker?.click()}><Icon name="image" size={15} /></button>
        <input class="hidden" type="file" accept="image/*" multiple bind:this={picker} onchange={(e) => { attach(e.currentTarget.files ?? []); e.currentTarget.value = ""; }} />
        {#if sttOn}<button class="tiny" class:on={!!recorder} aria-pressed={!!recorder} aria-label={recorder ? "Stop and write it" : "Speak"} title={recorder ? "Stop: what you said goes in the box" : "Speak (speech to text)"} disabled={hearing || gone} onclick={listen}><Icon name="mic" size={15} /></button>{/if}
        {#if agent.modes?.length}
          <select class="chip" aria-label="Permission mode" value={agent.mode} onchange={(e) => run(command("set_mode", { agent: agent.id, mode: e.currentTarget.value }))}>
            {#each agent.modes as mode}<option value={mode}>Mode · {mode}</option>{/each}
          </select>
        {/if}
        {#each options as o (o.id)}
          <select class="chip" aria-label={o.name ?? o.id} title={o.name ?? o.id} value={o.currentValue} onchange={(e) => run(command("set_option", { agent: agent.id, option: o.id, value: e.currentTarget.value }))}>
            {#each flat(o.options) as v (v.value)}<option value={v.value}>{(o.name ?? o.id).replace(/ mode$/, "")} · {v.name}</option>{/each}
          </select>
        {/each}
        <span class="grow"></span>
        {#if st.shape === "wait"}<span class="status waiting">Waits for you</span>{:else if busy}<span class="status">Working</span>{/if}
        {#if busy && !ready}
          <button class="send stop" aria-label="Stop" title="Stop (Esc)" onclick={() => run(command("stop", { agent: agent.id }))}><Icon name="stop" /></button>
        {:else}
          <button class="send" aria-label="Send" disabled={!ready || reading || gone} onclick={send}><Icon name="send" /></button>
        {/if}
      </div>
    </div>
  </div>
</section>

</div>
