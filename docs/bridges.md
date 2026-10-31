# Bridges

Providers are what loom drives (Claude Code under each agent). Bridges are how something
outside loom reaches it: the Telegram bot today, perhaps a phone app or another chat later.

Each bridge is a process of its own, `loom bridge <name>`. loom starts it while the settings
switch it on, starts it again if it dies (after 2 s, then longer, up to a minute), and stops it
when switched off. When loom closes, the bridge's stdin closes and the bridge ends.

## Starting

loom writes one line on the bridge's stdin:

```json
{"socket": "/run/user/1000/loom/loom-bridges.sock", "secret": "…", "app": "dev.shirvani.Loom"}
```

The secret is made for that run only. It is never written to disk, the environment or the
arguments. The bridge socket is not the agents' control socket: an agent can't reach it
without the secret, so it can't answer its own permission requests.

## Requests

Connect to the socket, write one line, read one line back:

```json
{"secret": "…", "method": "agents", "params": {}}
```

→ `{"ok": …}` or `{"error": "…"}`.

| Method | Params | Answer |
|---|---|---|
| `hello` | | `{settings, agents}`: this bridge's settings and every agent |
| `agents` | | the agents, in the sidebar's order |
| `send` | `agent`, `text`, `images` (base64, any format) | the agent |
| `save` | `agent`, `name`, `data` (base64) | where it was saved (`<agent folder>/<bridge>-files/`) |
| `answer` | `agent`, `serial`, `option` | `{answered, agent}`; "always allow" is refused |
| `form` | `agent`, `serial`, `do`: `pick` (`field`, `option`), `write` (`text`), `send`, `skip` | `{done, problem, agent}` |
| `stop` | `agent` | the agent |
| `last_reply` | `agent` | its last reply (Markdown), or null |
| `transcribe` | `audio` (base64), `name`, `mime` | the text, with loom's speech-to-text settings |
| `state` | the bridge's own status | shown in Settings |

An agent is `{id, seq, title, project, status, unseen, busy, questions}`. `seq` grows with each
view loom makes: keep the newest. A question is `{serial, title, detail, options, fields,
is_form, writable, summary, blank}`.

## Events

Write `{"secret": "…", "method": "subscribe"}` and keep the connection open. loom writes one
event per line:

| Event | With |
|---|---|
| `agent` | the agent changed (status, questions…) |
| `asked` | it asks something |
| `done` | its turn ended: `reply`, `took`, `error`, `cancelled` |
| `unsent` | `count` queued messages never went out |
| `failed` | it stopped: `why` |
| `closed` | `agent` (its id) was closed |
| `settings` | this bridge's `settings` changed |

Agent events carry `agent` and `looking`: the user has that agent in front of them in loom, so
a bridge can stay quiet.

Nothing else is reachable. A bridge that needs more is a change to this protocol, reviewed as one.
