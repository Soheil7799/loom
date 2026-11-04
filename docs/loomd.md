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
- **What needs a window isn't there:** an agent's `loom_open` and `loom_edit` answer that
  nothing can be shown; the Notes chat and Scribe run only in the window, for now. Pictures
  sent from a bridge are taken as PNG only (the window reads any format).
- **Who may do what** is as with the window (`docs/trust.md`). With no window, the window's
  rights (answering agents' questions above all) come only through `loomd --web`: it prints
  one link, on its own stderr, to whoever started it; that page is then as the window.
  `loom api web` gives a page the CLI's rights. The bridges act as themselves. Started from a
  systemd unit, loomd's stderr goes to the journal, which your agents can read too: use
  `--web` from a terminal.

It is not started by itself (no service file is installed): run it when you want agents to
keep going with no window open, for instance from a systemd user unit of your own.
