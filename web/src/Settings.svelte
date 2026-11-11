<script lang="ts">
  // Settings, as the window's: the same pages and words. Every change is saved at once
  // (`set_settings`); secrets are only sent, never shown. Remote pairs a device by a QR code
  // (`pairing`), confirmed here. Some settings only loom's window may change: a page opened
  // with `loomd --web` has its rights, one from `loom api web` the CLI's.
  import { command, follow, type Item } from "./lib/api";
  import { renderSVG } from "uqr";
  import Icon from "./Icon.svelte";

  type Row = { key: string; title: string; detail?: string; kind: "switch" | "select" | "text" | "number" | "secret"; options?: [unknown, string][]; when?: string };
  type Page = { name: string; sections: { title: string; rows: Row[] }[] };

  const MODES: [unknown, string][] = [["", "Claude Code's default"], ["default", "Ask before acting"], ["acceptEdits", "Accept edits"], ["plan", "Plan only"], ["auto", "Auto"]];
  const IDLE: [unknown, string][] = [[0, "Never"], [5, "After 5 min"], [15, "After 15 min"], [30, "After 30 min"], [45, "After 45 min"], [60, "After 1 hour"], [120, "After 2 hours"]];
  const TOKENS: [unknown, string][] = [[50000, "Over 50K tokens"], [100000, "Over 100K tokens"], [200000, "Over 200K tokens"], [400000, "Over 400K tokens"]];
  const SCALES: [unknown, string][] = [[0.9, "Small"], [1, "Normal"], [1.1, "Large"], [1.25, "Larger"], [1.4, "Largest"]];

  const PAGES: Page[] = [
    { name: "General", sections: [
      { title: "Notifications", rows: [
        { key: "notify_questions", title: "When an agent asks for permission", kind: "switch" },
        { key: "notify_done", title: "When an agent finishes a turn", kind: "switch" },
      ] },
      { title: "Behaviour", rows: [
        { key: "sort_by_urgency", title: "Agents that need you first", detail: "One waiting for your answer moves to the top of its project until answered.", kind: "switch" },
        { key: "start_on_overview", title: "When loom starts", detail: "The overview shows every agent at a glance and resumes none until you open it.", kind: "select", options: [[true, "The overview"], [false, "The last agent"]] },
        { key: "show_running", title: "Running section open", detail: "In the window's sidebar, the agents running now, above the projects. Folded, its header still counts who needs you. Ctrl+1…9 go by it.", kind: "switch" },
        { key: "cycle_running", title: "Ctrl+Tab cycles running agents only", detail: "Suspended ones are skipped, so passing one doesn't wake it.", kind: "switch" },
        { key: "confirm_quit", title: "Confirm quitting while agents work", kind: "switch" },
        { key: "scratch_autohide", title: "Hide the scratch terminal when it loses focus", kind: "switch" },
      ] },
    ] },
    { name: "Agents", sections: [
      { title: "New agents", rows: [{ key: "default_mode", title: "Permission mode", detail: "What a new agent may do without asking. Shift+Tab changes it per agent.", kind: "select", options: MODES }] },
      { title: "Conversation", rows: [
        { key: "show_thinking", title: "Show thinking", detail: "Claude's reasoning, folded under each answer.", kind: "switch" },
        { key: "suggest_prompts", title: "Suggest your next message", detail: "After a turn, Claude Code suggests what you'll write next; → or Tab fills it in.", kind: "switch" },
        { key: "compact_alert", title: "Remind me to compact", detail: "Every message re-reads the whole context, so a long conversation costs more each turn. Once it holds this many tokens, the meter turns amber and a Compact button shows by it.", kind: "switch" },
        { key: "compact_tokens", title: "Compact at (tokens)", kind: "number", when: "compact_alert" },
      ] },
      { title: "Idle agents", rows: [
        { key: "idle_minutes", title: "Suspend idle agents", detail: "Frees an unwatched agent's memory; opening it resumes the conversation.", kind: "select", options: IDLE },
        { key: "rest_compact", title: "Compact before suspending", detail: "Sums up its conversation first, so resuming it is quick and cheap. Never the open agent, nor one waiting on you.", kind: "switch", when: "idle_minutes" },
        { key: "rest_compact_tokens", title: "When its context is", kind: "select", options: TOKENS, when: "rest_compact" },
      ] },
      { title: "Meaning search", rows: [
        { key: "embed_code", title: "For the code", detail: "loom_find ranks the code by what a question means, not only by its words: a local model, through Ollama on this machine (started, and the model downloaded, when needed).", kind: "switch" },
        { key: "embed_notes", title: "For the notes", detail: "The note search agents use goes by meaning too.", kind: "switch" },
        { key: "embed_model", title: "Model", detail: "Changing it embeds the code and notes again, once (minutes; the old vectors are kept).", kind: "select", options: [["embeddinggemma", "embeddinggemma (best on code in loom's tests)"], ["qwen3-embedding:0.6b", "qwen3-embedding 0.6B"]] },
      ] },
    ] },
    { name: "Appearance", sections: [{ title: "Text", rows: [
      { key: "text_scale", title: "Conversation text size", detail: "loom's window; this page follows your browser's zoom.", kind: "select", options: SCALES },
      { key: "terminal_font", title: "Terminal font", detail: "The scratchpad and the tray's terminals, in loom's window.", kind: "text" },
    ] }] },
    { name: "Claude", sections: [] },
    { name: "Advanced", sections: [
      { title: "Other editor", rows: [{ key: "editor", title: "Command", detail: "Opens files there (code, gnome-text-editor…); empty uses the app your system opens them with.", kind: "text" }] },
      { title: "Agent program", rows: [{ key: "adapter", title: "ACP adapter", detail: "Empty: agents run Claude Code itself. An ACP adapter's command here runs them through it instead. For agents started from now on.", kind: "text" }] },
      { title: "Troubleshooting", rows: [{ key: "debug_log", title: "Debug log", detail: "What loom does, in detail, in loom's state folder (debug.log). Holds parts of conversations; stays on this computer.", kind: "switch" }] },
    ] },
    { name: "Notes", sections: [{ title: "", rows: [
      { key: "notes_enabled", title: "Use Notes", detail: "What you tell your agents is kept in the vault below, sorted by topic.", kind: "switch" },
      { key: "notes_vault", title: "Vault", detail: "The vault's folder, on this computer.", kind: "text" },
      { key: "notes_daily_calls", title: "Calls a day", detail: "At most this many calls to Claude a day for notes. Past it, checks wait for tomorrow.", kind: "number" },
      { key: "scribe_model", title: "Model", detail: "What Scribe reads and writes your notes with. Haiku is the lightest on your usage limits.", kind: "select", options: [["haiku", "Haiku"], ["sonnet", "Sonnet"], ["opus", "Opus"]] },
      { key: "scribe_effort", title: "Effort", detail: "How hard it thinks on each call. Higher is slower and takes more of your usage limits.", kind: "select", options: [["", "Default"], ["low", "Low"], ["medium", "Medium"], ["high", "High"]] },
    ] }] },
    { name: "Telegram", sections: [{ title: "", rows: [
      { key: "telegram_enabled", title: "Use the Telegram bot", detail: "Your agents' replies and questions in Telegram, and your messages to them.", kind: "switch" },
      { key: "telegram_token", title: "Bot token", detail: "From @BotFather. Kept by loom; never shown again.", kind: "secret" },
      { key: "telegram_user", title: "Your Telegram user ID", detail: "The one account the bot answers; everyone else is ignored.", kind: "number" },
    ] }] },
    { name: "Remote", sections: [{ title: "Connection", rows: [
      { key: "remote_enabled", title: "Use the remote bridge", detail: "Your paired devices reach your agents, end-to-end encrypted.", kind: "switch" },
      { key: "remote_relay", title: "Relay", detail: "Helps your devices find each other. Your own (iroh-relay) or, empty, the public ones of n0, who make iroh. A relay can't read what it passes.", kind: "text" },
    ] }] },
    { name: "Speech to text", sections: [{ title: "", rows: [
      { key: "stt_enabled", title: "Use speech to text", detail: "Voice messages sent to the Telegram bot.", kind: "switch" },
      { key: "stt_url", title: "Address", detail: "An OpenAI-style transcription endpoint (Groq, OpenAI, or one of your own).", kind: "text" },
      { key: "stt_key", title: "Key", detail: "Kept by loom; never shown again.", kind: "secret" },
      { key: "stt_model", title: "Model", detail: "As the service names it (Groq: whisper-large-v3-turbo; OpenAI: whisper-1).", kind: "text" },
      { key: "stt_language", title: "Language", detail: "Empty: found from the speech. Or a code: fa, en, it…", kind: "text" },
    ] }] },
  ];

  let s = $state.raw<Item | null>(null);
  let bridges = $state.raw<Item[]>([]);
  let ticket = $state("");
  let page = $state(PAGES[0].name);
  let secrets = $state<Record<string, string>>({});
  let problem = $state("");
  let browserNotes = $state("Notification" in window ? Notification.permission : "denied");
  type Topic = { name: string; path: string; folder: string; listens_for: string[]; apply: string; sync: boolean; enabled: boolean };
  type ClaudeItem = { kind: string; name: string; description: string; enabled: boolean; shipped: boolean; path: string };
  let topics = $state.raw<Topic[] | null>(null);
  let claude = $state.raw<{ folder: string; items: ClaudeItem[] } | null>(null);
  let newTopic = $state({ name: "", about: "" });
  let drafting = $state(false);
  let newItem = $state<Record<string, string>>({});

  async function loadTopics() { try { topics = (await command("topics", {})).topics; } catch (e) { topics = []; problem = (e as Error).message; } }
  async function loadClaude() { try { claude = await command("claude_items", {}); } catch (e) { problem = (e as Error).message; } }
  $effect(() => { if (page === "Notes" && s?.notes_enabled) loadTopics(); });
  $effect(() => { if (page === "Claude") loadClaude(); });
  const edit = (path: string) => run(command("edit_file", { path }));
  async function makeTopic(e: SubmitEvent) {
    e.preventDefault();
    drafting = true;
    try {
      const made = await command("topic_new", { name: newTopic.name.trim(), ...(newTopic.about.trim() ? { about: newTopic.about.trim() } : {}) });
      newTopic = { name: "", about: "" };
      problem = "";
      await loadTopics();
      edit(made.path);
    } catch (err) { problem = (err as Error).message; }
    drafting = false;
  }
  async function makeItem(kind: string) {
    const name = (newItem[kind] ?? "").trim();
    if (!name) return;
    try {
      const made = await command("claude_new", { kind, name });
      newItem = { ...newItem, [kind]: "" };
      await loadClaude();
      edit(made.path);
    } catch (err) { problem = (err as Error).message; }
  }
  const KINDS: [string, string][] = [["skill", "Skills"], ["agent", "Agents"], ["command", "Commands"]];

  $effect(() => follow("settings", (f) => (s = f.items[0] ?? null)));
  $effect(() => follow("bridges", (f) => (bridges = f.items)));
  $effect(() => follow("pairing", (f) => (ticket = f.items[0]?.ticket ?? "")));

  const shown = $derived(PAGES.find((p) => p.name === page)!);
  const remote = $derived(bridges.find((b) => b.id === "remote"));
  const telegram = $derived(bridges.find((b) => b.id === "telegram"));
  const devices = $derived((s?.remote_devices ?? []) as [string, string][]);
  const qr = $derived(ticket ? renderSVG(ticket, { border: 2 }) : "");

  async function run(p: Promise<unknown>) {
    problem = "";
    try { await p; return true; } catch (e) { problem = (e as Error).message; return false; }
  }
  const set = (key: string, value: unknown) => run(command("set_settings", { changes: { [key]: value } }));
  async function saveSecret(key: string) {
    if (await set(key, secrets[key] ?? "")) secrets = { ...secrets, [key]: "" };
  }
  // a select's value in the setting's own type, as the options keep it
  const pickOption = (row: Row, i: number) => set(row.key, row.options![i][0]);
</script>

<section class="settings" aria-label="Settings">
  <header class="header"><span class="headline"><span class="title">Settings</span></span></header>
  <div class="settings-split">
    <nav class="settings-nav" aria-label="Settings pages">
      {#each PAGES as p (p.name)}
        <button class="arow" class:sel={p.name === page} aria-current={p.name === page ? "page" : undefined} onclick={() => (page = p.name)}><span class="t">{p.name}</span></button>
      {/each}
    </nav>
    <div class="settings-page">
      {#if problem}<p class="error" role="alert">{problem}</p>{/if}
      {#if !s}
        <p class="empty">Loading…</p>
      {:else}
        {#if page === "General"}
          <div class="setting">
            <div class="what"><span>Notifications in this browser</span><p>When an agent waits for you or finishes while this page is out of sight (the switches below say which).</p></div>
            <div class="control">
              {#if !("Notification" in window)}<span class="quiet small">This browser has none.</span>
              {:else if browserNotes === "granted"}<span class="quiet small">On</span>
              {:else if browserNotes === "denied"}<span class="quiet small">Blocked: allow them in the browser's site settings</span>
              {:else}<button class="btn small" onclick={async () => (browserNotes = await Notification.requestPermission())}>Allow</button>{/if}
            </div>
          </div>
        {/if}
        {#if page === "Telegram" && telegram}<p class="bridge-state">The bot: <b>{telegram.state}</b>{#if telegram.why}{" · "}{telegram.why}{/if}</p>{/if}
        {#if page === "Remote" && remote}<p class="bridge-state">The bridge: <b>{remote.state}</b>{#if remote.why}{" · "}{remote.why}{/if}</p>{/if}

        {#each shown.sections as sec (sec.title)}
          <section class="settings-section">
            {#if sec.title}<h3>{sec.title}</h3>{/if}
            {#each sec.rows.filter((r) => !r.when || s![r.when]) as row (row.key)}
              <div class="setting">
                <div class="what"><label for="set-{row.key}">{row.title}</label>{#if row.detail}<p>{row.detail}</p>{/if}</div>
                <div class="control">
                  {#if row.kind === "switch"}
                    <input id="set-{row.key}" type="checkbox" role="switch" class="switch" checked={!!s[row.key]} onchange={(e) => set(row.key, e.currentTarget.checked)} />
                  {:else if row.kind === "select"}
                    <select id="set-{row.key}" class="chip" onchange={(e) => pickOption(row, e.currentTarget.selectedIndex)}>
                      {#each row.options! as [value, label], i (i)}<option selected={value === s[row.key]}>{label}</option>{/each}
                    </select>
                  {:else if row.kind === "number"}
                    <input id="set-{row.key}" type="number" min="0" value={s[row.key]} onchange={(e) => set(row.key, Number(e.currentTarget.value))} />
                  {:else if row.kind === "secret"}
                    <form class="adding" onsubmit={(e) => { e.preventDefault(); saveSecret(row.key); }}>
                      <input id="set-{row.key}" type="password" autocomplete="off" placeholder={s[`${row.key}_set`] ? "Set: type to replace" : "Not set"} bind:value={secrets[row.key]} />
                      <button class="go" disabled={!secrets[row.key]}>Save</button>
                    </form>
                  {:else}
                    <input id="set-{row.key}" value={s[row.key] ?? ""} onchange={(e) => set(row.key, e.currentTarget.value.trim())} />
                  {/if}
                </div>
              </div>
            {/each}
          </section>
        {/each}

        {#if page === "Telegram" && telegram?.state === "connected"}
          <section class="settings-section">
            <h3>Your account</h3>
            {#if telegram.bot}<p>Connected as <b>@{telegram.bot}</b>.{#if telegram.user}{" "}Linked to account {telegram.user}.{/if}</p>{/if}
            {#if telegram.code}
              <p>Send the bot this code: <code>{telegram.code}</code>{#if telegram.bot}{" · "}<a href="https://t.me/{telegram.bot}?start={telegram.code}" target="_blank" rel="noreferrer">Open in Telegram</a>{/if}</p>
            {/if}
            {#if telegram.candidate}
              <div class="permission-card">
                <div class="qh"><span class="needs">sent the code</span><b>{telegram.candidate[1]} ({telegram.candidate[0]})</b></div>
                <p>Link it only if this is you: the bot obeys this one account.</p>
                <div class="controls"><button class="btn suggested" onclick={() => set("telegram_user", telegram.candidate[0])}>Link this account</button></div>
              </div>
            {/if}
          </section>
        {/if}

        {#if page === "Notes" && s.notes_enabled}
          <section class="settings-section">
            <h3>Topics</h3>
            {#each topics ?? [] as t (t.name)}
              <div class="setting">
                <div class="what"><span>{t.name}</span><p>{t.folder}/ · {t.listens_for.length ? `listens for ${t.listens_for.slice(0, 4).join(", ")}${t.listens_for.length > 4 ? "…" : ""}` : "listens for nothing yet"} · {t.apply === "auto" ? "changes directly" : "asks before changing"}{t.sync ? "" : " · stays on this computer"}</p></div>
                <div class="control row"><button class="btn small" onclick={() => edit(t.path)}>Edit</button><input type="checkbox" role="switch" class="switch" aria-label="Use {t.name}" checked={t.enabled} onchange={async (e) => { await run(command("topic_set", { name: t.name, enabled: e.currentTarget.checked })); loadTopics(); }} /></div>
              </div>
            {/each}
            <form class="new-topic" onsubmit={makeTopic}>
              <input aria-label="New topic's name" placeholder="Surfing, Garden…" bind:value={newTopic.name} />
              <input aria-label="What it keeps (optional)" placeholder="Optional: what it keeps, in a sentence (I surf; track sessions, spots and boards)" bind:value={newTopic.about} />
              <button class="go" disabled={!newTopic.name.trim() || drafting}>{drafting ? "Drafting…" : "Create"}</button>
            </form>
            <p class="quiet">Its file opens in your editor, on this computer.</p>
          </section>
        {/if}

        {#if page === "Claude" && claude}
          <section class="settings-section">
            <h3>Folder</h3>
            <p class="mono small-path">{claude.folder}</p>
            <p class="quiet">Skills, agents and commands only loom's agents use; claude in a terminal doesn't load them. Changes reach agents started or woken from now on.</p>
          </section>
          {#each KINDS as [kind, title] (kind)}
            <section class="settings-section">
              <h3>{title}</h3>
              {#each claude.items.filter((i) => i.kind === kind) as i (i.name)}
                <div class="setting">
                  <div class="what"><span class="mono">{i.name}</span>{#if i.description}<p>{i.description.length > 140 ? i.description.slice(0, 140) + "…" : i.description}</p>{/if}</div>
                  <div class="control row">
                    {#if i.shipped}<button class="btn small" title="Put back loom's own version" onclick={async () => { if (confirm(`Put back loom's own ${i.name}? Your changes to it go.`)) { await run(command("claude_restore", { name: i.name })); loadClaude(); } }}>Restore</button>{/if}
                    <button class="btn small" disabled={!i.enabled} onclick={() => edit(i.path)}>Edit</button>
                    <input type="checkbox" role="switch" class="switch" aria-label="Use {i.name}" checked={i.enabled} onchange={async (e) => { await run(command("claude_set", { kind, name: i.name, enabled: e.currentTarget.checked })); loadClaude(); }} />
                  </div>
                </div>
              {/each}
              <form class="adding" onsubmit={(e) => { e.preventDefault(); makeItem(kind); }}>
                <input class="mono" aria-label="New {kind}'s name" placeholder="name" bind:value={newItem[kind]} />
                <button class="go" disabled={!newItem[kind]?.trim()}>Create</button>
              </form>
            </section>
          {/each}
        {/if}

        {#if page === "Remote"}
          <section class="settings-section">
            <h3>Devices</h3>
            {#each devices as [id, name] (id)}
              <div class="setting">
                <div class="what"><span>{name || id}</span><p class="mono">{id.slice(0, 16)}…</p></div>
                <div class="control"><button class="btn deny" onclick={() => set("remote_devices", devices.filter(([d]) => d !== id))}>Remove</button></div>
              </div>
            {:else}
              <p class="quiet">No device paired yet.</p>
            {/each}
            {#if remote?.candidate}
              <div class="permission-card">
                <div class="qh"><span class="needs">asks to pair</span><b>{remote.candidate.name || remote.candidate.id}</b></div>
                <p>Let it in only if it's the device you're pairing now.</p>
                <div class="controls"><button class="btn suggested" onclick={() => run(command("pair_confirm", { id: remote.candidate.id }))}>Let it in</button><button class="btn" onclick={() => run(command("pairing", { open: false }))}>No</button></div>
              </div>
            {/if}
            {#if ticket}
              <div class="pairing">
                <div class="qr" aria-label="The pairing code, as a QR code" role="img">{@html qr}</div>
                <div>
                  <p>Scan this with the loom app, or paste the ticket into: <code>loom-remote pair &lt;ticket&gt;</code>. You confirm the device here before it is added. It stays good for ten minutes.</p>
                  <div class="controls">
                    <button class="btn" onclick={() => navigator.clipboard?.writeText(ticket)}>Copy the ticket</button>
                    <button class="btn" onclick={() => run(command("pairing", { open: false }))}>Stop pairing</button>
                  </div>
                </div>
              </div>
            {:else}
              <div class="setting">
                <div class="what"><span>Pair a device</span><p>Shows a code to scan. It stays good for ten minutes.</p></div>
                <div class="control"><button class="btn" disabled={!s.remote_enabled} onclick={() => run(command("pairing", { open: true }))}><Icon name="plus" size={13} /> Pair</button></div>
              </div>
              {#if !s.remote_enabled}<p class="quiet">Switch the remote bridge on first.</p>{/if}
            {/if}
            <p class="quiet">A paired device can do what the Telegram bot can: read replies, send messages, answer questions. “Always allow” and ! commands stay in loom.</p>
          </section>
        {/if}
      {/if}
    </div>
  </div>
</section>
