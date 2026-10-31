//! What a vault keeps of its Notes chats (the chats themselves are agents in the vault).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
