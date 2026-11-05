# loom's core API

Generated from `core/src/api/spec.rs` (and `wire`'s types): edit there, then `LOOM_BLESS=1 cargo test -p loom-core spec`. Who may do what, and why: `docs/trust.md`.

## The wire

JSON-RPC 2.0, one message per line, on `$XDG_RUNTIME_DIR/loom/<name>-api.sock` (the folder 0700, the socket 0600), or on a connection loom hands down (bridges: fd 3). Params and results are objects; ids of commands are strings unique to the client (sent again, a command isn't done again: the first result comes back). Readers ignore what they don't know; changes are additive only.

A subscription answers its snapshot in pages, then `synced`, then numbered `changes`; `seq` counts its messages with no gaps; a client that falls behind gets a `resync` and a fresh snapshot. Ops: `upsert` (a whole item by id), `remove`, `append` (text to a field), `order`, `bytes`.

| Protocol method | Params | Result |
|---|---|---|
| `initialize` | `{protocol: {min, max}, client: {name, version}, capabilities}` | {protocol, server, principal, capabilities: {subscriptions: {page, queue}, commands}} — first, once |
| `subscribe` | `{resource}` | {sub, resource}; its messages follow as `subscription` notifications |
| `unsubscribe` | `{sub}` | {} |
| `resync` | `{sub}` | {} — a fresh snapshot follows (after a gap) |

A web page (a UI in a browser or a webview) reaches the API through `web_open`'s link: http://127.0.0.1:<port>/?t=<token>. The token is traded once for a session, given in the address's fragment (`/#<session>`: sent to no server; the page keeps it for its tab and takes it out of the address); then `ws://<host>/api?s=<session>` is a WebSocket carrying the same messages, one per text frame. `Host` must be 127.0.0.1 or localhost at that port and the WebSocket's `Origin` that page; there are no CORS headers and the page can't be framed. `web_close` lets every page go, connected ones too. The page's files: `LOOM_WEB_UI`, else the UI built into loom, else `<data dir>/loom/web` (its index.html), else a small page built in.

Server notification: `subscription`, params a message (`{sub, seq, kind, …}`, see `docs/api.schema.json`).

Errors: -32700 not JSON, -32600 not a request, -32601 no such method, -32602 invalid params, -32002 initialize first, -32003 no protocol in common, -32010 not found, -32011 forbidden, -32012 conflict (answered elsewhere first), -32013 failed; `data.kind` names it.

## Resources

### `agents`

The open agents, in the sidebar's order (Notes chats not among them).

Who: window, cli, bridge, agent.

| Field | Type | |
|---|---|---|
| `id` | string | the agent's id |
| `title` | string |  |
| `project` | string | its project's name |
| `project_id` | string | its project's id (folder) |
| `status` | string | idle, working, asking, suspended, … |
| `unseen` | boolean | something new the user hasn't seen |
| `busy` | boolean | a turn under way |
| `questions` | integer | questions waiting |
| `used` | string | last used (RFC 3339) |
| `place` | string | where in the project it works (a repo, a folder) |
| `edited` | integer | files it changed |
| `awake` | boolean | kept from resting |
| `draft` | string? | what waits in its input (a rewind's message), as last published |
| `options` | array? | the session's settings (ACP configOptions: {id, name, category, currentValue, options: [{value, name} or groups]}): model, effort…; `set_option` |
| `queued` | array? | messages waiting for the turn to end, in order (`unqueue`) |
| `stalled` | boolean? | a turn quiet for 3 minutes, no tool running |
| `looking` | boolean? | the user has it in front of them in loom |
| `mode` | string? | its permission mode |
| `modes` | array? | the modes it can take (`set_mode`) |
| `last_turn` | object? | {turns, seconds, error, cancelled}: `turns` grows as each ends |
| `failed` | object? | {times, why}: its process failing |
| `unsent` | object? | {times, count}: queued messages that never went out |

### `projects`

The projects, in the sidebar's order.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string | its folder |
| `name` | string |  |
| `collapsed` | boolean | folded in the sidebar |

### `conversation/<agent>`

An agent's conversation: Agent → turn → entry. Questions are entries `awaiting_user` until answered (`answer`).

Who: window, cli, bridge, agent (its own).

| Field | Type | |
|---|---|---|
| `id` | string | e1, e2… |
| `kind` | string | user, agent, thought, tool, question, notice, turn…; an unknown one is drawn plainly |
| `status` | string | pending, streaming, awaiting_user, done, failed, cancelled |
| `turn_id` | string? | t1, t2…: the turn it belongs to |
| `text` | string? | text; streamed as `append` ops |
| `message` | string? | the agent's id for it (rewind, forks) |
| `images` | array? | pictures: blob ids (`blob`) |
| `call` | string? | a tool's call id |
| `title` | string? | a tool's title |
| `tool` | string? | a tool's kind |
| `input` | object? | a tool's input |
| `content` | array? | a tool's content (diffs…) |
| `locations` | array? | files a tool touched |
| `output` | string? | a tool's output (its last 16,000 characters) |
| `exit` | integer? | a command's exit code |
| `parent` | string? | the task a tool is part of |
| `detail` | string? | a question's detail |
| `options` | array? | a permission question's options: {id, name, kind} |
| `form` | object? | a form question's JSON Schema |
| `answer` | string? | how a question was answered |
| `seconds` | integer? | a turn's length |

### `usage`

The account's limits and each agent's context and cost.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string | `account`, or an agent's id |
| `kind` | string | account or agent |
| `plan` | string? | the account's plan |
| `windows` | array? | limits: {kind, utilization, resets_at, seen_at} |
| `used` | integer? | an agent's context used |
| `size` | integer? | its context's size |
| `cost` | object? | {amount, currency} |
| `limit` | object? | a spending limit |

### `jobs`

Every project's jobs, oldest first.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string |  |
| `project_id` | string |  |
| `name` | string |  |
| `command` | string |  |
| `status` | string | running, restarting, exited |
| `exit_code` | integer? |  |

### `job/<id>`

A job's terminal: its screen, then `bytes` ops at their offsets (drop what the screen holds; past it, resync). A new run: an `upsert`.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string | `output` |
| `rows` | integer |  |
| `cols` | integer |  |
| `screen` | string | base64 terminal bytes drawing its screen |
| `offset` | integer | the stream's byte the screen is at |

### `settings`

One item: every setting by its snake_case name; secrets only as `<name>_set`.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string | `settings` |

### `vault/<folder>`

A folder of the notes vault (`vault/`: the top), folders first; it follows the files.

Who: window, cli, bridge:remote.

| Field | Type | |
|---|---|---|
| `id` | string | its path |
| `name` | string |  |
| `path` | string |  |
| `kind` | string | folder, note, file, error |
| `count` | integer? | a folder's entries |
| `title` | string? | a note's |
| `size` | integer? |  |
| `modified` | integer? |  |

### `note/<path>`

One note (or file), as read; it follows the file.

Who: window, cli, bridge:remote.

| Field | Type | |
|---|---|---|
| `id` | string | `note`, or `error` |
| `path` | string? |  |
| `text` | string? |  |
| `title` | string? |  |
| `tags` | array? |  |
| `links` | array? |  |
| `backlinks` | array? |  |
| `modified` | integer? |  |
| `message` | string? | an error's |

### `notes_chats`

loom's Notes chats.

Who: window, cli, bridge:remote.

| Field | Type | |
|---|---|---|
| `id` | string | its session |
| `title` | string |  |
| `updated` | string? |  |
| `mark` | string? |  |
| `current` | boolean? | the one open |

### `scribe`

Scribe: what it does, asks, proposes and wrote.

Who: window, cli.

| Field | Type | |
|---|---|---|
| `id` | string | `status`, or `<list>:<n>` |
| `list` | string? | questions, proposals, recent, teach |
| `doing` | string? | status: what it is doing |
| `running` | boolean? |  |
| `calls_left` | integer? |  |

### `crew/<agent>`

An agent's subagents (tasks it runs), while its turn goes on.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string | the subagent's session, or its task's tool call |
| `name` | string |  |
| `step` | string | what it did last |
| `state` | string? | running, requires_action, idle |
| `started` | string |  |
| `at` | string | when it last did something |
| `ended` | string? |  |
| `guessed` | boolean | no session of its own: its state is a guess from its steps |

### `closed`

Conversations closed lately, newest first (`reopen`).

Who: window, cli.

| Field | Type | |
|---|---|---|
| `id` | string | its session |
| `title` | string |  |
| `folder` | string |  |
| `used` | string |  |

### `bridges`

The bridges as they say they are (Settings shows the same), the pairing ticket aside (`pairing`).

Who: window, cli.

| Field | Type | |
|---|---|---|
| `id` | string | telegram, remote |
| `state` | string | off, connecting, online, failed… |
| `candidate` | object? | a device asking to pair: {id, name} |
| `why` | string? |  |

### `pairing`

The remote bridge's pairing ticket, while pairing is open (`pairing {open}`): whoever has it can ask to pair, so the window's alone.

Who: window.

| Field | Type | |
|---|---|---|
| `id` | string | `remote` |
| `ticket` | string | for the device to scan (`loom:`…) |

### `schedules`

Scheduled prompts (made by agents' tools or in the window).

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string |  |
| `title` | string | the agent's, when it was made |
| `prompt` | string |  |
| `when` | string | as typed: 09:00, weekdays 9:30, every 2h… |
| `next` | string? | the next run |
| `paused` | boolean |  |
| `last_run` | string? |  |
| `folder` | string |  |
| `session` | string? | the conversation it goes on (none: a new one each time) |

### `asks`

What agents ask of the user beyond their conversation, where no window of loom's shows it (loomd): a UI shows them; `ask_done`, `ask_open`.

Who: window, cli, bridge.

| Field | Type | |
|---|---|---|
| `id` | string |  |
| `agent` | string | who asks |
| `title` | string | its title |
| `kind` | string | open (a file to see), edit (a file to fill in), notify |
| `path` | string? | the file |
| `message` | string |  |
| `state` | string | waiting |
| `at` | string | when |

### `bridge/<name>`

A bridge's events: each an item added and taken away at once (a snapshot is empty).

Who: bridge (that one).

| Field | Type | |
|---|---|---|
| `id` | string |  |
| `event` | object | a bridge-protocol event |

## Commands

### `send`

A message to an agent, as typed in its chat. Bridges send no `!` commands.

Who: window, cli, bridge. Result: `{}`, once done.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `text` | string? | the message (text or pictures are needed) |
| `images` | array? | pictures, base64, any format the UI reads |

### `answer`

Answers a question waiting in a conversation; the first answer wins (later ones: conflict). Bridges never answer "Always allow"; the CLI answers nothing (but in a test instance, LOOM_TEST_INSTANCE=1).

Who: window, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `entry` | string | the question's entry id |
| `option` | string? | a permission question: the chosen option's id |
| `content` | object? | a form: what was filled in |
| `skip` | boolean? | a form: skipped |

### `set_mode`

An agent's permission mode. `bypassPermissions` from loom's window only.

Who: window, cli, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `mode` | string | one of the agent's `modes` |

### `suspend`

Suspends an agent (frees its memory; opening it or a message resumes it). A conflict while it works.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |

### `set_awake`

Keeps an agent awake, or lets it be suspended when idle.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `awake` | boolean | never suspended for being idle (true), or allowed |

### `rewind`

A copy of the conversation from just before your message, with it in the input to change; the copy shows in `agents` (this one is kept). With files: the files go back too.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `entry` | string | a message of yours (a `user` entry) |
| `files` | boolean? | the files Claude changed from there on too |

### `set_option`

Changes a session setting of the agent's (the model, the effort…); its `options` say the new one.

Who: window, cli, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `option` | string | one of its `options`' id (model, effort…) |
| `value` | string | one of that option's values |

### `set_draft`

Sets what waits in the agent's input (`draft`), as a client took or sent it.

Who: window, cli, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `text` | string | what its input holds (empty: nothing) |

### `unqueue`

Takes a queued message back into the agent's input (its `draft`).

Who: window, cli, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `index` | integer | which of its `queued` messages |

### `rename`

Renames an agent (its own titles don't replace it then).

Who: window, cli, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `title` | string | its new title |

### `reopen`

Reopens a conversation closed lately, as an agent.

Who: window, cli. Result: `{agent}`.

| Param | Type | |
|---|---|---|
| `session` | string | a `closed` item's id |

### `past_conversations`

Claude Code's past conversations, newest first (200 at most).

Who: window, cli. Result: `{conversations: [{session, title, folder, updated}]}`, once done.

| Param | Type | |
|---|---|---|
| `folder` | string? | only those in it (else all) |

### `resume`

A past conversation picked up as an agent (the one open, if it is).

Who: window, cli. Result: `{agent}`.

| Param | Type | |
|---|---|---|
| `session` | string | a past conversation |
| `folder` | string | its folder |
| `title` | string? |  |

### `schedule_new`

Schedules a prompt for an agent.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | whose prompt |
| `prompt` | string |  |
| `when` | string | 09:00, weekdays 9:30, every 2h, in 20m… |
| `same` | boolean? | in its conversation (true), or a new one each time |

### `pairing`

The remote bridge's pairing: its ticket is in `bridges`.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `open` | boolean? | open (true) or close pairing |

### `pair_confirm`

Lets in the device asking to pair, and closes pairing.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | the device shown to the user (`bridges`' candidate) |

### `schedule_act`

Acts on a scheduled prompt.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `schedules` item |
| `act` | string | run_now, pause (pauses or goes on), remove |

### `stop`

Stops the agent's turn.

Who: window, cli, bridge. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |

### `new_agent`

Starts an agent in a folder.

Who: window, cli. Result: `{agent}`.

| Param | Type | |
|---|---|---|
| `folder` | string | an absolute folder: its project |

### `rename_project`

Renames a project.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `project` | string | the project's id (its folder) |
| `name` | string | its new name (empty: the folder's) |

### `set_collapsed`

Folds a project in the sidebar, or unfolds it (every UI's).

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `project` | string | the project's id (its folder) |
| `collapsed` | boolean | its agents hidden (true) or shown |

### `remove_project`

Takes a project out of the sidebar, closing its agents (they can be reopened). A conflict while any of them works.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `project` | string | the project's id (its folder) |

### `close_agent`

Closes an agent (its conversation resource ends: gone).

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |

### `run_job`

A new job in the project's tray.

Who: window, cli. Result: `{job}`.

| Param | Type | |
|---|---|---|
| `project` | string | the project's id (its folder) |
| `command` | string | a shell command |
| `name` | string? | the job's name (else its program's) |

### `stop_job`

Stops a job.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `job` | string | the job's id |

### `job_input`

Types into a job's terminal.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `job` | string | the job's id |
| `text` | string | keys typed, as the terminal sends them (escapes included) |

### `job_resize`

The size a job's terminal is shown at (one size for every client: the last one told).

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `job` | string | the job's id |
| `rows` | integer |  |
| `cols` | integer |  |

### `restart_job`

Runs a job again (its output starts over).

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `job` | string | the job's id |

### `set_settings`

Changes settings. What may send things off this machine, decides what agents may do or names a program loom runs (bridges, pairing, speech to text, the agent program, the default mode, the editor, the notes vault) only from loom's window.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `changes` | object | settings by their names in `settings`, with new values |

### `search_notes`

Searches the notes vault.

Who: window, cli, bridge:remote. Result: `{results: [{path, title, folder, snippet, modified}]}`.

| Param | Type | |
|---|---|---|
| `query` | string | words, all of them in a note |
| `limit` | integer? | results at most (30; 100 at most) |

### `blob`

Bytes kept by id (pictures), base64.

Who: window, cli, bridge. Result: `{data}`.

| Param | Type | |
|---|---|---|
| `id` | string | a blob id (an entry's `images`) |

### `scribe_answer`

Answers Scribe's question.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |
| `answer` | string | the answer |

### `scribe_decide`

Decides on a change Scribe proposes.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |
| `write` | boolean | OK (true) or not |

### `scribe_undo`

Undoes what Scribe wrote.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |

### `scribe_drop`

Drops an item from Scribe's lists.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |

### `scribe_teach`

Tells Scribe why it was wrong.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |
| `said` | string | why not |

### `scribe_keep_rule`

Keeps a rule Scribe proposes.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |
| `rule` | string | the rule |

### `scribe_forget`

Forgets a rule Scribe learned.

Who: window. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | a `scribe` item |

### `save`

A file for an agent, saved in its folder under `<bridge>-files/` (never over another).

Who: bridge. Result: `{path}`, once done.

| Param | Type | |
|---|---|---|
| `agent` | string | the agent's id |
| `name` | string | the file's name |
| `data` | string | its bytes, base64 |

### `transcribe`

Speech to text with loom's settings (its key never leaves loom).

Who: window, cli, bridge. Result: `{text}`, once done.

| Param | Type | |
|---|---|---|
| `audio` | string | the recording, base64 |
| `name` | string? | its file name |
| `mime` | string? | its type |

### `bridge_state`

What the bridge says of itself (any object), shown in Settings.

Who: bridge. Result: `{}`.

### `bridge`

The bridge protocol, for bridges that still speak it (the remote one, for the phone app already shipped); its events: `bridge/<name>`.

Who: bridge. Result: `{ok} or {error}`, once done.

| Param | Type | |
|---|---|---|
| `method` | string | a request of docs/bridges.md |
| `params` | object? | its parameters |

### `web_open`

Starts the web page's server (this computer only) if it isn't, and a link to it with a one-time token (two minutes). The page is who opened the link: the window's rights, or the CLI's.

Who: window, cli. Result: `{url}`.

| Param | Type | |
|---|---|---|
| `port` | integer? | where to listen on 127.0.0.1 (else any free port), the first time |

### `web_close`

Stops the web page's server; every page let in is let go.

Who: window, cli. Result: `{}`.

### `ask_done`

An ask answered: a file filled in (the agent goes on), or one seen (it goes).

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | an `asks` item |

### `ask_open`

Opens an ask's file in the user's editor (the setting, else the desktop's), on this computer.

Who: window, cli. Result: `{}`.

| Param | Type | |
|---|---|---|
| `id` | string | an `asks` item with a file |

### `notes_open`

loomd: a Notes chat open (an agent working in the vault): its conversation is `conversation/<agent>`.

Who: window, cli, bridge:remote. Result: `{agent}`.

| Param | Type | |
|---|---|---|
| `session` | string? | a past chat (`notes_chats`); none: a new one |

### `notes_close`

loomd: a Notes chat closed (it stays among the past ones).

Who: window, cli, bridge:remote. Result: `{}`.

| Param | Type | |
|---|---|---|
| `agent` | string | the chat's agent |

### `tool`

An agent's loom tools (its MCP server), as the agent loom found the caller to be.

Who: agent. Result: `the tool's answer`.

| Param | Type | |
|---|---|---|
| `name` | string | a loom tool's request (run_job, job_output, open_file, …) |
| `params` | object? | its parameters |
