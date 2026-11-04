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
    Command { name: "stop", who: &["window", "cli", "bridge"], params: &[("agent", "string", true, "the agent's id")], result: "{}", later: false, about: "Stops the agent's turn." },
    Command { name: "new_agent", who: PEOPLE, params: &[("folder", "string", true, "an absolute folder: its project")], result: "{agent}", later: false, about: "Starts an agent in a folder." },
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
    Command { name: "scribe_teach", who: WINDOW, params: &[("id", "string", true, "a `scribe` item"), ("said", "string", true, "why not")], result: "{}", later: false, about: "Tells Scribe why it was wrong." },
    Command { name: "scribe_keep_rule", who: WINDOW, params: &[("id", "string", true, "a `scribe` item"), ("rule", "string", true, "the rule")], result: "{}", later: false, about: "Keeps a rule Scribe proposes." },
    Command { name: "scribe_forget", who: WINDOW, params: &[("id", "string", true, "a `scribe` item")], result: "{}", later: false, about: "Forgets a rule Scribe learned." },
    Command {
        name: "save",
        who: &["bridge"],
        params: &[("agent", "string", true, "the agent's id"), ("name", "string", true, "the file's name"), ("data", "string", true, "its bytes, base64")],
        result: "{path}",
        later: true,
        about: "A file for an agent, saved in its folder under `<bridge>-files/` (never over another).",
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
            ("status", "string", true, "idle, working, asking, suspended, …"),
            ("unseen", "boolean", true, "something new the user hasn't seen"),
            ("busy", "boolean", true, "a turn under way"),
            ("questions", "integer", true, "questions waiting"),
            ("used", "string", true, "last used (RFC 3339)"),
            ("place", "string", true, "where in the project it works (a repo, a folder)"),
            ("edited", "integer", true, "files it changed"),
            ("awake", "boolean", true, "kept from resting"),
            ("looking", "boolean", false, "the user has it in front of them in loom"),
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
            ("images", "array", false, "pictures: blob ids (`blob`)"),
            ("call", "string", false, "a tool's call id"),
            ("title", "string", false, "a tool's title"),
            ("tool", "string", false, "a tool's kind"),
            ("input", "object", false, "a tool's input"),
            ("content", "array", false, "a tool's content (diffs…)"),
            ("locations", "array", false, "files a tool touched"),
            ("output", "string", false, "a tool's output (its last 16,000 characters)"),
            ("exit", "integer", false, "a command's exit code"),
            ("parent", "string", false, "the task a tool is part of"),
            ("detail", "string", false, "a question's detail"),
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
        item: &[("id", "string", true, "`account`, or an agent's id"), ("kind", "string", true, "account or agent"), ("plan", "string", false, "the account's plan"), ("windows", "array", false, "limits: {kind, utilization, resets_at, seen_at}"), ("used", "integer", false, "an agent's context used"), ("size", "integer", false, "its context's size"), ("cost", "object", false, "{amount, currency}"), ("limit", "object", false, "a spending limit")],
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
    Resource { name: "bridge/<name>", who: &["bridge (that one)"], item: &[("id", "string", true, ""), ("event", "object", true, "a bridge-protocol event")], about: "A bridge's events: each an item added and taken away at once (a snapshot is empty)." },
];

/// The protocol's own methods.
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
