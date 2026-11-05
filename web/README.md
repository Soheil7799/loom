# loom web

loom's web page: a plain client of loom's core API (`docs/api.md`), in Svelte, compiled to
static files that loom serves on 127.0.0.1 (`docs/trust.md`, "The web page").

```sh
bun install
bun run build          # → dist/, which the next cargo build compiles into loom and loomd
bun run check          # the API's types (src/lib/api-types.ts, from ../docs/api.schema.json), then svelte-check
```

TypeScript, checked by `svelte-check` (warnings fail the build). `src/lib/api-types.ts` is
generated from the API's spec by `scripts/api-types.ts`: don't edit it by hand. The look is
loom/default (`src/theme.css`: the Nocturne colours as CSS variables, Onest + JetBrains Mono
bundled from `src/fonts/`, SIL OFL; text under 14 px stays regular weight).

A loom built after `bun run build` has the page in its binary (the release does); one built
without it serves `$XDG_DATA_HOME/loom/web`, else a small built-in page. `LOOM_WEB_UI=<folder>`
serves that folder instead, whichever was built in (`LOOM_WEB_UI=web/dist` while working on it).
Open it with `loom api web --open` (the CLI's rights) or from `loomd --web`
(the window's: it can answer agents' questions).

What it has: the agents by project, their conversations as they stream (tool calls,
questions answered with buttons or a form), sending and stopping, an agent's permission
mode; the jobs and their output; what agents ask beyond the conversation (files to see or
fill in); with loomd, the Notes chat.
