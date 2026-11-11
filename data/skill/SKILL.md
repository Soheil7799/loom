---
name: loom
description: Use when running inside loom (the loom MCP tools are available, or LOOM_SOCKET is set). Long-running or streaming commands — dev servers, watchers, builds, test suites, installs, log tails — go to loom's job tray through loom_run/loom_wait/loom_output instead of Bash; files the user should read or fill in are shown with loom_open.
---

# loom — the user's agent workspace

You are one of several Claude Code agents in **loom**, a GTK app the user watches. Besides
your chat it shows a **job tray** (one chip per named job, live output the user can peek or
pin), a **files panel** with an editor, and a sidebar of projects and agents. The `loom` MCP
server gives you tools to use them.

## Long-running commands → the job tray

Anything that runs for more than a few seconds or streams output goes to the tray, not Bash:
dev servers, `--watch` builds, test suites, `cargo build`, `npm i`, migrations, log tails.

- `loom_run(name, command, cwd?, restart?)` — starts it as job `name` in the project folder
  (or `cwd`, e.g. one repo of a multi-repo project). Returns at once.
- `loom_wait(name, until?, timeout_seconds?)` — blocks until it exits, until its output contains
  `until` (e.g. `"ready on"`, `"Finished"`), or the timeout. Returns state and the last lines.
- `loom_output(name, lines?)` — the latest output, whenever you need it.
- `loom_stop(name)`, `loom_jobs()`.

Names are a fixed vocabulary, one job per name, reused rather than duplicated:

| name | for |
|---|---|
| `dev` | dev server or watcher the work needs running |
| `test` | test suites, linters, type checks |
| `build` | builds, packaging, installs |
| `db` | migrations, seeds, database shells |
| `logs` | log tails |
| `research` | throwaway probes, benchmarks |

Two of a kind at once get a short qualifier (`dev-api`, `dev-web`), never `dev-2`. Running a
name again reruns that job; pass `restart: true` to replace one that is still running (a stale
dev server). Short, one-shot commands (`git status`, `ls`, a quick grep) stay in Bash.

## Finding your way in the code

- Don't know which file something is in? `loom_find(query)` first, in plain words ("where
  finished subagent rows are removed", "archive the done lane"). It answers with places
  (`path:line` and the definition's first line), not code: Read just those lines (`offset`,
  `limit`), not the whole file. Grep is still best for an exact string you already know.
- `loom_find` also answers from the project's **concept cards**: one small markdown file per
  feature in `.loom/map/` of the project folder (next to `.loom/board.md`). A card is the
  map for the next agent: where a feature lives, in the words people use for it.
- When you finish work that took searching to find your way (and whenever you move a board
  card to Done), add or update the card of the feature you touched. Keep it short, pointers
  not essays; the code stays the truth:

  ```markdown
  ---
  title: Subagent strip
  aliases: [crew, subagent rows, stuck rows]
  files: [core/src/agent/mod.rs, src/chat.rs, web/src/Conversation.svelte]
  symbols: [show_sub, crew_set, crew_remove]
  ---
  The live rows of subagents above the chat. loomd keeps them in the crew/<agent> store;
  the window and the web page only draw it. Why it is like this, and gotchas: a row that
  ends outside a turn is removed; rows stuck from before clear only when loomd restarts.
  ```

  `files` are relative to the project folder. One card per feature (update it rather than
  adding a second); name the file after it (`subagent-strip.md`). loom_find marks a card
  whose files or symbols are gone as possibly out of date: fix or delete it when you see one.
- A file you have read stays in your context. Don't Read it again unless it changed since
  (you edited it, or a command did). Every message re-reads the whole conversation, so each
  repeated file is paid for again and again.

## Show the user a file

`loom_open(path)` opens a file in loom's editor panel without taking their keyboard: use it
for a document or plan you wrote for them to read, a config to review. Mention in your reply
that it is open.

## Have the user fill a file in (secrets)

When something must go in a file that you must not see or have pasted into the chat (an API
key, a token, a password, personal data), don't ask for it in the chat: call
`loom_edit(path, message)`. loom opens the file for the user with your message and a Done
button, and the call waits until they press Done (or Cancel). With `secret: true` (the
default) you get back only which `KEY=value` lines now have a value, never the values: then
use the file without reading it or printing it (source it, point the program at it). Write
the keys it needs into the file first (`OPENAI_API_KEY=`), so the user only fills the values
in. Use `secret: false` for a file you may read once they are done.

## A command that needs the user to type (passwords, prompts)

A command that asks for something only the user may type (sudo's password, an SSH or GPG
passphrase, a y/n it won't take from a flag) runs with `loom_run(name, command, input: true,
message)`: its terminal opens in front of them with your message, and they type into it there.
Then `loom_wait(name, timeout_seconds: 600)` until it exits, and read `loom_output`. Never put
a password in the command or ask for it in the chat; never pipe one in with `echo`.

## Get their attention

`loom_notify(message)` sends a desktop notification. Only for something that needs them and
cannot wait for the end of your turn (loom already notifies when you ask a permission question
or finish).

## The user's notes

When the user has loom's Notes add-on on, two more tools reach their own notes: an Obsidian
vault about their life and work (finances, work, study, languages, health, their system's
setup…). Before answering about the user's own facts, plans or past decisions ("what did I
decide about…", "what's my rent", "how did I fix the Wi-Fi last time"), look there first:
`loom_note_search(query, folder?)` finds the notes (best first, with the matching lines and
their links), `loom_note_read(note)` reads one in full. Don't write to the vault yourself:
loom keeps it.

## The project's board

Each project has a board (`.loom/board.md`, a kanban the user tracks work on: Backlog, To do,
Doing, Done). `loom_board()` shows it, cards with their ids. Keep it true as you work:

- Starting on a card's task: `loom_board_move(card, "Doing")`; finished:
  `loom_board_note(card, "what was done, the commit")`, then `loom_board_move(card, "Done")`.
- Something found and not fixed now (a review finding, a bug beside your task, a follow-up):
  `loom_board_add(title, body)` files it in the Backlog, with where (file:line), why and the fix.
- A task the user gives that is bigger than this turn: file it, then move it as you go.

Don't edit `.loom/board.md` yourself: the tools date the cards. Work you finish with no card
is logged on the board by loom anyway.

## Other agents

`loom_agents()` lists the agents open in loom (title, project, status), e.g. to avoid editing
the same files as another one at the same time.
