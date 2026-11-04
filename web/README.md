# loom web

loom's web page: a plain client of loom's core API (`docs/api.md`), in Svelte, compiled to
static files that loom serves on 127.0.0.1 (`docs/trust.md`, "The web page").

```sh
npm install
npm run build                                   # → dist/
mkdir -p ~/.local/share/loom && cp -r dist ~/.local/share/loom/web
```

loom serves `$XDG_DATA_HOME/loom/web` (or `LOOM_WEB_UI=<folder>`); with neither, a small
built-in page. Open it with `loom api web --open` (the CLI's rights) or from `loomd --web`
(the window's: it can answer agents' questions).

What it has: the agents by project, their conversations as they stream (tool calls,
questions answered with buttons or a form), sending and stopping, an agent's permission
mode; the jobs and their output; what agents ask beyond the conversation (files to see or
fill in); with loomd, the Notes chat.
