//! What a vault keeps of its Notes chats (the chats themselves are agents in the vault).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::json;

use crate::agent::Agent;
use crate::state::Saved;

/// A topic a chat is about: (its name, its folder in the vault).
pub type Scope = (String, String);

/// The notes a chat's agent is about to edit, as they were before: path → text (None: new).
pub type Before = Rc<RefCell<HashMap<PathBuf, Option<String>>>>;

/// What the chat's agent is told, besides the skill.
pub const PROMPT: &str = "You are in loom's Notes chat: the working folder is the user's notes vault. Follow the notes-chat skill for how to read and change the notes.";

/// The vault's own list of its Notes chats (`.loom/chats.json`): only those are listed.
fn chats_file(vault: &Path) -> PathBuf {
    vault.join(crate::vault::OWN).join("chats.json")
}

pub fn chats(vault: &Path) -> Vec<String> {
    std::fs::read_to_string(chats_file(vault)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn remember_chat(vault: &Path, session: &str) {
    let mut all = chats(vault);
    if all.iter().any(|s| s == session) {
        return;
    }
    all.push(session.to_owned());
    let _ = std::fs::create_dir_all(vault.join(crate::vault::OWN));
    let _ = std::fs::write(chats_file(vault), serde_json::to_string(&all).unwrap_or_default());
}

/// The topics chats are about (`.loom/chat-topics.json`): session → topic name.
fn topics_file(vault: &Path) -> PathBuf {
    vault.join(crate::vault::OWN).join("chat-topics.json")
}

pub fn chat_topics(vault: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(topics_file(vault)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn remember_topic(vault: &Path, session: &str, topic: &str) {
    let mut all = chat_topics(vault);
    all.insert(session.to_owned(), topic.to_owned());
    let _ = std::fs::create_dir_all(vault.join(crate::vault::OWN));
    let _ = std::fs::write(topics_file(vault), serde_json::to_string(&all).unwrap_or_default());
}

/// `path` with its folder's links resolved (the vault may be reached through a link).
pub fn resolved(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => crate::history::real(parent).join(name),
        _ => crate::history::real(path),
    }
}

/// The topic a past chat was about, if it was.
pub fn scope_of(vault: &Path, session: &str) -> Option<Scope> {
    let name = chat_topics(vault).remove(session)?;
    let topic = crate::topics::list(vault).into_iter().find(|t| t.name == name)?;
    Some((topic.name, topic.folder))
}

/// A chat's agent as saved: in the vault, the past chat `session` or a new one.
pub fn saved_for(vault: &Path, session: Option<String>, scope: Option<&Scope>) -> Saved {
    let title = match (&session, scope) {
        (Some(_), _) => String::new(),
        (None, Some((name, _))) => format!("About {name}"),
        (None, None) => "New chat".to_owned(),
    };
    Saved { cwd: vault.to_owned(), session_id: session, title, named: false, draft: String::new(), dirs: Vec::new(), used: 0, awake: false, options: serde_json::Value::Null, chosen: serde_json::json!({}) }
}

/// What the chat's input says before anything is typed.
pub fn hint(scope: Option<&Scope>) -> String {
    match scope {
        Some((name, _)) => format!("Ask about {name}, or tell it what to keep"),
        None => "Ask your notes, or tell them what to keep".to_owned(),
    }
}

/// `agent` made a Notes chat: kept out of Scribe's journal, told what it is (and its topic),
/// on Sonnet; each note it is about to edit read first, into what is returned.
pub fn prepare(agent: &Agent, vault: &Path, scope: Option<&Scope>) -> Before {
    agent.no_journal.set(true);
    let prompt = match scope {
        Some((name, folder)) => format!("{PROMPT} This chat is about the topic \"{name}\"; its notes are in the folder {folder}. Keep to it unless the user asks for more."),
        None => PROMPT.to_owned(),
    };
    *agent.extra_options.borrow_mut() = json!({ "model": "sonnet", "appendSystemPrompt": prompt });
    let before: Before = Rc::default();
    let (map, root) = (before.clone(), crate::history::real(vault));
    *agent.on_tool.borrow_mut() = Some(Rc::new(move |update: &serde_json::Value| {
        if update["sessionUpdate"] != "tool_call" || update["kind"] != "edit" {
            return;
        }
        let paths = update["locations"].as_array().into_iter().flatten().filter_map(|l| l["path"].as_str());
        for path in paths {
            let path = resolved(Path::new(path));
            if path.starts_with(&root) && !map.borrow().contains_key(&path) {
                let was = std::fs::read_to_string(&path).ok();
                map.borrow_mut().insert(path, was);
            }
        }
    }));
    before
}

/// The notes changed since `before` was read (taken: it starts over), as (note relative to the
/// vault, before, after), read off the core's thread and handed to `done` when there are some.
pub fn collect_edits(vault: &Path, before: &Before, done: impl FnOnce(Vec<(String, Option<String>, String)>) + 'static) {
    let taken: Vec<(PathBuf, Option<String>)> = before.borrow_mut().drain().collect();
    if taken.is_empty() {
        return;
    }
    let root = crate::history::real(vault);
    crate::rt::spawn_local(async move {
        let edits = crate::rt::spawn_blocking(move || {
            taken
                .into_iter()
                .filter_map(|(path, before)| {
                    let note = path.strip_prefix(&root).ok()?.to_string_lossy().into_owned();
                    if !note.ends_with(".md") || note.starts_with(".loom/") || note.starts_with(".git/") {
                        return None;
                    }
                    let after = std::fs::read_to_string(&path).ok()?;
                    (before.as_deref() != Some(after.as_str())).then_some((note, before, after))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        if !edits.is_empty() {
            done(edits);
        }
    });
}
