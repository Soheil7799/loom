//! `loom mcp`: the MCP server loom gives every agent (stdio, one JSON-RPC message per line).
//! Its tools act on the running app through the core API (the `tool` command): named jobs in the tray
//! (loom's answer to herdr's named panes), files opened for the user, notifications. With the
//! Notes add-on on, also the user's notes: searched and read here, from the vault's index.
//!
//! LOOM_SOCKET, set by loom when it starts the agent, says where loom's API is. Who the
//! agent is, loom works out from the process that connects (this one, under the agent's), never
//! from what it says: an agent can't act as another.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};


const PROTOCOL: &str = "2025-06-18";

/// The vault's index, kept between calls (refreshed from the files' times at each one).
static NOTES: Mutex<Option<crate::notes::Index>> = Mutex::new(None);
/// The project's code, for loom_find (kept by this process, refreshed at each search).
static CODE: Mutex<Option<crate::code_search::Index>> = Mutex::new(None);

/// The Notes vault loom has (None: the add-on is off, or no vault chosen).
fn vault(socket: &str, agent: u32) -> Option<PathBuf> {
    let info = call(socket, agent, "notes_info", json!({})).ok()?;
    info["vault"].as_str().map(PathBuf::from)
}

/// One of the agent's tools, done by loom (`tool` in the core API, for agents only).
fn call(socket: &str, agent: u32, name: &str, params: Value) -> Result<Value, String> {
    use crate::api::cli::{Connection, new_id};
    // Started by an older loom, still running (its control socket, not `…-api.sock`): its
    // protocol, until it is restarted. Goes with the next release.
    if !socket.ends_with("-api.sock") {
        return legacy_call(socket, agent, name, params);
    }
    let mut loom = Connection::open(std::path::Path::new(socket)).map_err(|e| format!("{e}; is loom still open?"))?;
    loom.command(&new_id(), "tool", json!({ "name": name, "params": params })).map_err(|e| e.message)
}

/// An older loom's control protocol: one request naming the agent, one answer.
fn legacy_call(socket: &str, agent: u32, method: &str, params: Value) -> Result<Value, String> {
    use std::io::BufReader;
    let mut stream = std::os::unix::net::UnixStream::connect(socket).map_err(|e| format!("loom is not reachable ({e}); is it still open?"))?;
    writeln!(stream, "{}", json!({ "agent": agent, "method": method, "params": params })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let answer: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer from loom: {e}"))?;
    match answer.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_owned()),
        None => Ok(answer["ok"].clone()),
    }
}

/// Calls in flight, by request id: set to stop one (the client cancelled it).
type Running = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

pub fn run() -> i32 {
    let socket = std::env::var("LOOM_SOCKET").unwrap_or_default();
    let agent: u32 = std::env::var("LOOM_AGENT").ok().and_then(|a| a.parse().ok()).unwrap_or(0);
    let stdin = std::io::stdin();
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let running: Running = Arc::default();
    let send = |stdout: &Arc<Mutex<std::io::Stdout>>, response: Value| {
        let mut out = stdout.lock().unwrap_or_else(|e| e.into_inner());
        writeln!(out, "{response}").and_then(|()| out.flush()).is_ok()
    };
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let method = message["method"].as_str().unwrap_or("");
        if method == "notifications/cancelled" {
            let id = message["params"]["requestId"].to_string();
            if let Some(flag) = running.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
                flag.store(true, Ordering::Relaxed);
            }
            continue;
        }
        let Some(id) = message.get("id").cloned() else { continue }; // notifications need no answer
        // Tool calls run side by side (a long loom_wait must not hold up the next call).
        if method == "tools/call" {
            let cancelled = Arc::new(AtomicBool::new(false));
            running.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), cancelled.clone());
            let (socket, stdout, running, params) = (socket.clone(), stdout.clone(), running.clone(), message["params"].clone());
            std::thread::spawn(move || {
                let result = call_tool(&socket, agent, &params, &cancelled);
                running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id.to_string());
                send(&stdout, json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            });
            continue;
        }
        let notes = matches!(method, "initialize" | "tools/list").then(|| vault(&socket, agent)).flatten();
        let reply = match method {
            "initialize" => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
                "instructions": format!(
                    "You run inside loom, the user's agent workspace. Long-running or streaming commands (dev servers, watchers, builds, test suites, log tails, installs) go to loom's job tray with loom_run, where the user sees them, not to Bash. Read them with loom_output and loom_wait. Show the user a file you wrote for them to read with loom_open. When the user must put something in a file themselves (an API key, a token, a password, any secret that must not pass through the chat), use loom_edit: it opens the file for them and waits until they are done. To find where something is in the code when you don't know the file, ask loom_find in words before grepping. A file you have read stays in your context: don't Read it again unless it changed since.{}",
                    if notes.is_some() { " The user keeps notes about their life and work (finances, work, study, languages, health, their system's setup…): before answering about their own facts, plans or past decisions, look with loom_note_search and loom_note_read. Don't edit the vault yourself; loom keeps it." } else { "" }
                )
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools(notes.as_deref()) })),
            _ => Err(json!({ "code": -32601, "message": format!("unknown method {method}") })),
        };
        let response = match reply {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        };
        if !send(&stdout, response) {
            break;
        }
    }
    0
}

fn tools(notes: Option<&std::path::Path>) -> Value {
    let name = json!({ "type": "string", "description": "The job's name, e.g. dev, test, build, logs (one job per name: running it again reuses the chip)" });
    let mut all = json!([
        {
            "name": "loom_run",
            "description": "Run a shell command as a named job in loom's job tray, in the project folder, where the user sees it live. Use for anything long-running or streaming (dev server, watch mode, build, test suite, install, log tail). Returns at once; then loom_wait or loom_output. Running a name again reruns that job.",
            "inputSchema": { "type": "object", "properties": {
                "name": name,
                "command": { "type": "string", "description": "Shell command (bash -c)" },
                "cwd": { "type": "string", "description": "Folder to run in (e.g. one repo of the project), relative to yours; default the project folder" },
                "restart": { "type": "boolean", "description": "Stop the job if it is still running and start this instead (default false)" },
                "input": { "type": "boolean", "description": "The command asks the user to type something: sudo's password, a passphrase, a y/n. Its terminal opens in front of them and they type into it there (never put a password in the command or ask for it in the chat); then loom_wait until it exits. Default false" },
                "message": { "type": "string", "description": "With input: what they are asked for, shown with the terminal (\"Your sudo password, to install the package\")" }
            }, "required": ["name", "command"] }
        },
        {
            "name": "loom_output",
            "description": "The last lines a job printed, with whether it is still running and its exit code.",
            "inputSchema": { "type": "object", "properties": {
                "name": name,
                "lines": { "type": "integer", "description": "How many lines (default 60, max 2000)" }
            }, "required": ["name"] }
        },
        {
            "name": "loom_wait",
            "description": "Wait until a job exits, or until its output contains some text (e.g. 'ready on'), or a timeout. Returns the state and the last lines.",
            "inputSchema": { "type": "object", "properties": {
                "name": name,
                "until": { "type": "string", "description": "Return as soon as the output contains this text" },
                "timeout_seconds": { "type": "integer", "description": "Default 120, max 1800" },
                "lines": { "type": "integer", "description": "Lines of output to return (default 40)" }
            }, "required": ["name"] }
        },
        {
            "name": "loom_stop",
            "description": "Stop a running job (SIGTERM to its process group).",
            "inputSchema": { "type": "object", "properties": { "name": name }, "required": ["name"] }
        },
        {
            "name": "loom_jobs",
            "description": "The jobs in this project's tray: name, running, exit code.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "loom_open",
            "description": "Open a file in loom's editor panel for the user to read or fill in (an .env to complete, a document you wrote). Does not take their keyboard focus.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Absolute, or relative to the agent's folder" }
            }, "required": ["path"] }
        },
        {
            "name": "loom_edit",
            "description": "Ask the user to fill a file in themselves, and wait until they have: opens it in loom's editor with your message and a Done button, and returns when they press Done (saved) or Cancel. For anything the user should not paste into the chat: API keys, tokens, passwords, personal data. The file is created empty if it doesn't exist. With secret: true (the default), you get back only which KEY=value lines now have a value, never the values: don't read the file afterwards, use it (source it, point the program at it).",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "Absolute, or relative to the agent's folder (e.g. .env)" },
                "message": { "type": "string", "description": "What to put in it, shown above the file: \"Add your OPENAI_API_KEY\"" },
                "secret": { "type": "boolean", "description": "It holds secrets: report only which keys are set, never the content (default true)" }
            }, "required": ["path", "message"] }
        },
        {
            "name": "loom_notify",
            "description": "Send the user a desktop notification (for something that needs them and can't wait for the end of the turn).",
            "inputSchema": { "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] }
        },
        {
            "name": "loom_agents",
            "description": "The other agents open in loom: title, project, status.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "loom_find",
            "description": "Find where something is in this project's code when you don't know the file: a feature, a behavior, a concept or a name, in plain words (\"where finished subagent rows are removed\", \"archive the done lane\"). Ranks the project's concept cards (.loom/map: a feature's files, symbols and gotchas) and the definitions (functions, types, doc headings) by their names, doc comments, bodies and paths, and returns places (path:line and the definition's first line), not code: then Read only those lines. One call instead of several Grep and Read rounds; Grep stays best for an exact string you already know.",
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "What you look for, in words" },
                "limit": { "type": "integer", "description": "How many places (default 10, at most 25)" }
            }, "required": ["query"] }
        },
        {
            "name": "loom_board",
            "description": "This project's board (the user's kanban: Backlog, To do, Doing, Done), its cards with their ids, descriptions and dates. Look here for what to work on and what is in progress.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "loom_board_add",
            "description": "File a card on this project's board: a bug or a review finding you won't fix now, a follow-up, a task the user gave. Say what and where (file:line), and why, in the description.",
            "inputSchema": { "type": "object", "properties": {
                "title": { "type": "string", "description": "Short: what is to be done" },
                "body": { "type": "string", "description": "The details: where, why, how to fix" },
                "lane": { "type": "string", "description": "Backlog (default), To do, Doing or Done" }
            }, "required": ["title"] }
        },
        {
            "name": "loom_board_move",
            "description": "Move a card: to Doing when you start on it, to Done when it is finished (dated automatically).",
            "inputSchema": { "type": "object", "properties": {
                "card": { "type": "integer" },
                "lane": { "type": "string", "description": "Backlog, To do, Doing or Done" }
            }, "required": ["card", "lane"] }
        },
        {
            "name": "loom_board_note",
            "description": "Add a line to a card's description: what was done (the commit), what is left.",
            "inputSchema": { "type": "object", "properties": {
                "card": { "type": "integer" },
                "note": { "type": "string" }
            }, "required": ["card", "note"] }
        }
    ]);
    // The user's notes, only when they have them on.
    if let (Some(vault), Some(list)) = (notes, all.as_array_mut()) {
        let folders: Vec<String> = crate::topics::list(vault).into_iter().filter(|t| t.enabled).map(|t| t.folder).collect();
        let folders = if folders.is_empty() { String::new() } else { format!(" Topic folders: {}.", folders.join(", ")) };
        list.push(json!({
            "name": "loom_note_search",
            "description": format!("Search the user's own notes (an Obsidian vault loom keeps about their life and work). Returns the best matching notes with the lines that matched, what each links to and what links to it.{folders}"),
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "Words to look for (titles, aliases, tags and headings count most)" },
                "folder": { "type": "string", "description": "Only notes in this folder of the vault (a topic's, e.g. Finances)" },
                "limit": { "type": "integer", "description": "How many notes (default 8, max 30)" }
            }, "required": ["query"] }
        }));
        list.push(json!({
            "name": "loom_note_topics",
            "description": "The user's note topics: each with its folder in the vault, what belongs in it, and how many notes it has. Start here to see what the notes cover.",
            "inputSchema": { "type": "object", "properties": {} }
        }));
        list.push(json!({
            "name": "loom_note_list",
            "description": "The notes in one folder of the vault (a topic's), newest first, with their titles and when each last changed.",
            "inputSchema": { "type": "object", "properties": {
                "folder": { "type": "string", "description": "The folder, e.g. Finances (empty: the whole vault)" },
                "limit": { "type": "integer", "description": "How many (default 30, max 200)" }
            } }
        }));
        list.push(json!({
            "name": "loom_note_recent",
            "description": "The notes changed lately (what the user noted this week, say), newest first.",
            "inputSchema": { "type": "object", "properties": {
                "days": { "type": "integer", "description": "How many days back (default 7)" },
                "limit": { "type": "integer", "description": "How many (default 30, max 200)" }
            } }
        }));
        list.push(json!({
            "name": "loom_note_read",
            "description": "Read one of the user's notes in full, by its path in the vault or its [[name]], with its links and backlinks.",
            "inputSchema": { "type": "object", "properties": {
                "note": { "type": "string", "description": "Its path in the vault (Finances/Overview.md) or its name (Overview)" }
            }, "required": ["note"] }
        }));
    }
    all
}

/// loom_note_search / loom_note_read, from the index of the vault loom has now.
/// loom_find: the places in the agent's folder (this process runs in it: a worktree's agent
/// searches its worktree) and its project's concept cards best matching the query.
fn find(socket: &str, agent: u32, args: &Value) -> Result<String, String> {
    let agents = call(socket, agent, "list_agents", json!({}))?;
    let project = agents.as_array().into_iter().flatten().find(|a| a["you"] == true).and_then(|a| a["project"].as_str()).map(PathBuf::from).ok_or("loom doesn't know this agent's project")?;
    let root = std::env::current_dir().unwrap_or_else(|_| project.clone());
    let query = args["query"].as_str().unwrap_or("").trim();
    if query.is_empty() {
        return Err("Say what to find.".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(10).clamp(1, 25) as usize;
    let mut held = CODE.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_none_or(|index| index.root() != root) {
        *held = Some(crate::code_search::Index::new(&root, Some(crate::code_search::map_of(&project))));
    }
    let index = held.as_mut().ok_or("no index")?;
    index.refresh();
    let found = index.search(query, limit);
    if found.is_empty() {
        return Ok(format!("Nothing in {} matched those words: try others (what the code would call it), or Grep for an exact string.", root.display()));
    }
    let mut out = format!("In {} (paths relative to it):\n", root.display());
    for f in found {
        out.push_str(&format!("{}:{}  {}\n", f.path, f.line, f.head));
        if !f.doc.is_empty() {
            out.push_str(&format!("    {}\n", f.doc));
        }
        if !f.also.is_empty() {
            out.push_str(&format!("    also in another checkout: {}\n", f.also.join(", ")));
        }
    }
    out.push_str("Not it? Ask again in other words (what the code would call it).");
    Ok(out)
}

fn note_tool(socket: &str, agent: u32, tool: &str, args: &Value) -> Result<String, String> {
    let vault = vault(socket, agent).ok_or("The Notes add-on is off (or has no vault) in loom's settings.")?;
    let mut held = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    if held.as_ref().is_none_or(|index| index.root() != vault) {
        *held = Some(crate::notes::Index::new(&vault));
    }
    let index = held.as_mut().ok_or("no index")?;
    index.refresh();
    if tool == "loom_note_read" {
        let name = args["note"].as_str().unwrap_or("");
        let (path, text, links, backlinks) = index.read(name).ok_or_else(|| format!("No note named {name}. Find it with loom_note_search."))?;
        const MOST: usize = 60_000;
        let warning = if crate::vault::has_conflict_marks(&text) {
            "Careful: this note holds a sync conflict left unsettled (<<<<<<< lines): two versions in one. Don't change it; the user settles it in loom's Notes view.\n"
        } else {
            ""
        };
        let text = if text.len() > MOST { format!("{}\n… (cut: the note is longer)", &text[..text.floor_char_boundary(MOST)]) } else { text };
        return Ok(format!("{warning}{path}\n---\n{text}\n---\nLinks to: {}\nLinked from: {}", list_or_none(&links), list_or_none(&backlinks)));
    }
    let query = args["query"].as_str().unwrap_or("");
    let limit = args["limit"].as_u64().unwrap_or(8).clamp(1, 30) as usize;
    let hits = index.search(query, args["folder"].as_str(), limit);
    // Conflict copies are not searched: said, so they are not taken for missing notes.
    let copies = crate::vault::conflicts(&vault).len();
    let left_out = if copies == 0 {
        String::new()
    } else {
        format!("\n\n({copies} sync conflict cop{} left out: older or other versions of notes, for the user to settle in loom's Notes view.)", if copies == 1 { "y is" } else { "ies are" })
    };
    if hits.is_empty() {
        return Ok(format!("No note matches \"{query}\" ({} notes in the vault).{left_out}", index.len()));
    }
    Ok(hits
        .iter()
        .map(|h| format!("{} ({})\n{}\nLinks to: {} · Linked from: {}", h.path, h.title, h.snippet, list_or_none(&h.links), list_or_none(&h.backlinks)))
        .collect::<Vec<_>>()
        .join("\n\n")
        + &left_out)
}

/// loom_note_topics / loom_note_list / loom_note_recent: what the vault holds, read from its
/// files (loom's own folder and git's left out).
fn note_browse(socket: &str, agent: u32, tool: &str, args: &Value) -> Result<String, String> {
    let vault = vault(socket, agent).ok_or("The Notes add-on is off (or has no vault) in loom's settings.")?;
    let notes = crate::vault::all_notes(&vault);
    let limit = args["limit"].as_u64().unwrap_or(30).clamp(1, 200) as usize;
    match tool {
        "loom_note_topics" => {
            let topics = crate::topics::list(&vault);
            if topics.is_empty() {
                return Ok("No topics yet.".to_owned());
            }
            Ok(topics
                .iter()
                .map(|t| {
                    let count = notes.iter().filter(|n| !t.folder.is_empty() && n.0.starts_with(&format!("{}/", t.folder.trim_end_matches('/')))).count();
                    let about = if t.listens_for.is_empty() { String::new() } else { format!(" · about: {}", t.listens_for.join(", ")) };
                    format!("{} (folder {}, {count} notes{}){about}", t.name, if t.folder.is_empty() { "none" } else { &t.folder }, if t.enabled { "" } else { ", off" })
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "loom_note_list" => {
            let folder = args["folder"].as_str().unwrap_or("").trim_matches('/');
            let found: Vec<&(String, String, u64)> = notes.iter().filter(|n| folder.is_empty() || n.0.starts_with(&format!("{folder}/"))).take(limit).collect();
            if found.is_empty() {
                return Ok(format!("No notes in {}.", if folder.is_empty() { "the vault" } else { folder }));
            }
            Ok(found.iter().map(|(path, title, when)| listed(path, title, *when)).collect::<Vec<_>>().join("\n"))
        }
        _ => {
            let days = args["days"].as_u64().unwrap_or(7).max(1);
            let since = crate::state::now().saturating_sub(days * 86_400);
            let found: Vec<&(String, String, u64)> = notes.iter().filter(|n| n.2 >= since).take(limit).collect();
            if found.is_empty() {
                return Ok(format!("No note changed in the last {days} days."));
            }
            Ok(found.iter().map(|(path, title, when)| listed(path, title, *when)).collect::<Vec<_>>().join("\n"))
        }
    }
}

/// A note in a list: its path, title and age; a conflict copy said to be one.
fn listed(path: &str, title: &str, when: u64) -> String {
    match crate::vault::conflict_of(path) {
        Some(of) => format!("{path} · a sync conflict copy of {of}, not a note: leave it to the user · changed {}", crate::vault::ago(when)),
        None => format!("{path} ({title}) · changed {}", crate::vault::ago(when)),
    }
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() { "none".to_owned() } else { items.join(", ") }
}

fn call_tool(socket: &str, agent: u32, params: &Value, cancelled: &AtomicBool) -> Value {
    let args = &params["arguments"];
    let result = match params["name"].as_str().unwrap_or("") {
        "loom_wait" => wait(socket, agent, args, cancelled),
        "loom_run" => call(socket, agent, "run_job", args.clone()).and_then(|v| {
            let name = v["name"].as_str().unwrap_or("?").to_owned();
            if args["input"].as_bool() != Some(true) {
                return Ok(format!("Started job {name} in loom's tray. Use loom_wait or loom_output to follow it."));
            }
            // Its terminal in front of the user, to type into.
            call(socket, agent, "show_job", json!({ "name": name, "message": args["message"] }))?;
            Ok(format!("Started job {name}; its terminal is open in front of the user, who types into it there. Use loom_wait until it exits (a long timeout: they may take a while), then loom_output."))
        }),
        "loom_output" => call(socket, agent, "job_output", args.clone()).map(describe_job),
        "loom_stop" => call(socket, agent, "stop_job", args.clone()).map(|_| "Stopped.".to_owned()),
        "loom_jobs" => call(socket, agent, "list_jobs", json!({})).map(|v| pretty(&v)),
        "loom_open" => call(socket, agent, "open_file", args.clone()).map(|_| "Opened in loom's editor.".to_owned()),
        "loom_edit" => edit(socket, agent, args, cancelled),
        "loom_notify" => call(socket, agent, "notify", args.clone()).map(|_| "Sent.".to_owned()),
        "loom_agents" => call(socket, agent, "list_agents", json!({})).map(|v| pretty(&v)),
        "loom_find" => find(socket, agent, args),
        "loom_board" => call(socket, agent, "board", json!({})).map(|v| pretty(&v)),
        "loom_board_add" => call(socket, agent, "board_add", args.clone()).map(|v| format!("Filed as card {}.", v["card"])),
        "loom_board_move" => call(socket, agent, "board_move", args.clone()).map(|_| {
            if args["lane"].as_str().is_some_and(|l| l.eq_ignore_ascii_case("done")) {
                // the moment it knows best where the feature lives: kept for the next agent
                "Moved. If finding your way took searching, add or update the feature's concept card in the project's .loom/map/ (next to .loom/board.md; the format is in the loom skill), so loom_find answers the next agent at once.".to_owned()
            } else {
                "Moved.".to_owned()
            }
        }),
        "loom_board_note" => call(socket, agent, "board_edit", json!({ "card": args["card"], "note": args["note"] })).map(|_| "Noted.".to_owned()),
        tool @ ("loom_note_search" | "loom_note_read") => note_tool(socket, agent, tool, args),
        tool @ ("loom_note_topics" | "loom_note_list" | "loom_note_recent") => note_browse(socket, agent, tool, args),
        other => Err(format!("unknown tool {other}")),
    };
    match result {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(error) => json!({ "content": [{ "type": "text", "text": error }], "isError": true }),
    }
}

fn wait(socket: &str, agent: u32, args: &Value, cancelled: &AtomicBool) -> Result<String, String> {
    let timeout = Duration::from_secs(args["timeout_seconds"].as_u64().unwrap_or(120).min(1800));
    let until = args["until"].as_str().filter(|u| !u.is_empty());
    let lines = args["lines"].as_u64().unwrap_or(40);
    let started = Instant::now();
    loop {
        let job = call(socket, agent, "job_output", json!({ "name": args["name"], "lines": 400 }))?;
        let output = job["output"].as_str().unwrap_or("");
        let matched = until.is_some_and(|u| output.contains(u));
        if !job["running"].as_bool().unwrap_or(false) || matched || started.elapsed() >= timeout {
            let tail: Vec<&str> = output.lines().rev().take(lines as usize).collect::<Vec<_>>().into_iter().rev().collect();
            let mut short = job.clone();
            short["output"] = json!(tail.join("\n"));
            let why = if matched {
                "the output matched".to_owned()
            } else if job["running"].as_bool().unwrap_or(false) {
                format!("timed out after {}s; still running", timeout.as_secs())
            } else {
                "it exited".to_owned()
            };
            return Ok(format!("Stopped waiting: {why}.\n{}", describe_job(short)));
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// loom_edit: the file opened for the user, then waiting (cancellable) for Done or Cancel.
fn edit(socket: &str, agent: u32, args: &Value, cancelled: &AtomicBool) -> Result<String, String> {
    let asked = call(socket, agent, "request_edit", json!({ "path": args["path"], "message": args["message"] }))?;
    let id = asked["id"].as_u64().ok_or("loom did not open the file")?;
    let path = asked["path"].as_str().unwrap_or("").to_owned();
    let secret = args["secret"].as_bool().unwrap_or(true);
    // The key names the agent put there itself (the template): the only names said back.
    let template = std::fs::read_to_string(&path).unwrap_or_default();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = call(socket, agent, "edit_cancel", json!({ "id": id }));
            return Err("cancelled".to_owned());
        }
        let state = call(socket, agent, "edit_state", json!({ "id": id }))?;
        match state["state"].as_str().unwrap_or("gone") {
            "waiting" => std::thread::sleep(Duration::from_millis(500)),
            "done" => {
                let _ = call(socket, agent, "edit_cancel", json!({ "id": id }));
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                return Ok(if secret {
                    format!("The user filled in {path} and pressed Done. {} Don't read the file or print its values: use it as it is.", keys_set(&template, &text))
                } else {
                    format!("The user edited {path} and pressed Done; read it to go on.")
                });
            }
            "cancelled" => {
                let _ = call(socket, agent, "edit_cancel", json!({ "id": id }));
                return Ok(format!("The user cancelled: {path} was not filled in. Ask them what to do instead."));
            }
            _ => return Err("the request is gone (loom was closed or restarted)".to_owned()),
        }
    }
}

/// A file's `KEY=value` (or `key: value`) lines: (key, has a value).
fn key_lines(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once(['=', ':'])?;
            let key = key.trim();
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            (!key.is_empty() && !key.contains(char::is_whitespace)).then(|| (key.to_owned(), !value.is_empty()))
        })
        .collect()
}

/// Which keys of a file of secrets have a value, without the values: what an agent may know
/// of it. Only keys that were in the file when it was asked for (`template`) are named:
/// anything else the user typed (a bare token, "name:password") could be a secret itself.
fn keys_set(template: &str, text: &str) -> String {
    let known: Vec<String> = key_lines(template).into_iter().map(|(key, _)| key).collect();
    let (mut set, mut empty, mut other) = (Vec::new(), Vec::new(), 0);
    for (key, has_value) in key_lines(text) {
        match (known.contains(&key), has_value) {
            (true, true) => set.push(key),
            (true, false) => empty.push(key),
            (false, _) => other += 1,
        }
    }
    let lines = text.lines().filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#')).count();
    let other = other.max(lines.saturating_sub(set.len() + empty.len()));
    let mut said = Vec::new();
    if !set.is_empty() {
        said.push(format!("Set: {}.", set.join(", ")));
    }
    if !empty.is_empty() {
        said.push(format!("Still empty: {}.", empty.join(", ")));
    }
    if other > 0 {
        said.push(format!("{other} other non-empty lines (not named: they may be secrets themselves)."));
    }
    if said.is_empty() {
        said.push("It is empty.".to_owned());
    }
    said.join(" ")
}

fn describe_job(job: Value) -> String {
    let state = if job["running"].as_bool().unwrap_or(false) {
        "running".to_owned()
    } else {
        match job["exit_code"].as_i64() {
            Some(code) => format!("exited with {code}"),
            None => "ended".to_owned(),
        }
    };
    format!("Job {}: {state}\n--- output ---\n{}", job["name"].as_str().unwrap_or("?"), job["output"].as_str().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_key_names_come_back() {
        let template = "OPENAI_API_KEY=\nDB_PASSWORD=\nEMPTY=\nPORT: 8080\n";
        let text = "# comment\nOPENAI_API_KEY=sk-secret\nexport DB_PASSWORD=\"hunter2\"\nEMPTY=\nPORT: 8080\nc2stcHJvai1hYmMxMjM=\nuser:hunter3\n";
        let said = super::keys_set(template, text);
        assert_eq!(said, "Set: OPENAI_API_KEY, DB_PASSWORD, PORT. Still empty: EMPTY. 2 other non-empty lines (not named: they may be secrets themselves).");
        assert!(!said.contains("sk-secret") && !said.contains("hunter") && !said.contains("c2st"));
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
