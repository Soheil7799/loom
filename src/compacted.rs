//! What a compacted conversation said before its compaction. Claude Code resumes a compacted
//! conversation from the summary, so session/load replays only what came after; the messages
//! before are read here from Claude Code's own file of it, following the chain back across
//! each compaction (what a rewind left behind is not on it).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// Something said before a compaction.
#[derive(Debug, PartialEq)]
pub enum Said {
    User(String),
    Agent(String),
    /// A compaction here (when: the file's ISO time).
    Compacted(String),
}

struct Record {
    parent: Option<String>,
    /// A compaction's link to the conversation it summed up.
    logical: Option<String>,
    boundary: Option<String>,
    said: Option<Said>,
}

/// The messages before the latest compaction, oldest first, at most `limit` of them (the
/// newest), and how many older ones were left out. Nothing if it was never compacted.
pub fn earlier(session: &str, limit: usize) -> (Vec<Said>, usize) {
    let Some(text) = crate::state::claude_file(session).and_then(|f| std::fs::read_to_string(f).ok()) else { return (Vec::new(), 0) };
    earlier_in(&text, limit)
}

fn earlier_in(text: &str, limit: usize) -> (Vec<Said>, usize) {
    if !text.contains("compact_boundary") {
        return (Vec::new(), 0);
    }
    let mut records: HashMap<String, Record> = HashMap::new();
    let mut last_boundary = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(uuid) = v["uuid"].as_str() else { continue };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let owned = |k: &str| v[k].as_str().map(str::to_owned);
        let boundary = (v["subtype"].as_str() == Some("compact_boundary")).then(|| owned("timestamp").unwrap_or_default());
        if boundary.is_some() {
            last_boundary = Some(uuid.to_owned());
        }
        records.insert(uuid.to_owned(), Record { parent: owned("parentUuid"), logical: owned("logicalParentUuid"), boundary, said: said(&v) });
    }
    let Some(start) = last_boundary.and_then(|b| records.get(&b)).and_then(|b| b.logical.clone()) else { return (Vec::new(), 0) };
    // Back from the last message before the latest compaction, to the very first.
    let mut back = Vec::new();
    let mut seen = HashSet::new();
    let mut at = Some(start);
    while let Some(uuid) = at.take() {
        if !seen.insert(uuid.clone()) {
            break;
        }
        let Some(record) = records.get_mut(&uuid) else { break };
        if let Some(when) = &record.boundary {
            back.push(Said::Compacted(when.clone()));
            at = record.logical.clone();
            continue;
        }
        if let Some(said) = record.said.take() {
            back.push(said);
        }
        at = record.parent.clone();
    }
    back.reverse();
    // The agent's texts between two tool calls are one reply.
    let mut all: Vec<Said> = Vec::new();
    for said in back {
        match (all.last_mut(), said) {
            (Some(Said::Agent(text)), Said::Agent(more)) => {
                text.push_str("\n\n");
                text.push_str(&more);
            }
            (_, said) => all.push(said),
        }
    }
    // A compaction first is the summary's own (nothing before it to show).
    while matches!(all.first(), Some(Said::Compacted(_))) {
        all.remove(0);
    }
    let left_out = all.len().saturating_sub(limit);
    (all.split_off(left_out), left_out)
}

/// What a line of the file says to read, if anything: the user's own words (not a command's
/// wrapping, a tool's result or the summary), or the agent's text.
fn said(v: &Value) -> Option<Said> {
    let content = &v["message"]["content"];
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n\n"),
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match v["type"].as_str()? {
        "user" => {
            const WRAPPED: [&str; 7] = ["<command-", "<local-command-", "<system-reminder>", "<task-notification>", "<bash-", "<user-prompt-submit-hook>", "[Request interrupted"];
            let skip = v["isMeta"].as_bool() == Some(true) || v["isCompactSummary"].as_bool() == Some(true) || WRAPPED.iter().any(|w| text.starts_with(w));
            (!skip).then(|| Said::User(text.to_owned()))
        }
        "assistant" => Some(Said::Agent(text.to_owned())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_came_before_a_compaction_is_found() {
        let lines = [
            r#"{"type":"user","uuid":"a","parentUuid":null,"message":{"content":"first question"}}"#,
            r#"{"type":"assistant","uuid":"b","parentUuid":"a","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use"}]}}"#,
            r#"{"type":"user","uuid":"c","parentUuid":"b","message":{"content":[{"type":"tool_result","content":"x"}]}}"#,
            r#"{"type":"assistant","uuid":"d","parentUuid":"c","message":{"content":[{"type":"text","text":"Found it."}]}}"#,
            r#"{"type":"user","uuid":"r","parentUuid":"b","message":{"content":"a rewound branch"}}"#,
            r#"{"type":"user","uuid":"e","parentUuid":"d","isMeta":true,"message":{"content":"<command-name>/compact</command-name>"}}"#,
            r#"{"type":"system","subtype":"compact_boundary","uuid":"f","parentUuid":null,"logicalParentUuid":"e","timestamp":"2026-10-28T11:15:58Z"}"#,
            r#"{"type":"user","uuid":"g","parentUuid":"f","isCompactSummary":true,"message":{"content":"This session is being continued"}}"#,
            r#"{"type":"user","uuid":"h","parentUuid":"g","message":{"content":"after"}}"#,
        ];
        let (said, left_out) = earlier_in(&lines.join("\n"), 10);
        assert_eq!(said, vec![Said::User("first question".into()), Said::Agent("Looking.\n\nFound it.".into())]);
        assert_eq!(left_out, 0);
        let (said, left_out) = earlier_in(&lines.join("\n"), 1);
        assert_eq!((said.len(), left_out), (1, 1));
        assert_eq!(earlier_in(lines[0], 10), (Vec::new(), 0));
    }
}
