# Bridges

Providers are what loom drives (Claude Code under each agent). Bridges are how something
outside loom reaches it: the Telegram bot, the remote bridge (your other devices), perhaps
another chat later.

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
| `send` | `agent`, `text`, `images` (base64, any format) | the agent; `!` commands are refused |
| `save` | `agent`, `name`, `data` (base64) | where it was saved (`<agent folder>/<bridge>-files/`) |
| `answer` | `agent`, `serial`, `option` | `{answered, agent}`; "always allow" is refused |
| `form` | `agent`, `serial`, `do`: `pick` (`field`, `option`), `write` (`text`), `send`, `skip` | `{done, problem, agent}` |
| `stop` | `agent` | the agent |
| `last_reply` | `agent` | its last reply (Markdown), or null |
| `history` | `agent`, `epoch`, `after` | its whole conversation, or what changed since (below) |
| `files` | `agent` | the files it created or changed, newest first: `{path, name, created, exists, size}` |
| `file` | `agent`, `path` | one of those, read-only: `{text, size, truncated}` (up to 512 KiB), or `{binary, size}` |
| `notes` | | the remote bridge only: loom's Notes chat, `{vault, chats: [{session, title, updated, ago, day, mark}], current}`; `vault` null when Notes is off; the list is read again each time (ask again for the newest) |
| `notes_open` | `session` (none: a new chat) | the remote bridge only: that chat open, as if picked in loom; its agent (then `send`, `history`, `answer`… as any agent) |
| `usage` | `agent` (optional) | `{account, agent}`: the account's limits `{plan, windows: [{kind, utilization, resetsAt}]}` (shared by every agent; `utilization` 0.33 is 33%, `resetsAt` Unix seconds), and that agent's `{used, size, cost}`; null until known |
| `transcribe` | `audio` (base64), `name`, `mime` | the text, with loom's speech-to-text settings |
| `state` | the bridge's own status | shown in Settings |

An agent is `{id, seq, title, project, status, unseen, busy, questions, notes}` (`notes`: a
Notes chat's agent, project "Notes"; its events go to the remote bridge only). `seq` grows with each
view loom makes: keep the newest. A question is `{serial, title, detail, options, fields,
is_form, writable, summary, blank}`.

### `history`

An agent's conversation as entries, in order:

| `kind` | Fields |
|---|---|
| `user` | `text`, `pictures` (how many came with it) |
| `agent`, `thought` | `text` (Markdown) |
| `tool` | `id`, `title`, `tool` (read, edit, execute…), `status`, `parent` (the task it ran under), `output` (its last 4,000 characters), `exit` |
| `notice` | `text`, `class` (`dim`, `error`…) |
| `turn` | `seconds` the turn took |

The answer is `{epoch, rev, reset, loading, entries}`, each entry with its place `i`. Entries
change as they stream in: send back the `epoch` and `rev` of the last answer as `epoch` and
`after` to get only the entries changed since (empty and 0 the first time). `reset`: drop what
you kept, `entries` are all of it (a new conversation, the agent read back again, or loom
restarted). `loading`: it is being read back (an agent loom restored but no one opened yet is
started by asking); ask again on its next event. Subagents' own conversations are not in it.

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

## The remote bridge

`loom bridge remote` runs `loom-remote bridge`, a program of its own beside loom: it brings
iroh and its async runtime, which loom itself doesn't need. It is an iroh endpoint, its secret
key made by loom when Remote is first switched on and kept in loom's settings. Its settings:

```json
{"key": "…", "relay": "", "devices": [{"id": "…", "name": "…"}], "pairing": null, "debug": false}
```

`relay` empty means n0's public relays. `pairing` is the one-time code while Settings shows
one (ten minutes at most). Its state, shown in Settings:
`{state: "online", id, relay, problem, ticket, candidate, online}`.

Devices speak ALPN `loom/remote/0`. Each request is a bidirectional stream: the device writes
`{"method", "params"}` and finishes its side; the bridge answers `{"ok"}` or `{"error"}` and
finishes. `subscribe` stays open and carries loom's events, one per line.

- **Pairing.** Settings shows a ticket (as a QR code): `loom:` and, in unpadded base64url,
  `{"addr": <the endpoint's address>, "code": "…"}`. The device connects and sends
  `pair {code, name}`; the user sees it in Settings and pairs it or refuses. Only then is it
  added to `devices`, and only then is the answer `ok`.
- **Paired devices** may send `agents`, `send`, `save`, `answer`, `form`, `stop`,
  `last_reply`, `history`, `files`, `file`, `usage`, `notes`, `notes_open`, `transcribe`
  (passed to loom as they are),
  `subscribe`, and `address` (where the desktop is now, kept by the device to find it next
  time).
- **Anyone else** is dropped as soon as the connection opens, unless a pairing code is
  shown; then `pair` is all it may send. Removing a device in Settings closes its connections.

`loom-remote pair <ticket>`, `agents`, `send`, `answer` and `watch` are such a device on the
command line. Both it and the phone apps are built on `loom-link` (`link/`), the device's
side as a library: `Desktop::pair`, `Desktop::connect`, a method per request, and
`subscribe`. It keeps nothing: the device's key and the desktop's address are the caller's
to store.

A ticket carries the relay and at most three direct addresses (this network's first; never
loopback or link-local), so its QR code stays easy to scan; once connected, `address` gives
the whole one.
