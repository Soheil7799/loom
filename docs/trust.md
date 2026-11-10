# Who may do what: loom's trust model

loom's core API is a local Unix socket (`$XDG_RUNTIME_DIR/loom/<name>-api.sock`; the folder
is 0700 and the socket 0600). This page says who connects to it, what each may do, and what
it does not protect against.

## The plain truth first

**An agent with unrestricted shell access is you.** Your agents run as your Unix user. With
a shell they can read and write any file you can: loom's settings, Claude Code's settings
and allow-rules, your shell's startup files. No socket rule changes that. The real line is
the permission Claude Code asks before running a command; loom's job is not to hand an
agent a quieter way around that line.

## Who is on the other end

loom decides, when a connection is made; a client never says who it is.

| Principal | Who | How loom knows |
|---|---|---|
| window | loom's own window | in-process, no socket |
| agent | an agent's tools, its commands, the jobs it ran in the tray | the connecting process is in that agent's systemd scope, or descends from its process or job |
| bridge | Telegram, a paired phone | started by loom, on a connection loom made and handed to it |
| CLI | anything else of yours (`loom api` in a terminal, a script) | none of the above |
| web page | a UI in a browser or webview, on this computer | the session its one-time link was traded for; it is whoever opened the link (the window, or the CLI) |

Each agent's process, and each job an agent runs in the tray, starts in a systemd scope of
its own (`loom-<pid>-a<agent>-<n>.scope`, through `systemd-run --user --scope`). A process
can't leave its cgroup by itself, so whatever an agent starts stays known as that agent's,
wherever it moves in the process tree. Without a user systemd, loom goes by the process tree
alone (set `LOOM_NO_SCOPES=1` to choose that).

An agent's loom tools (its MCP server) speak the same API, through one command, `tool`, which
acts as the agent loom found, never as one the caller names.

Other users are turned away. The connecting process is pinned with a pidfd when it
connects, so a process id reused afterwards can't be taken for it.

## What each may do

- **The window:** everything.
- **Bridges:** read and answer agents, without "Always allow" and without `!` commands
  (Telegram reads less: no notes).
- **The CLI:** read everything and do most things (send messages, start and close agents,
  run jobs, change most settings). It may **not**:
  - answer agents' questions, permission questions above all (only a person does that, in
    the window or through a paired bridge);
  - change what may leave this machine, what agents may do or a program loom runs: the
    bridges and pairing, speech to text, the agent program, the default permission mode,
    the editor, the notes vault;
  - decide for Scribe (its OKs, answers and rules write your notes).
- **Agents:** the agent list and their own conversation; nothing else.

## What this does not stop

The CLI is "whatever loom can't place under an agent". With scopes, `setsid`, `nohup`,
`cmd &` and tmux sessions stay placed; what starts a new systemd unit escapes: `systemd-run
--user`, a timer, a cron or `at` job, a script your shell runs later. Without scopes, any
process an agent leaves running on its own escapes too. Such a process is taken for the
CLI. That is why the CLI answers no questions and can't open a way off the
machine: whatever it can do, the agent could already do by editing your files.

**The strict sandbox** (Settings → Advanced, off by default) closes the systemd way out:
agents and the jobs they start run under Landlock (`loom sandbox`, Linux with Landlock ABI 9)
and reach no Unix socket but loom's own and the SSH and GPG agents', and no abstract one. No
session bus, no systemd: no new unit. They also lose sudo (no new privileges), `systemctl
--user`, notifications, gsettings, podman and portals; run those in the tray yourself (your
own tray commands are not sandboxed). What still escapes is a file
it edits that something else runs later outside it (your `.bashrc`, a git hook you trigger).
loom's own settings file is one of them: the API takes the switch only from loom's window,
but an agent can still write `"strictSandbox": false` into `settings.json`, which loomd reads
when it next starts. An SSH agent whose socket sits right in the runtime folder (systemd's
`ssh-agent.service`) stays out of reach: allowing its folder would let in the bus beside it.

## The web page

Off until asked for (`web_open`: loom's window, or `loom api web`), and on 127.0.0.1 only. The
link's token works once, within two minutes, and is traded for a session given in the
address's fragment (a browser sends a fragment to no server); the page keeps it for its tab
and takes it out of the address. Not a cookie: a browser sends this computer's cookies to
every port of it, so any local server the browser visits would read one. Requests must name
this computer (`127.0.0.1` or `localhost` at that port: a site whose name points here is
turned away), and the WebSocket must come from the page itself (`Origin`) with the session.
Nothing may frame the page, and no other site is answered. Another user of this computer can
reach the port, but has no session. `web_close` stops it and lets every page go, connected
ones too.

## loomd

The core with no window (`docs/loomd.md`). One core per app id: loomd and loom's window
never run together for the same agents. With no window, its rights come only through
`loomd --web`'s link, printed on loomd's own stderr to whoever started it.

## The audit log

Every command that comes through the socket is written to the state folder
(`~/.local/state/loom/audit.jsonl`): when, the principal, the process id and command line
that sent it, the method, its parameters (cut at 2,000 characters) and whether it was done.
Refused commands are logged too. The file is 0600 and never leaves the machine.

## Tests

A loom started with `LOOM_TEST_INSTANCE=1` lets the CLI answer questions, so end-to-end tests
can drive it through the socket (`cargo test --test e2e -- --ignored`). This is set when
loom starts, never by a client; a loom started that way hosts none of your agents.
