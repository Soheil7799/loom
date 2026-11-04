# loomd: loom with no window

`loomd` is loom's core as its own program: the agents (restored as loom left them), their
jobs, the bridges (Telegram, your paired devices) and the core API, with nothing drawn. Every
face of loom is then a client of it: a web page (`loom api web --open`), the phone, the
Telegram bot, `loom api` in a terminal.

```sh
loomd                        # until SIGTERM or Ctrl+C
loomd --web                  # and a link to the web page, as loom's window would open it
LOOM_APP_ID=… loomd          # another instance's agents and settings, as with loom
```

- **One core per app id.** loomd doesn't start while a loom of its app id runs (window or
  loomd), and loom's window doesn't start while loomd does: two cores on the same agents
  would fight over them. Stop one to use the other; both read and save the same workspace.
- **Stopping** (SIGTERM, Ctrl+C) saves the workspace and stops the agents and their jobs, as
  closing the window does.
- **What the window would show, a UI shows** (`docs/api.md`):
  - files an agent wants you to see or fill in (`loom_open`, `loom_edit`) and its
    `loom_notify` are the `asks` resource: `ask_done` (filled in, or seen), `ask_open` (opens
    the file in your editor here); each also comes as a desktop notification (`notify-send`);
  - the Notes chat: `notes_open` (a new chat, or a past one from `notes_chats`) gives the
    chat's agent, whose conversation is `conversation/<agent>`; `notes_close`;
  - Scribe works as in the window: it reads conversations after their turns, and its
    questions and proposals are the `scribe` resource;
  - schedules: `schedules`, `schedule_act`; an agent's mode: `set_mode`; its title: `rename`.
- Pictures sent from a bridge: PNG, JPEG, WebP or GIF (made PNG).
- Agents' loom tools and the bridges run as the `loom` program installed beside loomd.
- **Who may do what** is as with the window (`docs/trust.md`). With no window, the window's
  rights (answering agents' questions above all) come only through `loomd --web`: it prints
  one link, on its own stderr, to whoever started it; that page is then as the window.
  `loom api web` gives a page the CLI's rights. The bridges act as themselves. Started from a
  systemd unit, loomd's stderr goes to the journal, which your agents can read too: use
  `--web` from a terminal.

The web page's UI is in `web/` (Svelte; `web/README.md` builds and installs it).

It is not started by itself (no service file is installed): run it when you want agents to
keep going with no window open, for instance from a systemd user unit of your own.
