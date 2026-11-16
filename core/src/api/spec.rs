//! What the core API offers, said once, here: its resources and commands (who may use them,
//! their parameters, results and items), from which `docs/api.md` and `docs/api.schema.json`
//! are generated (the messages' own types from `wire`, by schemars). A test fails when the
//! committed files differ from what this makes (`LOOM_BLESS=1` writes them), and the end-to-end
//! test when a running loom has a command or resource this doesn't list, or the other way.
//!
//! Additive only (research/core-api/00-decisions.md §5): a field, command or resource added is
//! fine; one taken away or changed is a new protocol.

use serde_json::{Map, Value, json};

/// A field of parameters or of an item: (name, JSON type, required, what it is).
pub type Field = (&'static str, &'static str, bool, &'static str);

pub struct Command {
    pub name: &'static str,
    /// Who may: window, cli, agent, bridge (any), bridge:remote, bridge:telegram.
    pub who: &'static [&'static str],
    pub params: &'static [Field],
    pub result: &'static str,
    /// Answered once done (it may take a while), not at once.
    pub later: bool,
    pub about: &'static str,
}

pub struct Resource {
    /// Its name; `<…>` is filled in (`conversation/<agent>`).
    pub name: &'static str,
    pub who: &'static [&'static str],
    pub item: &'static [Field],
    pub about: &'static str,
}

const PEOPLE: &[&str] = &["window", "cli"];
/// Loom's window (the CLI too in a test instance).
const WINDOW: &[&str] = &["window"];
const ALL_BUT_AGENTS: &[&str] = &["window", "cli", "bridge"];

/// Commands only loomd has (the window does these its own way: its Notes view).
pub const DAEMON_ONLY: &[&str] = &["notes_open", "notes_close", "set_looking", "loomd_stop"];

pub const COMMANDS: &[Command] = &[
    Command {
        name: "send",
        who: &["window", "cli", "bridge"],
        params: &[("agent", "string", true, "the agent's id"), ("text", "string", false, "the message (text or pictures are needed)"), ("images", "array", false, "pictures, base64, any format the UI reads")],
        result: "{}",
        later: true,
        about: "A message to an agent, as typed in its chat. Bridges send no `!` commands.",
    },
    Command {
        name: "answer",
        who: &["window", "bridge"],
        params: &[
            ("agent", "string", true, "the agent's id"),
            ("entry", "string", true, "the question's entry id"),
            ("option", "string", false, "a permission question: the chosen option's id"),
            ("content", "object", false, "a form: what was filled in"),
            ("skip", "boolean", false, "a form: skipped"),
        ],
        result: "{}",
        later: false,
        about: "Answers a question waiting in a conversation; the first answer wins (later ones: conflict). Bridges never answer \"Always allow\"; the CLI answers nothing (but in a test instance, LOOM_TEST_INSTANCE=1).",
    },
    Command {
        name: "set_mode",
        who: &["window", "cli", "bridge"],
        params: &[("agent", "string", true, "the agent's id"), ("mode", "string", true, "one of the agent's `modes`")],
        result: "{}",
        later: false,
        about: "An agent's permission mode. `bypassPermissions` from loom's window only.",
    },
    Command { name: "suspend", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Suspends an agent (frees its memory; opening it or a message resumes it). A conflict while it works." },
    Command { name: "set_awake", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("awake", "boolean", true, "never suspended for being idle (true), or allowed")], result: "{}", later: false, about: "Keeps an agent awake, or lets it be suspended when idle." },
    Command { name: "changes", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{files: [{path, mark: created|changed, repo, status: git's two letters, diff: against the last commit}]}", later: true, about: "The files an agent created or changed, with their diffs." },
    Command { name: "revert", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("path", "string", true, "one of its `changes`' files"), ("hunk", "integer", false, "one hunk of that file's diff (from 0); none: the whole file"), ("hunk_text", "string", false, "with `hunk`: that hunk as shown, from its @@ line (a conflict if the file's diff has changed there)")], result: "{}", later: true, about: "Throws away changes to a file the agent touched, back to the last commit: all of them, or one hunk. It can't be undone. A file git doesn't know (new) is not reverted." },
    Command { name: "fork", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "A copy of the whole conversation, as an agent of its own (it shows in `agents`)." },
    Command { name: "clear", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "/clear: the agent starts a fresh conversation (the old one stays among the past ones)." },
    Command { name: "restart", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Stops the agent's process and starts it again, resuming its conversation." },
    Command { name: "add_folder", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("folder", "string", true, "an absolute folder")], result: "{}", later: false, about: "Lets the agent work in another folder too (it restarts)." },
    Command { name: "export", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{markdown}", later: false, about: "The conversation as Markdown: what was said, by whom." },
    Command { name: "move_agent", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("by", "integer", true, "-1 up, 1 down, among its project's agents")], result: "{}", later: false, about: "Moves an agent in the sidebar's order." },
    Command { name: "move_project", who: PEOPLE, params: &[("project", "string", true, "the project's id (its folder)"), ("by", "integer", true, "-1 up, 1 down")], result: "{}", later: false, about: "Moves a project in the sidebar's order." },
    Command { name: "mark_seen", who: ALL_BUT_AGENTS, params: &[("agent", "string", false, "the agent's id"), ("all", "boolean", false, "every agent (Mark all seen)")], result: "{}", later: false, about: "The user has seen what is new in an agent (or in all): `unseen` goes." },
    Command { name: "background", who: ALL_BUT_AGENTS, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Ctrl+B D: what its running turn runs goes on in the background (a notice says so if the agent program can't)." },
    Command { name: "send_now", who: ALL_BUT_AGENTS, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Ctrl+Enter: its queued messages go into the running turn now (or as the next prompt)." },
    Command { name: "stop_workflow", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("workflow", "string", true, "a `workflows/<agent>` item's id")], result: "{}", later: false, about: "Stops a workflow the agent runs." },
    Command { name: "stop_subagent", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("subagent", "string", true, "a `crew/<agent>` item's id")], result: "{}", later: false, about: "Stops one subagent (or background task) of the agent's, the rest going on." },
    Command { name: "mcp_server", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("server", "string", true, "a tool server's name (`mcp`)"), ("act", "string", true, "reconnect, on or off")], result: "{}", later: false, about: "Connects one of the agent's tool servers (MCP) again, or switches it on or off; `mcp` shows how it went." },
    Command { name: "reload_extensions", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "The agent's session reads its skills and plugins again (changed in loom's Claude folder)." },
    Command { name: "press", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("entry", "string", true, "a notice with a `button`")], result: "{}", later: false, about: "Presses a notice's button (\"Rewind\" with the files…). A choice's button goes once pressed." },
    Command { name: "rewind", who: PEOPLE, params: &[("agent", "string", true, "the agent's id"), ("entry", "string", true, "a message of yours (a `user` entry)"), ("files", "boolean", false, "the files Claude changed from there on too")], result: "{}", later: false, about: "A copy of the conversation from just before your message, with it in the input to change; the copy shows in `agents` (this one is kept). With files: the files go back too." },
    Command { name: "set_option", who: ALL_BUT_AGENTS, params: &[("agent", "string", true, "the agent's id"), ("option", "string", true, "one of its `options`' id (model, effort…)"), ("value", "string", true, "one of that option's values")], result: "{}", later: false, about: "Changes a session setting of the agent's (the model, the effort…); its `options` say the new one." },
    Command { name: "set_draft", who: ALL_BUT_AGENTS, params: &[("agent", "string", true, "the agent's id"), ("text", "string", true, "what its input holds (empty: nothing)")], result: "{}", later: false, about: "Sets what waits in the agent's input (`inputs`), as a client took or sent it." },
    Command { name: "unqueue", who: ALL_BUT_AGENTS, params: &[("agent", "string", true, "the agent's id"), ("index", "integer", true, "which of its queued messages (`inputs`)")], result: "{}", later: false, about: "Takes a queued message back into the agent's input (`inputs`)." },
    Command { name: "rename", who: &["window", "cli", "bridge"], params: &[("agent", "string", true, "the agent's id"), ("title", "string", true, "its new title")], result: "{}", later: false, about: "Renames an agent (its own titles don't replace it then)." },
    Command { name: "reopen", who: PEOPLE, params: &[("session", "string", true, "a `closed` item's id")], result: "{agent}", later: false, about: "Reopens a conversation closed lately, as an agent." },
    Command {
        name: "past_conversations",
        who: PEOPLE,
        params: &[("folder", "string", false, "only those in it (else all)")],
        result: "{conversations: [{session, title, folder, updated}]}",
        later: true,
        about: "Claude Code's past conversations, newest first (200 at most).",
    },
    Command {
        name: "resume",
        who: PEOPLE,
        params: &[("session", "string", true, "a past conversation"), ("folder", "string", true, "its folder"), ("title", "string", false, "")],
        result: "{agent}",
        later: false,
        about: "A past conversation picked up as an agent (the one open, if it is).",
    },
    Command {
        name: "schedule_new",
        who: PEOPLE,
        params: &[("agent", "string", true, "whose prompt"), ("prompt", "string", true, ""), ("when", "string", true, "09:00, weekdays 9:30, every 2h, in 20m…"), ("same", "boolean", false, "in its conversation (true), or a new one each time")],
        result: "{}",
        later: false,
        about: "Schedules a prompt for an agent.",
    },
    Command { name: "pairing", who: WINDOW, params: &[("open", "boolean", false, "open (true) or close pairing")], result: "{}", later: false, about: "The remote bridge's pairing: its ticket is in `bridges`." },
    Command { name: "pair_confirm", who: WINDOW, params: &[("id", "string", true, "the device shown to the user (`bridges`' candidate)")], result: "{}", later: false, about: "Lets in the device asking to pair, and closes pairing." },
    Command {
        name: "schedule_act",
        who: PEOPLE,
        params: &[("id", "string", true, "a `schedules` item"), ("act", "string", true, "run_now, pause (pauses or goes on), remove")],
        result: "{}",
        later: false,
        about: "Acts on a scheduled prompt.",
    },
    Command { name: "stop", who: &["window", "cli", "bridge"], params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Stops the agent's turn." },
    Command { name: "loomd_stop", who: WINDOW, params: &[], result: "{}", later: false, about: "loomd stops, as on SIGTERM: agents stopped (their conversations resume next time), the workspace saved. The window restarts an older loomd with it." },
    Command {
        name: "set_looking",
        who: WINDOW,
        params: &[("active", "boolean", true, "the window is in use"), ("agents", "array", true, "the agents' ids in view")],
        result: "{}",
        later: false,
        about: "What loom's window shows (loomd: no desktop notice about those agents, none of a question while the window is in use). Said again within a minute, else forgotten.",
    },
    Command { name: "new_agent", who: PEOPLE, params: &[("folder", "string", true, "an absolute folder: its project")], result: "{agent}", later: false, about: "Starts an agent in a folder." },
    Command {
        name: "new_worktree_agent",
        who: PEOPLE,
        params: &[("project", "string", true, "the project's id (its folder)"), ("branch", "string", true, "the new branch's name")],
        result: "{agent}",
        later: true,
        about: "Starts an agent in a worktree of its own: a new branch from what the project has checked out, in <parent>/<project>.worktrees/<branch>. Its agents stay on the project's card.",
    },
    Command { name: "browser_open", who: PEOPLE, params: &[("project", "string", true, "the project's id (its folder)")], result: "its `browsers` item", later: false, about: "Starts the project's browser if it doesn't run (agents start it with their loom_browser tool): headed on a screen of its own when Xvfb is installed, with loom's profile for the project. Keeps it from stopping unused." },
    Command { name: "browser_stop", who: PEOPLE, params: &[("project", "string", true, "the project's id (its folder)")], result: "{}", later: false, about: "Stops the project's browser (its profile, and so its logins, stay)." },
    Command { name: "embeddings_check", who: PEOPLE, params: &[], result: "the `embeddings` item", later: true, about: "Checks meaning search again (Settings' Refresh): starts Ollama when nothing answers on this machine, downloads the model when it is missing." },
    Command { name: "worktree_merge", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{said}", later: true, about: "Merges the branch of an agent's worktree into what its project has checked out (a merge commit). Refused while it has changes not committed; a conflict is undone." },
    Command { name: "worktree_remove", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{said}", later: true, about: "Closes an agent and removes its worktree, and its branch once merged (an unmerged branch is kept). Refused while it works or has changes not committed." },
    Command { name: "rename_project", who: PEOPLE, params: &[("project", "string", true, "the project's id (its folder)"), ("name", "string", true, "its new name (empty: the folder's)")], result: "{}", later: false, about: "Renames a project." },
    Command { name: "set_collapsed", who: PEOPLE, params: &[("project", "string", true, "the project's id (its folder)"), ("collapsed", "boolean", true, "its agents hidden (true) or shown")], result: "{}", later: false, about: "Folds a project in the sidebar, or unfolds it (every UI's)." },
    Command { name: "remove_project", who: PEOPLE, params: &[("project", "string", true, "the project's id (its folder)")], result: "{}", later: false, about: "Takes a project out of the sidebar, closing its agents (they can be reopened). A conflict while any of them works." },
    Command { name: "close_agent", who: PEOPLE, params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Closes an agent (its conversation resource ends: gone)." },
    Command {
        name: "run_job",
        who: PEOPLE,
        params: &[("project", "string", true, "the project's id (its folder)"), ("command", "string", true, "a shell command"), ("name", "string", false, "the job's name (else its program's)")],
        result: "{job}",
        later: false,
        about: "A new job in the project's tray.",
    },
    Command { name: "stop_job", who: PEOPLE, params: &[("job", "string", true, "the job's id")], result: "{}", later: false, about: "Stops a job." },
    Command { name: "job_text", who: PEOPLE, params: &[("job", "string", true, "the job's id"), ("lines", "integer", false, "its last lines (2000; 20000 tops)")], result: "{text}", later: false, about: "A job's output as text (to copy)." },
    Command { name: "remove_job", who: PEOPLE, params: &[("job", "string", true, "an exited job's id")], result: "{}", later: false, about: "Takes a job that has exited out of its tray (a conflict while it runs)." },
    Command { name: "job_input", who: PEOPLE, params: &[("job", "string", true, "the job's id"), ("text", "string", true, "keys typed, as the terminal sends them (escapes included)")], result: "{}", later: false, about: "Types into a job's terminal." },
    Command { name: "job_resize", who: PEOPLE, params: &[("job", "string", true, "the job's id"), ("rows", "integer", true, ""), ("cols", "integer", true, "")], result: "{}", later: false, about: "The size a job's terminal is shown at (one size for every client: the last one told)." },
    Command { name: "restart_job", who: PEOPLE, params: &[("job", "string", true, "the job's id")], result: "{}", later: false, about: "Runs a job again (its output starts over)." },
    Command {
        name: "set_settings",
        who: PEOPLE,
        params: &[("changes", "object", true, "settings by their names in `settings`, with new values")],
        result: "{}",
        later: false,
        about: "Changes settings. What may send things off this machine, decides what agents may do or names a program loom runs (bridges, pairing, speech to text, the agent program, the default mode, the editor, the notes vault) only from loom's window.",
    },
    Command {
        name: "search_notes",
        who: &["window", "cli", "bridge:remote"],
        params: &[("query", "string", true, "words, all of them in a note"), ("limit", "integer", false, "results at most (30; 100 at most)")],
        result: "{results: [{path, title, folder, snippet, modified}]}",
        later: false,
        about: "Searches the notes vault.",
    },
    Command { name: "blob", who: ALL_BUT_AGENTS, params: &[("id", "string", true, "a blob id (an entry's `images`)")], result: "{data}", later: false, about: "Bytes kept by id (pictures), base64." },
    Command { name: "scribe_answer", who: WINDOW, params: &[("id", "string", true, "a `scribe` item"), ("answer", "string", true, "the answer")], result: "{}", later: false, about: "Answers Scribe's question." },
    Command { name: "scribe_decide", who: WINDOW, params: &[("id", "string", true, "a `scribe` item"), ("write", "boolean", true, "OK (true) or not")], result: "{}", later: false, about: "Decides on a change Scribe proposes." },
    Command { name: "scribe_undo", who: WINDOW, params: &[("id", "string", true, "a `scribe` item")], result: "{}", later: false, about: "Undoes what Scribe wrote." },
    Command { name: "scribe_drop", who: WINDOW, params: &[("id", "string", true, "a `scribe` item")], result: "{}", later: false, about: "Drops an item from Scribe's lists." },
    Command { name: "scribe_fix", who: WINDOW, params: &[("id", "string", true, "a `scribe` change it wrote"), ("said", "string", true, "what's wrong or missing (\"it's net, not gross\")")], result: "{}", later: false, about: "Scribe changes a note it wrote as you say." },
    Command { name: "scribe_teach", who: WINDOW, params: &[("id", "string", true, "a `scribe` item"), ("said", "string", true, "why not")], result: "{}", later: false, about: "Tells Scribe why it was wrong." },
    Command { name: "scribe_keep_rule", who: WINDOW, params: &[("id", "string", true, "a `scribe` item"), ("rule", "string", true, "the rule")], result: "{}", later: false, about: "Keeps a rule Scribe proposes." },
    Command { name: "scribe_history", who: WINDOW, params: &[("limit", "integer", false, "200; 2000 tops")], result: "{changes: [{id, kind: written|dropped, topic, reason, change, title, at, undone}]}", later: false, about: "Everything Scribe wrote or was told no, newest first (`scribe_undo` takes a written one back)." },
    Command { name: "scribe_forget", who: WINDOW, params: &[("id", "string", true, "a `scribe` item")], result: "{}", later: false, about: "Forgets a rule Scribe learned." },
    Command { name: "scribe_seen", who: WINDOW, params: &[("id", "string", false, "a `change` item of `scribe` (none: all)")], result: "{}", later: false, about: "A change Scribe wrote, seen: shown folded from then on." },
    Command { name: "scribe_check", who: WINDOW, params: &[("session", "string", true, "the conversation")], result: "{}", later: false, about: "Scribe reads what is left unread of a conversation, now (\"Check now\")." },
    Command {
        name: "save",
        who: ALL_BUT_AGENTS,
        params: &[("agent", "string", true, "the agent's id"), ("name", "string", true, "the file's name"), ("data", "string", true, "its bytes, base64")],
        result: "{path}",
        later: true,
        about: "A file for an agent, saved in its folder under `<bridge>-files/` (a bridge's) or `web-files/` (dropped in a web page), never over another.",
    },
    Command {
        name: "transcribe",
        who: ALL_BUT_AGENTS,
        params: &[("audio", "string", true, "the recording, base64"), ("name", "string", false, "its file name"), ("mime", "string", false, "its type")],
        result: "{text}",
        later: true,
        about: "Speech to text with loom's settings (its key never leaves loom).",
    },
    Command { name: "bridge_state", who: &["bridge"], params: &[], result: "{}", later: false, about: "What the bridge says of itself (any object), shown in Settings." },
    Command {
        name: "bridge",
        who: &["bridge"],
        params: &[("method", "string", true, "a request of docs/bridges.md"), ("params", "object", false, "its parameters")],
        result: "{ok} or {error}",
        later: true,
        about: "The bridge protocol, for bridges that still speak it (the remote one, for the phone app already shipped); its events: `bridge/<name>`.",
    },
    Command {
        name: "web_open",
        who: PEOPLE,
        params: &[("port", "integer", false, "where to listen on 127.0.0.1 (else any free port), the first time")],
        result: "{url}",
        later: false,
        about: "Starts the web page's server (this computer only) if it isn't, and a link to it with a one-time token (two minutes). The page is who opened the link: the window's rights, or the CLI's.",
    },
    Command { name: "web_close", who: PEOPLE, params: &[], result: "{}", later: false, about: "Stops the web page's server; every page let in is let go." },
    Command { name: "topics", who: PEOPLE, params: &[], result: "{vault, topics: [{name, path, folder, listens_for, apply: ask|auto, sync, enabled}]}", later: false, about: "The notes vault's topics (Notes on, with a vault)." },
    Command { name: "topic_set", who: PEOPLE, params: &[("name", "string", true, "a topic's name"), ("enabled", "boolean", true, "")], result: "{}", later: false, about: "Switches a topic on or off." },
    Command { name: "topic_new", who: PEOPLE, params: &[("name", "string", true, "its name"), ("about", "string", false, "what it keeps, in a sentence: Claude drafts its file from it")], result: "{path}", later: true, about: "A new topic, its file from loom's template or drafted from `about`." },
    Command { name: "claude_items", who: PEOPLE, params: &[], result: "{folder, items: [{kind: skill|agent|command, name, description, enabled, shipped, path}]}", later: false, about: "The skills, agents and commands only loom's agents load (its Claude folder)." },
    Command { name: "claude_set", who: PEOPLE, params: &[("kind", "string", true, "skill, agent or command"), ("name", "string", true, ""), ("enabled", "boolean", true, "")], result: "{}", later: false, about: "Switches one on or off (agents started or woken from now on)." },
    Command { name: "claude_new", who: PEOPLE, params: &[("kind", "string", true, "skill, agent or command"), ("name", "string", true, "")], result: "{path}", later: false, about: "A new one, from loom's template." },
    Command { name: "claude_restore", who: PEOPLE, params: &[("name", "string", true, "one loom ships")], result: "{}", later: false, about: "Puts back loom's own version of one it ships." },
    Command { name: "delete_conversation", who: PEOPLE, params: &[("session", "string", true, "a past conversation (`past_conversations`)")], result: "{}", later: true, about: "Deletes a past conversation (Claude Code's file of it). It can't be undone." },
    Command { name: "dirs_list", who: PEOPLE, params: &[("folder", "string", false, "an absolute folder (none: home)")], result: "{folder, parent, dirs: [name]}", later: false, about: "A folder's folders (hidden ones left out), to pick a project's folder." },
    Command { name: "edit_file", who: PEOPLE, params: &[("path", "string", true, "a file in a project, a topic's, or one of `claude_items`'")], result: "{}", later: false, about: "Opens the file in the user's editor (the setting, else the desktop's), on this computer." },
    Command { name: "files_list", who: PEOPLE, params: &[("folder", "string", true, "a folder in a project (absolute)")], result: "{entries: [{name, path, dir, size, modified}]}", later: true, about: "A folder's entries, folders first (.git left out)." },
    Command { name: "files_find", who: PEOPLE, params: &[("project", "string", true, "a project's folder"), ("query", "string", true, "letters of the path, in order"), ("limit", "integer", false, "at most (50; 500 tops)")], result: "{files: [path relative to the project]}", later: true, about: "Finds a project's files by name (Ctrl+P), best first." },
    Command { name: "file_read", who: PEOPLE, params: &[("path", "string", true, "a file in a project"), ("bytes", "boolean", false, "its bytes instead ({data}: base64, 10 MB at most)")], result: "{text, size, modified} or {data, size}", later: true, about: "A file's text (null: not text, or over 2 MB) and when it changed (ms), for `file_write`." },
    Command { name: "file_write", who: PEOPLE, params: &[("path", "string", true, "a file in a project (new or not)"), ("text", "string", true, ""), ("modified", "integer", false, "when it changed as read: a conflict if it changed since")], result: "{modified}", later: true, about: "Saves a file." },
    Command { name: "git_repos", who: PEOPLE, params: &[("project", "string", true, "a project's folder")], result: "{repos: [{path, name, branch, ahead, behind, changes: [{code, file}]}]}", later: true, about: "The project's git repositories and what changed in each (git's two letters: M  staged, ` M` not yet, ?? new)." },
    Command { name: "git_diff", who: PEOPLE, params: &[("repo", "string", true, "one of `git_repos`'"), ("file", "string", true, "relative to it"), ("staged", "boolean", false, "the staged changes (else the ones not yet)")], result: "{diff}", later: true, about: "A file's diff." },
    Command { name: "git_do", who: PEOPLE, params: &[("repo", "string", true, ""), ("act", "string", true, "stage, unstage, discard (back to the last commit: it can't be undone), commit"), ("files", "array", false, "relative paths (not for commit)"), ("message", "string", false, "commit's")], result: "{}", later: true, about: "Stages, unstages or throws away files' changes, or commits what is staged." },
    Command { name: "open_path", who: PEOPLE, params: &[("path", "string", true, "a file or folder in a project"), ("how", "string", false, "app (its own app: the default), folder (its folder in the file manager)")], result: "{}", later: true, about: "Opens it on this computer (xdg-open)." },
    Command { name: "trash_file", who: PEOPLE, params: &[("path", "string", true, "a file or folder in a project (not the project)")], result: "{}", later: true, about: "Moves it to the desktop's trash (gio trash)." },
    Command { name: "git_show", who: PEOPLE, params: &[("repo", "string", true, ""), ("hash", "string", true, "a commit's (`git_log`)")], result: "{text}", later: true, about: "One commit: who, when, its message, the files it changed and its diff (git show)." },
    Command { name: "git_log", who: PEOPLE, params: &[("repo", "string", true, ""), ("limit", "integer", false, "200; 2000 tops"), ("graph", "boolean", false, "every branch, with the graph drawn before each line"), ("all", "boolean", false, "every branch (no graph drawn)"), ("skip", "integer", false, "the newest ones left out: the next page")], result: "{commits: [{graph, hash, refs, subject, author, when, parents}]}", later: true, about: "The newest commits." },
    Command { name: "board", who: PEOPLE, params: &[("project", "string", true, "a project's folder")], result: "{project, file, lanes: [{title, cards: [{id, title, body, done, created, started, finished, by}]}]}", later: false, about: "A project's board, `.loom/board.md` in its folder (the Obsidian Kanban plugin's format): lanes and their cards, dates local to the minute. Agents use their loom tools for their own project's." },
    Command { name: "board_add", who: PEOPLE, params: &[("project", "string", true, ""), ("title", "string", true, ""), ("body", "string", false, "its description"), ("lane", "string", false, "a lane's title (none: the first)")], result: "{card}", later: false, about: "A new card at the end of a lane." },
    Command { name: "board_move", who: PEOPLE, params: &[("project", "string", true, ""), ("card", "integer", true, ""), ("lane", "string", true, ""), ("before", "integer", false, "the card it goes before (none: the end)")], result: "{}", later: false, about: "A card to a lane: started when it enters Doing, done (checked) when it enters Done." },
    Command { name: "board_edit", who: PEOPLE, params: &[("project", "string", true, ""), ("card", "integer", true, ""), ("title", "string", false, ""), ("body", "string", false, "its whole description"), ("note", "string", false, "added below its description")], result: "{}", later: false, about: "A card's title or description changed." },
    Command {
        name: "board_work",
        who: PEOPLE,
        params: &[("project", "string", true, ""), ("card", "integer", true, ""), ("agent", "string", false, "one of the project's agents to give it to (none: a new agent)")],
        result: "{agent}",
        later: false,
        about: "Puts an agent on a card: a new one, or `agent` (after its turn, if it works). The card's title and description are its message, the card goes to Doing, and the agent's work is logged on it.",
    },
    Command { name: "board_archive", who: PEOPLE, params: &[("project", "string", true, "")], result: "{archived}", later: false, about: "Done's cards moved to .loom/board-archive.md (a board of its own, newest first); new cards never take their ids." },
    Command { name: "board_remove", who: PEOPLE, params: &[("project", "string", true, ""), ("card", "integer", true, "")], result: "{}", later: false, about: "A card taken off the board." },
    Command { name: "ask_done", who: PEOPLE, params: &[("id", "string", true, "an `asks` item")], result: "{}", later: false, about: "An ask answered: a file filled in (the agent goes on), or one seen (it goes)." },
    Command { name: "ask_cancel", who: PEOPLE, params: &[("id", "string", true, "an `edit` item of `asks`")], result: "{}", later: false, about: "A file the user won't fill in: the agent hears they cancelled; the ask goes." },
    Command { name: "ask_open", who: PEOPLE, params: &[("id", "string", true, "an `asks` item with a file")], result: "{}", later: false, about: "Opens an ask's file in the user's editor (the setting, else the desktop's), on this computer." },
    Command { name: "notes_open", who: &["window", "cli", "bridge:remote"], params: &[("session", "string", false, "a past chat (`notes_chats`); none: a new one"), ("topic", "string", false, "a new chat about this topic (`topics`), its notes in reach")], result: "{agent}", later: false, about: "loomd: a Notes chat open (an agent working in the vault): its conversation is `conversation/<agent>`." },
    Command { name: "notes_close", who: &["window", "cli", "bridge:remote"], params: &[("agent", "string", true, "the chat's agent")], result: "{}", later: false, about: "loomd: a Notes chat closed (it stays among the past ones)." },
    Command {
        name: "tool",
        who: &["agent"],
        params: &[("name", "string", true, "a loom tool's request (run_job, job_output, open_file, …)"), ("params", "object", false, "its parameters")],
        result: "the tool's answer",
        later: false,
        about: "An agent's loom tools (its MCP server), as the agent loom found the caller to be.",
    },
];

pub const RESOURCES: &[Resource] = &[
    Resource {
        name: "agents",
        who: &["window", "cli", "bridge", "agent"],
        item: &[
            ("id", "string", true, "the agent's id"),
            ("title", "string", true, ""),
            ("project", "string", true, "its project's name"),
            ("project_id", "string", true, "its project's id (folder)"),
            ("cwd", "string", true, "the folder it works in (its project's, or a worktree of it)"),
            ("status", "string", true, "idle, working, asking, suspended, …"),
            ("unseen", "boolean", true, "something new the user hasn't seen"),
            ("busy", "boolean", true, "a turn under way"),
            ("questions", "integer", true, "questions waiting"),
            ("used", "string", true, "last used (RFC 3339)"),
            ("place", "string", true, "where in the project it works (a repo, a folder)"),
            ("edited", "integer", true, "files it changed"),
            ("awake", "boolean", true, "kept from resting"),
            ("options", "array", false, "the session's settings (configOptions: {id, name, category, currentValue, options: [{value, name} or groups]}): model, effort…; `set_option`"),
            ("chosen", "array", false, "the ids of the settings among `options` the agent has a choice of its own for (kept for it, applied whenever it starts); the rest follow the settings' defaults"),
            ("stalled", "boolean", false, "a turn quiet for 3 minutes, no tool running"),
            ("looking", "boolean", false, "the user has it in front of them in loom"),
            ("mode", "string", false, "its permission mode"),
            ("modes", "array", false, "the modes it can take (`set_mode`)"),
            ("last_turn", "object", false, "{turns, seconds, error, cancelled}: `turns` grows as each ends"),
            ("failed", "object", false, "{times, why}: its process failing"),
            ("unsent", "object", false, "{times, count}: queued messages that never went out"),
        ],
        about: "The open agents, in the sidebar's order (Notes chats not among them).",
    },
    Resource { name: "projects", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "its folder"), ("name", "string", true, ""), ("collapsed", "boolean", true, "folded in the sidebar")], about: "The projects, in the sidebar's order." },
    Resource {
        name: "conversation/<agent>",
        who: &["window", "cli", "bridge", "agent (its own)"],
        item: &[
            ("id", "string", true, "e1, e2…"),
            ("kind", "string", true, "user, agent, thought, tool, question, notice, turn…; an unknown one is drawn plainly"),
            ("status", "string", true, "pending, streaming, awaiting_user, done, failed, cancelled"),
            ("turn_id", "string", false, "t1, t2…: the turn it belongs to"),
            ("text", "string", false, "text; streamed as `append` ops"),
            ("message", "string", false, "the agent's id for it (rewind, forks)"),
            ("button", "string", false, "a notice's button, to `press` (gone once a choice is pressed)"),
            ("images", "array", false, "pictures: blob ids (`blob`)"),
            ("call", "string", false, "a tool's call id"),
            ("title", "string", false, "a tool's title"),
            ("tool", "string", false, "a tool's kind"),
            ("input", "object", false, "a tool's input"),
            ("content", "array", false, "a tool's content (diffs…)"),
            ("locations", "array", false, "files a tool touched"),
            ("output", "string", false, "a tool's output (its last 16,000 characters)"),
            ("exit", "integer", false, "a command's exit code"),
            ("opened", "integer", false, "grows each time it is to be shown open (a `!command`'s output)"),
            ("parent", "string", false, "the task a tool is part of"),
            ("detail", "string", false, "a question's detail"),
            ("markdown", "boolean", false, "a question's detail is Markdown (a plan to approve)"),
            ("options", "array", false, "a permission question's options: {id, name, kind}"),
            ("form", "object", false, "a form question's JSON Schema"),
            ("answer", "string", false, "how a question was answered"),
            ("seconds", "integer", false, "a turn's length"),
        ],
        about: "An agent's conversation: Agent → turn → entry. Questions are entries `awaiting_user` until answered (`answer`).",
    },
    Resource {
        name: "usage",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, "`account`, or an agent's id"), ("kind", "string", true, "account or agent"), ("plan", "string", false, "the account's plan"), ("windows", "array", false, "limits: {kind, utilization, resets_at, seen_at}"), ("used", "integer", false, "an agent's context used"), ("size", "integer", false, "its context's size"), ("cost", "object", false, "{amount, currency}"), ("limit", "object", false, "a spending limit"), ("parts", "array", false, "what fills an agent's context: {name, tokens}"), ("compacts_at", "integer", false, "the tokens it compacts at")],
        about: "The account's limits and each agent's context and cost.",
    },
    Resource {
        name: "jobs",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, ""), ("project_id", "string", true, ""), ("name", "string", true, ""), ("command", "string", true, ""), ("status", "string", true, "running, restarting, exited"), ("exit_code", "integer", false, "")],
        about: "Every project's jobs, oldest first.",
    },
    Resource {
        name: "job/<id>",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, "`output`"), ("rows", "integer", true, ""), ("cols", "integer", true, ""), ("screen", "string", true, "base64 terminal bytes drawing its screen"), ("offset", "integer", true, "the stream's byte the screen is at")],
        about: "A job's terminal: its screen, then `bytes` ops at their offsets (drop what the screen holds; past it, resync). A new run: an `upsert`.",
    },
    Resource {
        name: "embeddings",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, "`status`"), ("state", "string", true, "off, checking, pulling, ready, failed"), ("detail", "string", true, "a sentence for people")],
        about: "How meaning search stands (Settings' embed_code and embed_notes): Ollama and its model.",
    },
    Resource {
        name: "browsers",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, "the project's folder"), ("port", "integer", true, "its debugging port (CDP, on 127.0.0.1): a view screencasts the page from it"), ("headed", "boolean", true, "on a screen of its own (Xvfb), not headless"), ("name", "string", true, "the browser: Helium, Chromium…"), ("shown", "integer", true, "times an agent asked the user to look at it (a login): a UI brings it in front when it grows"), ("devtools", "boolean", true, "agents drive it with Chrome's DevTools MCP, not agent-browser")],
        about: "The projects' browsers that run (browser.rs), one item each.",
    },
    Resource { name: "settings", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "`settings`")], about: "One item: every setting by its snake_case name; secrets only as `<name>_set`." },
    Resource {
        name: "vault/<folder>",
        who: &["window", "cli", "bridge:remote"],
        item: &[("id", "string", true, "its path"), ("name", "string", true, ""), ("path", "string", true, ""), ("kind", "string", true, "folder, note, file, error"), ("count", "integer", false, "a folder's entries"), ("title", "string", false, "a note's"), ("size", "integer", false, ""), ("modified", "integer", false, "")],
        about: "A folder of the notes vault (`vault/`: the top), folders first; it follows the files.",
    },
    Resource {
        name: "note/<path>",
        who: &["window", "cli", "bridge:remote"],
        item: &[("id", "string", true, "`note`, or `error`"), ("path", "string", false, ""), ("text", "string", false, ""), ("title", "string", false, ""), ("tags", "array", false, ""), ("links", "array", false, ""), ("backlinks", "array", false, ""), ("modified", "integer", false, ""), ("message", "string", false, "an error's")],
        about: "One note (or file), as read; it follows the file.",
    },
    Resource { name: "notes_chats", who: &["window", "cli", "bridge:remote"], item: &[("id", "string", true, "its session"), ("title", "string", true, ""), ("updated", "string", false, ""), ("mark", "string", false, ""), ("current", "boolean", false, "the one open")], about: "loom's Notes chats." },
    Resource { name: "scribe", who: PEOPLE, item: &[("id", "string", true, "`status`, or `<list>:<n>`"), ("list", "string", false, "questions, proposals, recent, teach"), ("doing", "string", false, "status: what it is doing"), ("running", "boolean", false, ""), ("calls_left", "integer", false, "")], about: "Scribe: what it does, asks, proposes and wrote." },
    Resource { name: "subagent/<agent>/<subagent>", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "as `conversation/<agent>`'s")], about: "A subagent's own conversation (a `crew/<agent>` item's id; a tool entry's `subagent`), entries as an agent's." },
    Resource { name: "crew/<agent>", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "the subagent's session, or its task's tool call"), ("name", "string", true, ""), ("step", "string", true, "what it did last"), ("state", "string", false, "running, requires_action, idle"), ("started", "string", true, ""), ("at", "string", true, "when it last did something"), ("ended", "string", false, ""), ("guessed", "boolean", true, "no session of its own: its state is a guess from its steps")], about: "An agent's subagents (tasks it runs), while its turn goes on." },
    Resource { name: "overview", who: PEOPLE, item: &[("id", "string", true, "an agent's id"), ("waiting", "array", true, "the titles of its questions waiting"), ("questions", "array", true, "those questions: {entry, title, options: [{id, name, kind}], form} (`answer`; a form in its conversation)"), ("reply", "string", false, "the start of its last reply (240 characters, one line)"), ("session", "string", false, "its conversation (a schedule's `session`)")], about: "What the overview's cards show beside `agents`: people's alone." },
    Resource { name: "plan/<agent>", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "its place, from 0"), ("content", "string", true, "the step"), ("status", "string", true, "pending, in_progress, completed"), ("priority", "string", false, "")], about: "Claude's plan for the work (its to-do list), as it last said it." },
    Resource { name: "workflows/<agent>", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "its tool call"), ("name", "string", true, ""), ("summary", "string", false, ""), ("running", "boolean", true, ""), ("state", "string", true, "running, failed, stopped, ended, or what it said when done"), ("done", "integer", true, "agents done"), ("agents", "integer", true, "agents in all"), ("members", "array", true, "{label, phase, done}"), ("task", "string", true, "its task's id"), ("dir", "string", true, "the folder of its agents' transcripts (on loomd's computer)"), ("unanswered", "integer", true, "its stops that went unanswered: Stop is offered again when it grows")], about: "The workflows an agent launched, followed from their files (`stop_workflow`)." },
    Resource { name: "mcp", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "an agent's id"), ("servers", "array", true, "{name, status, error}")], about: "Each agent's MCP servers, as its process last said." },
    Resource { name: "uris_shown", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "an agent's id"), ("uri", "string", true, "a page an MCP server asked to open, the user accepting"), ("n", "integer", true, "grows each time")], about: "A web page an agent's MCP server asked the user to open (its sign-in): a client opens it." },
    Resource { name: "jobs_shown", who: ALL_BUT_AGENTS, item: &[("id", "string", true, "an agent's id"), ("job", "string", true, "the job it opened for the user (a login)"), ("n", "integer", true, "grows each time")], about: "A job an agent brings forward for the user: a client opens it." },
    Resource { name: "inputs", who: PEOPLE, item: &[("id", "string", true, "an agent's id"), ("draft", "string", true, "what waits in its input (a rewind's message, one taken back)"), ("draft_images", "array", true, "the pictures of the messages taken back (blob ids, `blob`): attached again with the draft"), ("queued", "array", true, "its messages waiting for the turn to end, in order (`unqueue`)"), ("commands", "array", true, "its slash commands: {name, about}"), ("suggestion", "string", false, "the next message it suggests (Tab takes it)")], about: "What waits in each agent's input: the user's own words, so people's alone (not in `agents`, which agents and bridges read)." },
    Resource { name: "closed", who: PEOPLE, item: &[("id", "string", true, "its session"), ("title", "string", true, ""), ("folder", "string", true, ""), ("used", "string", true, "")], about: "Conversations closed lately, newest first (`reopen`)." },
    Resource {
        name: "bridges",
        who: PEOPLE,
        item: &[("id", "string", true, "telegram, remote"), ("state", "string", true, "off, connecting, online, failed…"), ("candidate", "object", false, "a device asking to pair: {id, name}"), ("why", "string", false, ""), ("node", "string", false, "remote: this computer's id")],
        about: "The bridges as they say they are (Settings shows the same), the pairing ticket aside (`pairing`).",
    },
    Resource { name: "pairing", who: WINDOW, item: &[("id", "string", true, "`remote`"), ("ticket", "string", true, "for the device to scan (`loom:`…)")], about: "The remote bridge's pairing ticket, while pairing is open (`pairing {open}`): whoever has it can ask to pair, so the window's alone." },
    Resource {
        name: "schedules",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, ""), ("title", "string", true, "the agent's, when it was made"), ("prompt", "string", true, ""), ("when", "string", true, "as typed: 09:00, weekdays 9:30, every 2h…"), ("next", "string", false, "the next run"), ("paused", "boolean", true, ""), ("last_run", "string", false, ""), ("folder", "string", true, ""), ("session", "string", false, "the conversation it goes on (none: a new one each time)")],
        about: "Scheduled prompts (made by agents' tools or in the window).",
    },
    Resource {
        name: "asks",
        who: ALL_BUT_AGENTS,
        item: &[("id", "string", true, ""), ("agent", "string", true, "who asks"), ("title", "string", true, "its title"), ("kind", "string", true, "open (a file to see), edit (a file to fill in), notify, input (a job waits for the user to type in its terminal: a password, a y/n), show (a desktop notice about the agent was clicked: a window shows it)"), ("path", "string", false, "the file"), ("job", "string", false, "input's job (`job/<id>`, `job_input`)"), ("message", "string", true, ""), ("state", "string", true, "waiting"), ("at", "string", true, "when")],
        about: "What agents ask of the user beyond their conversation, where no window of loom's shows it (loomd): a UI shows them; `ask_done`, `ask_open`.",
    },
    Resource { name: "bridge/<name>", who: &["bridge (that one)"], item: &[("id", "string", true, ""), ("event", "object", true, "a bridge-protocol event")], about: "A bridge's events: each an item added and taken away at once (a snapshot is empty)." },
];

/// The protocol's own methods.
/// How a web page reaches the API (`web_open`'s link).
const WEB: &str = "A web page (a UI in a browser or a webview) reaches the API through `web_open`'s link: \
http://127.0.0.1:<port>/?t=<token>. The token is traded once for a session, given in the address's fragment \
(`/#<session>`: sent to no server; the page keeps it for its tab and takes it out of the address); then \
`ws://<host>/api?s=<session>` is a WebSocket carrying the same messages, one per text frame. `Host` must be \
127.0.0.1 or localhost at that port and the WebSocket's `Origin` that page; there are no CORS headers and the page \
can't be framed. `web_close` lets every page go, connected ones too. The page's files: `LOOM_WEB_UI`, else the UI built into loom, else `<data dir>/loom/web` (its index.html), else a small page built in.";

const PROTOCOL_METHODS: &[(&str, &str, &str)] = &[
    ("initialize", "{protocol: {min, max}, client: {name, version}, capabilities}", "{protocol, server, principal, capabilities: {subscriptions: {page, queue}, commands}} — first, once"),
    ("subscribe", "{resource}", "{sub, resource}; its messages follow as `subscription` notifications"),
    ("unsubscribe", "{sub}", "{}"),
    ("resync", "{sub}", "{} — a fresh snapshot follows (after a gap)"),
];

/// `docs/api.md`.
pub fn markdown() -> String {
    let mut out = String::from(
        "# loom's core API\n\nGenerated from `core/src/api/spec.rs` (and `wire`'s types): edit there, then `LOOM_BLESS=1 cargo test -p loom-core spec`. \
         Who may do what, and why: `docs/trust.md`.\n\n## The wire\n\nJSON-RPC 2.0, one message per line, on `$XDG_RUNTIME_DIR/loom/<name>-api.sock` (the folder 0700, the socket 0600), \
         or on a connection loom hands down (bridges: fd 3). Params and results are objects; ids of commands are strings unique to the client \
         (sent again, a command isn't done again: the first result comes back). Readers ignore what they don't know; changes are additive only.\n\n\
         A subscription answers its snapshot in pages, then `synced`, then numbered `changes`; `seq` counts its messages with no gaps; a client that \
         falls behind gets a `resync` and a fresh snapshot. Ops: `upsert` (a whole item by id), `remove`, `append` (text to a field), `order`, `bytes`.\n\n\
         | Protocol method | Params | Result |\n|---|---|---|\n",
    );
    for (name, params, result) in PROTOCOL_METHODS {
        out.push_str(&format!("| `{name}` | `{params}` | {result} |\n"));
    }
    out.push_str(&format!("\n{WEB}\n"));
    out.push_str("\nServer notification: `subscription`, params a message (`{sub, seq, kind, …}`, see `docs/api.schema.json`).\n\n");
    out.push_str("Errors: -32700 not JSON, -32600 not a request, -32601 no such method, -32602 invalid params, -32002 initialize first, -32003 no protocol in common, -32010 not found, -32011 forbidden, -32012 conflict (answered elsewhere first), -32013 failed; `data.kind` names it.\n\n## Resources\n");
    for r in RESOURCES {
        out.push_str(&format!("\n### `{}`\n\n{}\n\nWho: {}.\n\n| Field | Type | |\n|---|---|---|\n", r.name, r.about, r.who.join(", ")));
        for (name, kind, required, about) in r.item {
            out.push_str(&format!("| `{name}` | {kind}{} | {about} |\n", if *required { "" } else { "?" }));
        }
    }
    out.push_str("\n## Commands\n");
    for c in COMMANDS {
        out.push_str(&format!("\n### `{}`\n\n{}\n\nWho: {}. Result: `{}`{}.\n", c.name, c.about, c.who.join(", "), c.result, if c.later { ", once done" } else { "" }));
        if !c.params.is_empty() {
            out.push_str("\n| Param | Type | |\n|---|---|---|\n");
            for (name, kind, required, about) in c.params {
                out.push_str(&format!("| `{name}` | {kind}{} | {about} |\n", if *required { "" } else { "?" }));
            }
        }
    }
    out
}

fn object(fields: &[Field]) -> Value {
    let mut properties = Map::new();
    for (name, kind, _, about) in fields {
        properties.insert((*name).to_owned(), json!({ "type": kind, "description": about }));
    }
    let required: Vec<&str> = fields.iter().filter(|f| f.2).map(|f| f.0).collect();
    json!({ "type": "object", "properties": properties, "required": required })
}

/// `docs/api.schema.json`: the messages' types, each resource's items and each command's params.
pub fn schema() -> Value {
    let message = serde_json::to_value(schemars::schema_for!(super::Message)).unwrap_or_default();
    let resources: Map<String, Value> = RESOURCES.iter().map(|r| (r.name.to_owned(), json!({ "description": r.about, "who": r.who, "item": object(r.item) }))).collect();
    let commands: Map<String, Value> = COMMANDS.iter().map(|c| (c.name.to_owned(), json!({ "description": c.about, "who": c.who, "later": c.later, "params": object(c.params), "result": c.result }))).collect();
    json!({
        "title": "loom's core API",
        "protocol": super::rpc::PROTOCOL,
        "message": message,
        "resources": resources,
        "commands": commands,
    })
}

/// The resource a subscription name is (`conversation/3` → `conversation/<agent>`), if listed.
pub fn resource_of(name: &str) -> Option<&'static Resource> {
    RESOURCES.iter().find(|r| match r.name.split_once('<') {
        Some((prefix, _)) => name.starts_with(prefix),
        None => r.name == name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_spec_is_what_the_code_says() {
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs");
        let made = [("api.md", markdown()), ("api.schema.json", serde_json::to_string_pretty(&schema()).unwrap() + "\n")];
        for (file, text) in made {
            let path = docs.join(file);
            if std::env::var_os("LOOM_BLESS").is_some() {
                std::fs::write(&path, &text).unwrap();
                continue;
            }
            let committed = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(committed == text, "docs/{file} differs from core/src/api/spec.rs: LOOM_BLESS=1 cargo test -p loom-core spec");
        }
    }

    #[test]
    fn every_command_the_core_registers_is_listed() {
        crate::rt::set_host(|_| {}, |_, _| {});
        crate::agent::commands::serve();
        crate::jobs_api::serve();
        crate::bridges::api::serve();
        crate::api::blobs::serve();
        crate::api::web::serve();
        for name in crate::api::hub().commands() {
            assert!(COMMANDS.iter().any(|c| c.name == name), "{name} is not in spec.rs");
        }
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len(), "listed twice");
        assert!(resource_of("conversation/12").is_some_and(|r| r.name == "conversation/<agent>"));
        assert!(resource_of("weather").is_none());
    }
}
