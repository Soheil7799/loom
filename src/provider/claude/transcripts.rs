//! Claude Code's own files of its conversations (`<config>/projects/<folder>/<id>.jsonl`):
//! the list of them, deleting and copying one, and reading one back as ACP updates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::tools::{self, ToolUse};

/// A conversation's file, whichever project folder it is in.
pub fn file(session: &str) -> Option<PathBuf> {
    crate::state::claude_file(session)
}

/// Every conversation: sessionId, cwd, title, updatedAt (ISO), newest first.
pub fn list() -> Vec<Value> {
    let mut found: Vec<(std::time::SystemTime, Value)> = Vec::new();
    let Ok(projects) = std::fs::read_dir(crate::state::claude_dir().join("projects")) else { return Vec::new() };
    for project in projects.flatten() {
        let Ok(files) = std::fs::read_dir(project.path()) else { continue };
        for entry in files.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            let Some(info) = summary(&path) else { continue };
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            let updated = gtk::glib::DateTime::from_unix_utc(modified.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)).ok().and_then(|t| t.format_iso8601().ok()).map(|s| s.to_string()).unwrap_or_default();
            found.push((modified, json!({ "sessionId": id, "cwd": info.0, "title": info.1, "updatedAt": updated })));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, v)| v).collect()
}

/// How much of a conversation's start and end is read for the list (its folder and first
/// prompt are at the start; its title lines are written again at the end).
const EDGE: u64 = 64 * 1024;

/// The start and the end of a file (the end's first line, cut, left out); all of it when small.
fn edges(path: &Path) -> Option<(String, String)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = vec![0; EDGE.min(len) as usize];
    file.read_exact(&mut head).ok()?;
    let head = String::from_utf8_lossy(&head).into_owned();
    if len <= EDGE * 2 {
        let mut rest = String::new();
        file.read_to_string(&mut rest).ok()?;
        return Some((head + &rest, String::new()));
    }
    file.seek(SeekFrom::Start(len - EDGE)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail).into_owned();
    let tail = tail.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or_default();
    Some((head, tail))
}

/// A conversation's folder and title, if it holds a conversation at all (a file of only
/// settings lines, or of a subagent, is none).
fn summary(path: &Path) -> Option<(String, Option<String>)> {
    let (head, tail) = edges(path)?;
    let (mut cwd, mut custom, mut ai, mut first, mut last) = (None, None, None, None, None);
    for line in head.lines().chain(tail.lines()) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") => custom = v["customTitle"].as_str().map(str::to_owned),
            Some("ai-title") => ai = v["aiTitle"].as_str().map(str::to_owned),
            Some("last-prompt") => last = v["lastPrompt"].as_str().map(str::to_owned),
            Some("user") if v["isSidechain"].as_bool() != Some(true) => {
                if cwd.is_none() {
                    cwd = v["cwd"].as_str().map(str::to_owned);
                }
                if first.is_none() && v["isMeta"].as_bool() != Some(true) {
                    first = prompt_text(&v["message"]["content"]);
                }
            }
            _ => {}
        }
    }
    let cwd = cwd?;
    let title = custom.or(ai).or(first).or(last).map(|t| t.lines().next().unwrap_or("").chars().take(200).collect::<String>());
    Some((cwd, title))
}

/// The user's own words in a message (not a command's wrapping or a tool result).
fn prompt_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let text = text.trim();
    const WRAPPED: [&str; 6] = ["<command-", "<local-command-", "<system-reminder>", "<task-notification>", "<bash-", "[Request interrupted"];
    (!text.is_empty() && !WRAPPED.iter().any(|w| text.starts_with(w))).then(|| text.to_owned())
}

/// Deletes a conversation's file (and its folder of subagents and tool results).
pub fn delete(session: &str) -> Result<(), String> {
    let path = file(session).ok_or_else(|| "Session not found".to_owned())?;
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    let folder = path.with_extension("");
    if folder.is_dir() {
        let _ = std::fs::remove_dir_all(folder);
    }
    Ok(())
}

/// A copy of a conversation under a new id, up to and with agent message `at` (its message
/// id; None: all of it). The new id.
pub fn fork(session: &str, at: Option<&str>) -> Result<String, String> {
    let path = file(session).ok_or_else(|| "Session not found".to_owned())?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let id = gtk::glib::uuid_string_random().to_string();
    let lines: Vec<&str> = text.lines().collect();
    // The last line of that message (a message is written as one line per block).
    let end = match at {
        Some(at) => lines
            .iter()
            .rposition(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v["message"]["id"].as_str() == Some(at) || v["uuid"].as_str() == Some(at)))
            .ok_or_else(|| "That message is not in the conversation's file".to_owned())?,
        None => lines.len().saturating_sub(1),
    };
    let mut out = String::new();
    for line in &lines[..=end.min(lines.len().saturating_sub(1))] {
        match serde_json::from_str::<Value>(line) {
            Ok(mut v) => {
                if v.get("sessionId").is_some() {
                    v["sessionId"] = id.clone().into();
                }
                out.push_str(&v.to_string());
            }
            Err(_) => out.push_str(line),
        }
        out.push('\n');
    }
    std::fs::write(path.with_file_name(format!("{id}.jsonl")), out).map_err(|e| e.to_string())?;
    Ok(id)
}

/// A conversation read back as session updates, from its latest compaction on (what came
/// before is loom's to show, from the same file), following the chain of the conversation
/// that is current (a rewound branch is left out). None: no such conversation.
pub fn replay(session: &str, cwd: &Path) -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(file(session)?).ok()?;
    let mut lines: HashMap<String, Value> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut last = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let Some(uuid) = v["uuid"].as_str().map(str::to_owned) else { continue };
        if matches!(v["type"].as_str(), Some("user" | "assistant" | "system")) {
            last = Some(uuid.clone());
        }
        order.push(uuid.clone());
        lines.insert(uuid, v);
    }
    // Back from the newest message to the latest compaction (or the start).
    let mut chain = Vec::new();
    let mut at = last;
    while let Some(uuid) = at.take() {
        let Some(v) = lines.get(&uuid) else { break };
        if v["subtype"].as_str() == Some("compact_boundary") {
            break;
        }
        chain.push(uuid.clone());
        at = v["parentUuid"].as_str().map(str::to_owned);
    }
    chain.reverse();
    let mut updates = Vec::new();
    let mut tools: HashMap<String, ToolUse> = HashMap::new();
    for uuid in chain {
        let v = &lines[&uuid];
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                if v["isMeta"].as_bool() == Some(true) || v["isCompactSummary"].as_bool() == Some(true) {
                    continue;
                }
                match content {
                    Value::String(text) => {
                        if let Some(text) = prompt_text(&Value::String(text.clone())) {
                            updates.push(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": text } }));
                        }
                    }
                    Value::Array(blocks) => {
                        // Its words first, then its pictures (as it was shown when sent).
                        let (words, rest): (Vec<&Value>, Vec<&Value>) = blocks.iter().partition(|b| b["type"] == "text");
                        for block in words.into_iter().chain(rest) {
                            match block["type"].as_str() {
                                Some("text") => {
                                    if let Some(text) = prompt_text(&json!([block])) {
                                        updates.push(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": text } }));
                                    }
                                }
                                Some("image") if block["source"]["type"] == "base64" => {
                                    updates.push(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "image", "data": block["source"]["data"], "mimeType": block["source"]["media_type"] } }));
                                }
                                Some("tool_result") => {
                                    let Some(tool) = block["tool_use_id"].as_str().and_then(|id| tools.get(id)) else { continue };
                                    let structured = &v["toolUseResult"];
                                    if structured.is_object() {
                                        updates.push(tools::response_update(tool, structured));
                                    }
                                    updates.extend(tools::result_updates(tool, block, structured));
                                    if tool.name == "TodoWrite" {
                                        updates.push(json!({ "sessionUpdate": "plan", "entries": tools::todo_entries(&tool.input) }));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            // A message sent into a running turn (Ctrl+Enter).
            Some("attachment") if v["attachment"]["type"] == "queued_command" => {
                let prompt = &v["attachment"]["prompt"];
                let text = match prompt {
                    Value::String(text) => text.clone(),
                    _ => prompt.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                };
                if !text.trim().is_empty() {
                    updates.push(json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": text } }));
                }
            }
            Some("assistant") => {
                let id = v["message"]["id"].as_str().unwrap_or("");
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => updates.push(json!({ "sessionUpdate": "agent_message_chunk", "messageId": id, "content": { "type": "text", "text": block["text"] } })),
                        Some("thinking") if block["thinking"].as_str().is_some_and(|t| !t.is_empty()) => {
                            updates.push(json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": block["thinking"] } }))
                        }
                        Some("tool_use") => {
                            let tool = ToolUse { id: block["id"].as_str().unwrap_or("").to_owned(), name: block["name"].as_str().unwrap_or("").to_owned(), input: block["input"].clone() };
                            updates.push(tools::tool_call(&tool, cwd));
                            tools.insert(tool.id.clone(), tool);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Some(updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_back(lines: &[&str]) -> Vec<Value> {
        let dir = std::env::temp_dir().join(format!("loom-replay-{}", std::process::id()));
        let projects = dir.join("projects").join("p");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join("s1.jsonl"), lines.join("\n")).unwrap();
        // SAFETY: tests that read Claude Code's files run in this one test only.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &dir) };
        let updates = replay("s1", Path::new("/home/me/p")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        updates
    }

    #[test]
    fn a_conversation_reads_back_from_its_last_compaction() {
        let updates = read_back(&[
            r#"{"type":"user","uuid":"a","parentUuid":null,"message":{"content":"before"}}"#,
            r#"{"type":"system","subtype":"compact_boundary","uuid":"b","parentUuid":null,"logicalParentUuid":"a"}"#,
            r#"{"type":"user","uuid":"c","parentUuid":"b","message":{"content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA"}},{"type":"text","text":"what is this?"}]}}"#,
            r#"{"type":"attachment","uuid":"d","parentUuid":"c","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"and BANANA"}]}}"#,
            r#"{"type":"assistant","uuid":"e","parentUuid":"d","message":{"id":"m1","content":[{"type":"text","text":"A picture. BANANA"}]}}"#,
        ]);
        let kinds: Vec<String> = updates.iter().map(|u| format!("{}:{}", u["sessionUpdate"].as_str().unwrap(), u["content"]["type"].as_str().unwrap_or(""))).collect();
        assert_eq!(kinds, ["user_message_chunk:text", "user_message_chunk:image", "user_message_chunk:text", "agent_message_chunk:text"]);
        assert_eq!(updates[0]["content"]["text"], "what is this?");
        assert_eq!(updates[2]["content"]["text"], "and BANANA");
        assert_eq!(updates[3]["messageId"], "m1");
    }
}
