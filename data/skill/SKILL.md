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

## Show the user a file

`loom_open(path)` opens a file in loom's editor panel without taking their keyboard: use it
for an `.env` they must fill in, a document or plan you wrote for them to read, a config to
review. Mention in your reply that it is open.

## Get their attention

`loom_notify(message)` sends a desktop notification. Only for something that needs them and
cannot wait for the end of your turn (loom already notifies when you ask a permission question
or finish).

## Other agents

`loom_agents()` lists the agents open in loom (title, project, status), e.g. to avoid editing
the same files as another one at the same time.
